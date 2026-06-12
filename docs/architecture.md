# Zenmux Architecture

## Vision

A cross-platform (macOS + Windows + Linux) GPU-accelerated terminal emulator with workspace management and AI agent notification system, built with Rust.

Inspired by [cmux](https://cmux.com/) (macOS-only, Swift + libghostty) — Zenmux aims to be the cross-platform equivalent: beautiful UI, multiple workspaces, and first-class notifications for AI coding agents.

## Design Philosophy

1. **Rust-first, pragmatic about C** — Zero Zig dependencies. C dependencies (FreeType, HarfBuzz) are acceptable where pure Rust alternatives are unproven in production terminal use. The goal is a working terminal, not ideological purity.
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
 │  │  │ (wezterm-font)   │  │ Buffer   │  │ Call             │   │   │
 │  │  │                  │  │ (damage  │  │                  │   │   ││  │  └──────────────────┘  └──────────┘  └──────────────────┘   │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │  Terminal Sessions (one per tab/workspace)                    │   │
│  │  ┌────────────────┐  ┌─────────────────────┐  ┌──────────┐   │   │
│  │  │ portable-pty   │  │ alacritty_terminal  │  │ Notific. │   │   │
│  │  │ (PTY I/O)      │→ │ (vte + grid + term, │  │ System   │   │   │
│  │  │                │  │  from alacritty/)    │  │ (OSC 9/  │   │   │
│  │  │                │  │                     │  │  99/777) │   │   │
│  │  └────────────────┘  └─────────────────────┘  └──────────┘   │   │
│  └──────────────────────────────────────────────────────────────┘   │
```

## Data Flow (single frame)

```
User Input (keyboard / mouse)
    │
    ▼
egui event → input.rs → encode to terminal escape sequence
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
channel → main thread → vte::Parser + alacritty_terminal::Term
    │                  (grid/ring buffer, screen state, selection)
    ▼
egui::update() called
    │
    ├── egui_dock renders tab bar + sidebar, status bar (~0.1ms)
    │
    ├── Terminal mouse/key input processing (from egui events)
    │   ├── Mouse selection state update (click-drag, double/triple click)
    │   ├── SGR mouse encoding if terminal has mouse reporting enabled
    │   └── Keyboard → escape sequence via input.rs
    │
    └── Terminal area: egui_wgpu::CallbackTrait
        │
        ├── Snapshot alacritty_terminal::Grid (visible viewport)
        ├── Compare with previous frame → damage tracking (dirty rows)
        ├── Glyph atlas lookup (wezterm-font — ligatures, emoji, fallback built-in)
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

Mouse events are interpreted by Zenmux itself for text selection:

| Action | Behavior |
|--------|----------|
| Click-drag (left) | Select text in terminal grid |
| Double-click | Select word under cursor |
| Triple-click | Select entire line |
| Right-click | Show context menu (copy, paste, split) |
| Scroll wheel | Scroll back through scrollback buffer |
| Ctrl+Click | Click to open URL |

Selection state is tracked in `src/terminal/selection.rs` (adapted from `alacritty_terminal/src/selection.rs`). Selected cells are rendered with inverted or highlighted background color in the GPU shader.

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
zenmux/
├── Cargo.toml
├── docs/                       # This directory
├── src/
│   ├── main.rs                 # eframe entry point
│   ├── app.rs                  # App state, eframe::App impl
│   ├── config.rs               # TOML config loader
│   │
│   ├── ui/                     # egui UI chrome
│   │   ├── mod.rs
│   │   ├── tab.rs              # egui_dock integration
│   │   ├── sidebar.rs          # Workspace sidebar (like cmux)
│   │   ├── status_bar.rs       # Status bar
│   │   └── notification.rs     # Notification popovers
│   │
│   ├── terminal/               # Terminal engine + session management
│   │   ├── mod.rs
│   │   ├── session.rs          # TerminalSession: PTY + alacritty Term + notif
│   │   ├── grid.rs             # Thin wrapper around alacritty's Grid
│   │   ├── selection.rs        # Text selection logic (alacritty-based)
│   │   ├── notification.rs     # OSC 9/99/777 parser
│   │   └── input.rs            # Keyboard → escape sequence encoding
│   │                           # (adapted from alacritty/src/input.rs)
│   │
│   ├── render/                 # GPU rendering pipeline
│   │   ├── mod.rs
│   │   ├── pipeline.rs         # wgpu pipeline + CallbackTrait impl
│   │   ├── glyph_atlas.rs      # Glyph cache + etagere texture atlas
│   │   ├── font.rs             # Font loading via wezterm-font
│   │   └── shader.wgsl         # Terminal grid vertex/fragment shader
│   │
│   └── theme.rs                # Color scheme, spacing, typography tokens
│
├── alacritty/                  # Vendored: Alacritty source (reference + reuse)
└── wezterm/                    # Vendored: Wezterm source (reference + reuse)

## Key Technical Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| UI Framework | egui + eframe | Mature, cross-platform, WASM-ready, immediate-mode |
| Tabs/Docking | egui_dock | 594 stars, production-proven tab/split/dock |
| Terminal Core | `vte` + alacritty's `grid`/`term` | Battle-tested grid ring buffer, screen state, selection, and VT parsing from the vendored `alacritty/` directory. Full control over damage tracking and custom features. |
| PTY | `portable-pty` | Wezterm's crate, cross-platform (ConPTY on Windows) |
| Font Loading | `wezterm-font` | Wezterm's font stack: FreeType + HarfBuzz + Cairo. Full shaping, ligatures, BiDi, emoji, and font fallback from day one. No migration needed. |
| Glyph Atlas | `etagere` | Efficient space packing for GPU glyph storage |
| GPU API | wgpu | Cross-platform (Vulkan/Metal/DX12/WebGPU) |
| Terminal GPU Render | `egui_wgpu::CallbackTrait` | Renders terminal inline within egui's render pass — same window, same frame. No intermediate textures. One instanced draw call for the whole grid. |
| Config | TOML + serde | Simple, familiar, hot-reloadable |
| Clipboard | `copypasta` | Cross-platform, used by Alacritty |
| URL Detection | `linkify` | Standard, lightweight |

## Reference Projects

| Project | Why Reference |
|---------|---------------|
| **Alacritty** (~33k LOC) | **Core reuse target.** Grid/ring buffer (`grid/`), terminal state (`term/`), input encoding (`input.rs`), selection (`selection.rs`). All vendored in `alacritty/` directory. |
| **Wezterm** (~413k LOC) | `portable-pty` (used directly), `wezterm-font` (font+shaping from day one), `wezterm-toast-notification` (native notifications), overlay UI patterns. Vendored in `wezterm/` directory. |
| **cmux** | Workspace sidebar design, notification system UX, vertical tabs (inspiration only — macOS-only, Swift). |
| **Terminal Studio** | egui + wgpu terminal approach. **What NOT to do:** it used egui's text system (`ui.label()`) for terminal cells, resulting in 1920+ draw calls per frame. **Lesson:** Use `CallbackTrait` + custom wgpu instanced rendering, NOT egui text for terminal. |
| **Zed Editor** | Positive example of egui + custom GPU rendering coexistence via CallbackTrait for complex text/content areas. |
