# Development Roadmap

---

## Phase 1: Terminal Core

**Goal:** A window with a live shell prompt. Usable for daily terminal tasks.

### Tasks

 1. **Dependency setup**
    - Add `eframe`, `egui`, `egui-wgpu`, `wgpu` to Cargo.toml
    - Add `vte`, `cosmic-text`, `portable-pty`, `etagere`, `copypasta`, `linkify`
    - Add `parking_lot`, `serde`, `toml`
    - Verify builds on macOS, Windows, and Linux
2. **PTY + terminal session**
   - `TerminalSession` struct: `portable-pty::PtyMaster` + `vte::Parser` + `alacritty_terminal::term::Term`
   - Background thread reads PTY bytes → channel → main thread feeds `vte::Parser`
   - `Perform` impl bridges `vte` actions into `Term` (grid state, cursor, colors, alternate screen)
   - Handle `PtySize` on window resize → `Term::resize()`
   - Damage tracking: mark dirty rows for GPU upload

3. **Glyph atlas rendering pipeline**
   - Implement `egui_wgpu::CallbackTrait`
   - Vertex shader + fragment shader (WGSL) for instanced quad rendering
    - Glyph atlas with etagere packing + cosmic-text rasterization (ligatures, emoji, fallback built-in)   - Cell instance buffer: only upload changed cells (damage tracking)
   - ONE instanced draw call for the entire visible grid

4. **vte → Term bridge**
   - Implement `vte::Perform` that forwards parser actions to `alacritty_terminal::Term`
   - Handle: `print`, `execute`, `csi_dispatch`, `osc_dispatch`, `esc_dispatch`
   - This is the glue layer that makes vte + alacritty_terminal work together

5. **Input handling (basic)**
   - Adapt `alacritty/src/input.rs` from winit events to egui events
   - Standard ASCII encoding: letters, digits, Enter, Backspace, Tab, Escape
   - Ctrl combinations: Ctrl+C, Ctrl+D, Ctrl+L, etc.
   - Arrow keys: standard `\x1b[A` / `\x1b[B` / `\x1b[C` / `\x1b[D` encoding

6. **egui window**
   - `eframe::App::update()` with `CentralPanel`
   - Terminal area fills the panel (via `PaintCallback` + `CallbackTrait`)
   - PTY output → `vte::Parser` → `Term` → read grid cells → render via wgpu
   - Mouse click-drag for text selection (adapted from alacritty selection.rs)
   - Right-click context menu (copy, paste)

**Deliverable:** A live terminal in an egui window. Can run `ls`, `cat`, `vim`, `htop`. No tabs, no sidebar, no notifications.

---

## Phase 2: Zenterm Features

**Goal:** Multi-terminal workspace with AI agent notifications — Zenterm's differentiation.

### Tasks

 1. **Multi-tab (egui_dock)** — ✅ **DONE**
    - Each tab holds one `TerminalSession`
    - Tab bar with open/close/reorder (egui_dock DockArea)
    - Window splitting deferred to "Pane splitting" under Future
    - Per-session terminal state isolated; shared `wgpu::Device` and `SharedGlyphAtlas`
    - Dock layout persisted to `~/.config/zenterm/dock.json`; per-session metadata to `sessions.json`

 2. **Workspace management** (cmux-inspired) — ✅ **DONE**
    - Workspace abstraction: `WorkspaceManager` with named workspace grouping
    - Sidebar shows workspace → tab tree (hierarchical, not flat)
    - Workspace operations: create, rename (double-click or context menu), switch, close
    - Close workspace migrates tabs to another workspace
    - Auto-naming based on current directory
    - Keyboard shortcuts: `Ctrl+1..9` switch by index, `Ctrl+Tab` cycle
    - Session restoration: all persisted tabs are re-created on startup
    - `config.ui.sidebar_enabled` toggles visibility (default `false`)

 3. **Input handling (advanced)**
    - SGR mouse reporting (`?1006`) — vim/htop/nano mouse support
    - Kitty keyboard protocol (progressive enhancement)
    - Bracketed paste mode
    - Application cursor keys (terminal mode-dependent)
4. **Agent notifications**
   - OSC handler: detect `OSC 9` / `OSC 99` / `OSC 777` notification sequences
   - Sidebar badge + popover notification
   - OS-native notifications via `wezterm-toast-notification` (macOS, Windows, Linux)

5. **Clipboard integration**
   - Seamless copy/paste between terminal and system clipboard
   - Selection auto-copy (optional, like macOS terminal)
   - OSC 52 clipboard escape sequence support

**Deliverable:** A cmux-equivalent experience — multiple workspaces with tab grouping, workspace sidebar, and first-class AI agent notifications.

---

## Phase 3: Production Ready

**Goal:** Polished, distributable terminal emulator.

### Tasks

1. **Configuration system**
   - TOML config file (`~/.config/zenterm/zenterm.toml`)
   - Font family, font size, color scheme
   - Keybinding customization
   - Hot-reload on file change

2. **URL detection + click to open**
   - Auto-detect URLs in terminal output via `linkify`
   - Ctrl+Click to open in browser

3. **Performance tuning**
   - Profile rendering pipeline end-to-end
   - Optimize glyph atlas → minimize texture uploads
   - GPU → CPU readback-free rendering
   - Glyph cache warmup (common ASCII on startup)
   - Benchmark: match Alacritty within 20%

4. **WASM build** (optional, platform-dependent)
   - Compile core to `wasm32-unknown-unknown`
   - WebSocket PTY proxy (browser → native server → shell)
   - Same rendering code via WebGPU

5. **Distribution**
   - macOS: `.app` bundle via `cargo bundle`
   - Windows: installer via `nsis` or `wix`
   - Linux: AppImage or Flatpak

**Deliverable:** A polished, production-ready terminal emulator that ships on all three platforms.

---

## Future / Optional

- **Pane splitting** — Multiple terminals in one tab
- **Remote SSH sessions** — Built-in SSH client
- **Session persistence** — Like tmux, detach/reattach
- **Plugin system** — Lua or WASM-based plugins
- **Image support** — Kitty graphics protocol, sixel
- **Browser tabs** — Embedded web view (like cmux)

---

## Source Code Strategy

| Component | Source | License | Why |
|-----------|--------|---------|-----|
| `alacritty_terminal` (grid, term, selection, index) | `alacritty_terminal` on crates.io | Apache 2.0 | Battle-tested grid ring buffer, screen state, selection. Published as a library crate. |
| `input.rs` (key encoding) | `alacritty/src/input.rs` (vendored reference) | Apache 2.0 | Adapt from winit to egui events. Keep as reference in `alacritty/` submodule. |
| `portable-pty` | `portable-pty` on crates.io | MIT | Cross-platform PTY. Published by the wezterm team. |
| `cosmic-text` | `cosmic-text` on crates.io | MIT / Apache 2.0 | Pure Rust shaping + ligatures + emoji + fallback. Published by System76. |
| `wezterm-toast-notification` | `wezterm/wezterm-toast-notification/` (vendored, Phase 2+) | MIT | Cross-platform native notifications. Not on crates.io; vendor when needed. |
| `vte` | crates.io | Apache 2.0 | Low-level VT state machine. |
| `etagere` | crates.io | MIT / Apache 2.0 | Efficient texture atlas packing. |
| `vtparse` patterns | `wezterm/vtparse/` (reference) | MIT | Reference for escape parsing edge cases. |
| Overlay UI patterns | `wezterm/wezterm-gui/src/overlay/` (reference) | MIT | Command palette, search overlay reference. |
