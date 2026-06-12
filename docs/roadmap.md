# Development Roadmap

---

## Phase 1: Terminal Core

**Goal:** A window with a live shell prompt. Usable for daily terminal tasks.

### Tasks

1. **Dependency setup**
   - Add `eframe`, `egui`, `egui-wgpu`, `wgpu` to Cargo.toml
   - Add `vte`, `wezterm-font`, `portable-pty`, `etagere`, `copypasta`, `linkify`
   - Add `parking_lot`, `serde`, `toml`
   - Add `wezterm-toast-notification` (for future notification phase)
   - Integrate `alacritty_terminal/src/grid/`, `term/`, `selection/` as terminal core
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
   - Glyph atlas with etagere packing + wezterm-font rasterization (ligatures, emoji, fallback built-in)
   - Cell instance buffer: only upload changed cells (damage tracking)
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

## Phase 2: Zenmux Features

**Goal:** Multi-terminal workspace with AI agent notifications — Zenmux's differentiation.

### Tasks

1. **Multi-tab (egui_dock)**
   - Each tab holds one `TerminalSession`
   - Tab bar with open/close/reorder
   - Optional: window splitting (horizontal/vertical)

2. **Workspace sidebar** (cmux-inspired)
   - Vertical list of tabs with context info:
     - Git branch (read from `.git/HEAD`)
     - Current working directory
     - Exposed ports (if any)
     - Notification indicator (colored dot/ring)
   - Keyboard shortcuts for switching tabs

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

**Deliverable:** A cmux-equivalent experience — multiple terminal tabs, workspace sidebar, and first-class AI agent notifications.

---

## Phase 3: Production Ready

**Goal:** Polished, distributable terminal emulator.

### Tasks

1. **Configuration system**
   - TOML config file (`~/.config/zenmux/zenmux.toml`)
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
| `grid/` (ring buffer) | `alacritty/alacritty_terminal/src/grid/` | Apache 2.0 | Battle-tested, clean library design |
| `term/` (screen state) | `alacritty/alacritty_terminal/src/term/` | Apache 2.0 | Handles cursor, colors, modes, alternate screen |
| `selection/` | `alacritty/alacritty_terminal/src/selection/` | Apache 2.0 | Independent module, easy to adapt |
| `index/` (coords) | `alacritty/alacritty_terminal/src/index/` | Apache 2.0 | Row/Col/Line types |
| `input.rs` (key encoding) | `alacritty/alacritty/src/input.rs` | Apache 2.0 | Adapt from winit to egui events |
| `portable-pty` | `wezterm/pty/` (crate) | MIT | Cross-platform PTY, use as dependency |
| `wezterm-font` | `wezterm/wezterm-font/` (crate) | MIT | Full shaping + ligatures + emoji + fallback from day one |
| `wezterm-toast-notification` | `wezterm/wezterm-toast-notification/` (crate) | MIT | Cross-platform native notifications |
| `vtparse` patterns | `wezterm/vtparse/` | MIT | Reference for escape parsing edge cases |
| Overlay UI patterns | `wezterm/wezterm-gui/src/overlay/` | MIT | Command palette, search overlay reference |
