# Zenmux Architecture

## Vision

A cross-platform (macOS + Windows + Linux) GPU-accelerated terminal emulator with workspace management and AI agent notification system, built with Rust.

Inspired by [cmux](https://cmux.com/) (macOS-only, Swift + libghostty) — Zenmux aims to be the cross-platform equivalent: beautiful UI, multiple workspaces, and first-class notifications for AI coding agents.

## Design Philosophy

1. **Pure Rust** — Zero C/Zig dependencies. Every component must be pure Rust or have pure Rust alternatives.
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
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │  egui UI Chrome                                               │   │
│  │  ┌──────────┐  ┌────────────────┐  ┌──────────────────────┐  │   │
│  │  │ Tab Bar  │  │ Workspace      │  │ Status Bar           │  │   │
│  │  │ (egui_   │  │ Sidebar        │  │ (current dir, git,   │  │   │
│  │  │ dock)    │  │ (vertical,     │  │ branch, agent status) │  │   │
│  │  │          │  │  like cmux)    │  │                      │  │   │
│  │  └──────────┘  └────────────────┘  └──────────────────────┘  │   │
│  │  ┌──────────────────────────────────────────────────────────┐ │   │
│  │  │ Overlay System                                           │ │   │
│  │  │ (command palette, search, notification popover, settings)│ │   │
│  │  └──────────────────────────────────────────────────────────┘ │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │  Terminal Rendering Pipeline (per-tab)                        │   │
│  │  (egui_wgpu::CallbackTrait — custom wgpu render pass)         │   │
│  │                                                                  │   │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────────┐   │   │
│  │  │ vt100    │→ │ Cosmic   │→ │ Glyph    │→ │ wgpu         │   │   │
│  │  │ Screen   │  │ Text     │  │ Atlas    │  │ Instanced    │   │   │
│  │  │ (cells)  │  │ (shaping,│  │ (etagere)│  │ Draw Call    │   │   │
│  │  │          │  │ ligature)│  │          │  │              │   │   │
│  │  └──────────┘  └──────────┘  └──────────┘  └──────────────┘   │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │  Terminal Sessions (one per tab/workspace)                    │   │
│  │  ┌────────────────┐  ┌────────────────┐  ┌──────────────┐   │   │
│  │  │ portable-pty   │  │ vt100::Parser  │  │ Notification │   │   │
│  │  │ (PTY I/O)      │→ │ (VT parsing,   │  │ System       │   │   │
│  │  │                │  │  screen state)  │  │ (OSC 9/99/  │   │   │
│  │  │                │  │                │  │  777)        │   │   │
│  │  └────────────────┘  └────────────────┘  └──────────────┘   │   │
│  └──────────────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────────┘
```

## Data Flow (single frame)

```
User Input (keyboard)
    │
    ▼
egui event → input.rs → encode to terminal escape sequence
    │
    ▼
portable-pty::PtyMaster::write(bytes)
    │
    ▼
[Shell (bash/zsh/pwsh) processes input, produces output]
    │
    ▼
portable-pty::PtyMaster::read() → raw bytes
    │
    ▼
vt100::Parser::process(bytes) → updates Screen state
    │
    ▼
egui::update() called
    │
    ├── egui_dock renders tab bar + sidebar (~0.1ms)
    │
    └── Terminal area: egui_wgpu::CallbackTrait
        │
        ├── Read vt100::Screen cells (only visible viewport)
        ├── Cosmic-text shaping (ligatures, fallback)
        ├── Glyph atlas lookup/upload
        ├── Batch cells into instanced draw data
        └── wgpu draw call → GPU renders the grid
    │
    ▼
egui frame complete → swap buffers
```

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
│   │   ├── session.rs          # TerminalSession: PTY + vt100 + notification
│   │   ├── notification.rs     # OSC 9/99/777 parser
│   │   └── input.rs            # Keyboard → escape sequence encoding
│   │                           # (reference: alacritty input module)
│   │
│   ├── render/                 # GPU rendering pipeline
│   │   ├── mod.rs
│   │   ├── pipeline.rs         # wgpu pipeline + CallbackTrait
│   │   ├── glyph_atlas.rs      # Glyph cache + texture atlas
│   │   ├── shaper.rs           # cosmic-text integration
│   │   └── shader.wgsl         # Terminal grid vertex/fragment shader
│   │
│   └── theme.rs                # Color scheme, spacing, typography tokens
```

## Key Technical Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| UI Framework | egui + eframe | Mature, cross-platform, WASM-ready, immediate-mode |
| Tabs/Docking | egui_dock | 594 stars, production-proven tab/split/dock |
| Terminal Engine | vt100 crate | Pure Rust, 7M downloads, mature, clean API |
| PTY | portable-pty | Wezterm's crate, cross-platform (ConPTY on Windows) |
| Font Shaping | cosmic-text | Ligatures, BiDi, emoji, font fallback |
| Glyph Rasterization | swash (via cosmic-text) | Pure Rust, fast, GPU-ready |
| Texture Atlas | etagere | Efficient space packing for GPU glyph storage |
| GPU API | wgpu | Cross-platform (Vulkan/Metal/DX12/WebGPU) |
| Config | TOML + serde | Simple, familiar, hot-reloadable |

## Reference Projects

| Project | Why Reference |
|---------|---------------|
| **Alacritty** (~33k LOC) | Grid/ring buffer implementation, rendering loop architecture, input encoding |
| **Wezterm** (~413k LOC) | portable-pty crate, cross-platform approaches, multiplexer concepts |
| **cmux** | Workspace sidebar design, notification system UX, vertical tabs |
| **Terminal Studio** | egui + wgpu terminal approach (as cautionary example of what NOT to do for text rendering) |
