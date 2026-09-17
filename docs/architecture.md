# Zenterm Architecture

## Vision

A cross-platform (macOS + Windows + Linux) GPU-accelerated terminal emulator with workspace management and AI agent notification system, built with Rust.

Inspired by [cmux](https://cmux.com/) (macOS-only, Swift + libghostty) — Zenterm aims to be the cross-platform equivalent: beautiful UI, multiple workspaces, and first-class notifications for AI coding agents.

## Design Philosophy

1. **Rust-first, pure Rust preferred** — Zero Zig dependencies. Prefer pure Rust crates where they are production-proven; C dependencies accepted only as last resort. The goal is a working terminal, not ideological purity.
2. **Leverage ecosystem** — Don't reinvent the wheel. Use mature crates for VT parsing, PTY, GPU rendering, and UI.
3. **Separation of concerns** — UI chrome (tabs, sidebar, settings) is decoupled from terminal rendering. Each uses the best tool for its job.
4. **Performance budget** — UI chrome < 0.5ms per frame, terminal rendering < 1ms per frame. Total < 16ms (60 FPS) with headroom.
5. **WASM-friendly architecture** — Core components designed to optionally compile to WebAssembly for browser deployment.

## High-Level Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                        eframe / egui                                 │
│  (window management, event loop, UI layout, WASM support)           │
│                                                                      │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │  Terminal Rendering Pipeline (per-tab)                        │   │
│  │  (egui_wgpu::CallbackTrait — custom wgpu render pass)         │   │
│  │                                                                  │   │
│  │  ┌──────────────────┐  ┌──────────┐  ┌──────────────────┐   │   │
 │  │  │ Glyph Atlas      │  │ Cell     │→ │ wgpu             │   │   │
 │  │  │ (etagere packed) │  │ Instance │  │ Instanced Draw   │   │   │
 │  │  │ (cosmic-text)    │  │ Buffer   │  │ Call             │   │   │
 │  │  │                  │  │ (damage  │  │                  │   │   ││  │  └──────────────────┘  └──────────┘  └──────────────────┘   │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │  Terminal Sessions (one per tab, grouped by workspace)        │   │
│  │  ┌────────────────┐  ┌─────────────────────┐  ┌──────────┐   │   │
│  │  │ portable-pty   │  │ alacritty_terminal  │  │ Notific. │   │   │
│  │  │ (PTY I/O)      │→ │ (vte + grid + term, │  │ System   │   │   │
│  │  │                │  │  terminal core      │  │ (OSC 9/  │   │   │
│  │  │                │  │ + vte + grid)       │  │  99/777) │   │   │
│  │  └────────────────┘  └─────────────────────┘  └──────────┘   │   │
│  └──────────────────────────────────────────────────────────────┘   │
```

## Workspace Hierarchy

Terminal sessions are organized into **workspaces** — named groups of tabs:

```
ZentermApp
 ├── sessions: HashMap<SessionId, TerminalSession>   (shared pool)
 ├── workspaces: WorkspaceManager
 │    ├── Workspace { id, name, dock: DockState<SessionId> }
 │    ├── Workspace { id, name, dock: DockState<SessionId> }
 │    └── ...
 └── active_session_id: Option<SessionId>
```

Each workspace owns its own `DockState` (egui_dock layout tree), so
different workspaces can have independent tab arrangements.  The
session pool is shared — a `SessionId` is globally unique.

Keyboard shortcuts: `Ctrl+1..9` switches workspace by index,
`Ctrl+Tab` / `Ctrl+Shift+Tab` cycles through workspaces.

## Data Flow (single frame)

```
User Input (keyboard / mouse)
    │
    ▼
egui event → zenterm-input → encode to terminal escape sequence
    │                   (alacritty-style key encoding)
    ▼
portable-pty::PtyMaster::write(bytes)
    │
    ▼
[Shell (bash/zsh/pwsh) processes input, produces output]
    │
    ▼
portable-pty::PtyMaster::read() → raw bytes (background thread)
    │
    ▼
channel → main thread → zenterm-term::Terminal
    │                  (grid/ring buffer, screen state, selection)
    ▼
egui::update() called
    │
    ├── egui_dock renders tab bar + sidebar, status bar (~0.1ms)
    │
    ├── Terminal mouse/key input processing (from egui events)
    │   ├── Mouse selection state update (click-drag, double/triple click)
    │   ├── SGR mouse encoding if terminal has mouse reporting enabled
    │   └── Keyboard → escape sequence via zenterm-input
    │
    └── Terminal area: egui_wgpu::CallbackTrait
        │
        ├── Snapshot alacritty_terminal::Grid (visible viewport)
        ├── Compare with previous frame → damage tracking (dirty rows)
        ├── Glyph atlas lookup (cosmic-text — ligatures, emoji, fallback built-in)
        ├── Upload only changed cell instance data to GPU
        ├── ONE instanced wgpu draw call for the entire grid
        └── GPU renders via terminal.wgsl shader
    │
    ▼
egui frame complete → swap buffers (vsync)
```

## Mouse Interaction Model

The terminal has two mouse modes, determined by the program running inside:

### Mode 1: Selection Mode (default — bash, zsh, most CLI tools)

Mouse events are interpreted by Zenterm itself for text selection:

| Action | Behavior |
|--------|----------|
| Click-drag (left) | Select text in terminal grid |
| Double-click | Select word under cursor |
| Triple-click | Select entire line |
| Right-click | Show context menu (copy, paste, split) |
| Scroll wheel | Scroll back through scrollback buffer |
| Ctrl+Click | Click to open URL |

Selection state is exposed by `zenterm-term` and implemented in `crates/zenterm-term/src/term/terminal/selection.rs`, on top of `alacritty_terminal`'s selection type. Selected cells are rendered with inverted or highlighted background color in the GPU shader.

### Mode 2: Mouse Report Mode (vim, htop, nano, mc)

When the shell application enables mouse tracking (via `\x1b[?1000h`, `\x1b[?1006h`):
- All mouse events are encoded as **SGR escape sequences** (protocol `?1006`) and forwarded to the PTY
- Clicks, drags, and scroll wheel are encoded as `\x1b[<row;col;btn M` / `m`

### Implementation in egui

```rust
// Inside App::update(), for the terminal area:
let pointer = ui.input(|i| i.pointer.clone());  // same-frame pointer state

if self.term.mode().contains(MOUSE_REPORT) {
    // Forward to PTY as SGR escape sequences
    let seq = encode_sgr_mouse(cell_pos, button, event_type);
    self.pty_writer.write(seq.as_bytes());
} else {
    // Handle selection locally
    if response.dragged() { self.selection.update(cell_pos); }
    if response.double_clicked() { self.selection.select_word(cell_pos); }
}
```

**Key guarantee:** `egui::PointerState` reflects the current frame's events with no one-frame delay, so mouse-report mode has identical latency to native terminal emulators.

## Directory Structure

```
zenterm/
├── Cargo.toml
├── docs/                       # This directory
├── crates/
│   ├── zenterm/                # eframe entry point
│   ├── zenterm-ui/             # App orchestration, sessions, workspaces, settings
│   ├── zenterm-term/           # VT processing, terminal state, grid projection, images
│   ├── zenterm-input/          # Keyboard and Kitty keyboard encoding
│   ├── zenterm-pty/            # Cross-platform PTY creation and I/O
│   ├── zenterm-render/         # wgpu callback, shaders, cell-instance uploads
│   ├── zenterm-glyph/          # Font discovery, shaping, rasterization, atlas packing
│   ├── zenterm-config/         # TOML configuration, defaults, hot-reload diffing
│   └── zenterm-core/           # Shared cells, colors, sizes, damage, images, errors
│
├── docs/                       # Architecture, components, configuration, roadmap
├── terminal-render-test/       # Cross-platform manual/automated terminal probes
└── .github/workflows/          # Push/PR CI and tagged release packaging

## Key Technical Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| UI Framework | egui + eframe | Mature, cross-platform, WASM-ready, immediate-mode |
| Tabs/Docking | egui_dock | 594 stars, production-proven tab/split/dock |
| Terminal Core | `zenterm-term` + `alacritty_terminal` + `vte` | Owns terminal protocol handling, grid projection, selection, OSC/graphics extensions, and damage propagation while reusing the published Alacritty terminal engine. |
| PTY | `zenterm-pty` + `portable-pty` | Cross-platform PTY abstraction, including ConPTY on Windows. |
| Font Loading | `zenterm-glyph` + `cosmic-text` | Font discovery, shaping, rasterization, fallback, and ligature handling. |
| Glyph Atlas | `zenterm-glyph` + `etagere` | Shared atlas allocation and cached glyph image data. |
| GPU API | `zenterm-render` + `wgpu` | Cross-platform (Vulkan/Metal/DX12/WebGPU) callback and instanced cell rendering. |
| Terminal GPU Render | `egui_wgpu::CallbackTrait` | Renders terminal cells, images, and decorations inside egui's wgpu pass. |
| Config | `zenterm-config` + TOML + serde | Typed sections, platform-aware defaults, persistence, and hot-reload diffing. |
| Clipboard | `arboard` | Cross-platform clipboard access used by terminal sessions. |
| URL Detection | `linkify` in `zenterm-ui` | Detects visible URLs and email addresses before applying open-link policy. |

## Reference Projects

| Project | Why Reference |
|---------|---------------|
| **Alacritty** | Grid/ring buffer, terminal state, selection, index types, and terminal protocol behavior through the published `alacritty_terminal` crate. |
| **WezTerm** | `portable-pty` API and cross-platform PTY behavior; reference for terminal font and notification design. No source checkout is required by this project. |
| **cmux** | Workspace sidebar design, notification system UX, vertical tabs (inspiration only — macOS-only, Swift). |
| **Terminal Studio** | egui + wgpu terminal approach. **What NOT to do:** it used egui's text system (`ui.label()`) for terminal cells, resulting in 1920+ draw calls per frame. **Lesson:** Use `CallbackTrait` + custom wgpu instanced rendering, NOT egui text for terminal. |
| **Zed Editor** | Positive example of egui + custom GPU rendering coexistence via CallbackTrait for complex text/content areas. |
