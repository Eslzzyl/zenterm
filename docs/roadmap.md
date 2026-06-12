# Development Roadmap

Estimated timeline for a solo developer working evenings/weekends.

---

## Phase 0: Scaffold (Week 1) ✅ Done

- [x] Initialize project (Cargo.toml, basic structure)
- [x] Create docs directory
- [x] Document architecture decisions

**Deliverable:** Empty egui window that compiles and runs.

---

## Phase 1: Hello, Shell (~2-4 weeks)

Goal: A window with a black rectangle that shows a live shell prompt.

### Tasks

1. **Dependency setup**
   - Add `eframe`, `egui`, `egui-wgpu`, `wgpu` to Cargo.toml
   - Add `vt100`, `portable-pty`, `cosmic-text`, `swash`, `etagere`
   - Verify builds on macOS and Windows

2. **PTY + terminal session**
   - `TerminalSession` struct: `portable-pty::PtyMaster` + `vt100::Parser`
   - Background thread reads PTY bytes → channel → main thread feeds parser
   - Handle `PtySize` on window resize

3. **Glyph atlas rendering pipeline**
   - Implement `egui_wgpu::CallbackTrait`
   - Vertex shader + fragment shader (WGSL) for instanced quad rendering
   - Glyph atlas with etagere packing
   - Basic font loading via cosmic-text/swash

4. **egui window**
   - `eframe::App::update()` with `CentralPanel`
   - Terminal area fills the panel
   - PTY output → `vt100::Parser::process()` → read cells → render

**Deliverable:** egui window with a live bash/zsh shell. Can run `ls`, `cat`, basic commands. No tabs, no styling beyond default.

---

## Phase 2: Core Terminal (~2-4 weeks)

Goal: Feature-complete terminal experience.

### Tasks

1. **Complete ANSI support**
   - Verify `vt100` handles: 256-color, truecolor, bold/italic/underline, alternate screen, mouse tracking
   - Fix gaps if any

2. **Terminal features**
   - Scrollback (vt100 handles this)
   - Text selection (click-drag highlight, copy to clipboard)
   - Copy/paste (Ctrl+C/V, right-click menu)
   - URL detection + click to open

3. **Input handling**
   - Implement `input.rs` borrowing Alacritty's approach
   - Standard key encoding
   - Kitty keyboard protocol (progressive)
   - Mouse event encoding (SGR format)

4. **Configuration**
   - TOML config file
   - Font family, font size, color scheme
   - Keybindings

5. **Performance tuning**
   - Profile rendering pipeline
   - Optimize glyph atlas → minimize texture uploads
   - Implement damage tracking (only re-render changed cells)
   - Benchmark: should match Alacritty within 20%

**Deliverable:** A usable daily-driver terminal. Could replace your system terminal for most tasks.

---

## Phase 3: Tabs & Workspaces (~2-4 weeks)

Goal: Multi-terminal management inspired by cmux.

### Tasks

1. **egui_dock integration**
   - Each tab = one `TerminalSession`
   - Tab bar with open/close/reorder
   - Optional: window splitting (horizontal/vertical)

2. **Workspace sidebar** (like cmux)
   - Vertical list of tabs with:
     - Git branch (read from `.git/HEAD`)
     - Current working directory
     - Exposed ports (if any)
     - Notification indicator (colored dot/ring)
   - Keyboard shortcuts for switching

3. **Tab management**
   - Create new tab (Ctrl+T)
   - Close tab (Ctrl+W)
   - Switch between tabs (Ctrl+Tab, Ctrl+Shift+Tab)
   - Reorder by drag-and-drop

4. **Persistence**
   - Remember open tabs and their working directories between sessions
   - Restore on next launch

**Deliverable:** cmux-equivalent experience — multiple terminal tabs with workspace sidebar.

---

## Phase 4: Agent Notifications (~1-2 weeks)

Goal: First-class notification system for AI coding agents.

### Tasks

1. **OSC handler**
   - Detect `OSC 9` (terminal-notifier compatible)
   - Detect `OSC 99` (custom notification protocol)
   - Detect `OSC 777` (iTerm2 notification protocol)
   - Parse notification text

2. **Notification UI**
   - Sidebar badge/ring on the target tab
   - Popover notification (fade in from sidebar)
   - macOS native notification (via `notify-rust` or AppleScript)
   - Windows toast notification

3. **Agent integration**
   - Claude Code, Codex, Aider, etc. — automatic detection
   - Git-aware: show current branch + PR status
   - Plugin/hook system for custom notification triggers

**Deliverable:** Agent-friendly terminal with visual and OS-level notifications.

---

## Phase 5: Polish & Advanced Features (~4-8 weeks)

### Tasks

1. **Performance to parity**
   - Full damage tracking (cell-level, not line-level)
   - GPU → CPU readback-free rendering
   - Sub-pixel text positioning
   - Glyph cache warmup (common ASCII on startup)

2. **Ligatures + complex text**
   - Verify ligatures work across font families
   - Test CJK, Arabic, emoji
   - Font fallback ordering

3. **Advanced UI**
   - Tab theming (per-tab color accents)
   - Transparency/background blur (macOS)
   - Search overlay (Ctrl+F, highlight matches)
   - Command palette (Cmd+Shift+P)

4. **WASM build** (if desired)
   - Compile core to `wasm32-unknown-unknown`
   - WebSocket PTY proxy (browser → server → shell)

5. **Distribution**
   - macOS: `.app` bundle via `cargo bundle`
   - Windows: installer via `nsis` or `wix`
   - Linux: AppImage or Flatpak

**Deliverable:** Polished, production-ready terminal emulator.

---

## Future / Optional

- **Pane splitting** — Multiple terminals in one tab
- **Remote SSH sessions** — Built-in SSH client
- **Session persistence** — Like tmux, detach/reattach
- **Plugin system** — Lua or WASM-based plugins
- **Image support** — Kitty graphics protocol, sixel
- **Browser tabs** — Embedded web view (like cmux)

---

## Reference: How to Copy from Alacritty

| Alacritty Module | Lines | What to Take | License |
|-----------------|-------|--------------|---------|
| `input.rs` | ~1,800 | Keyboard → escape sequence mapping | Apache 2.0 |
| `grid/storage.rs` | ~800 | Ring buffer implementation | Apache 2.0 |
| `grid/row.rs` | ~300 | Row (line of cells) representation | Apache 2.0 |
| `term/mod.rs` | ~3,000 | Terminal state machine pattern | Apache 2.0 |
| `renderer/text/atlas.rs` | ~200 | Simple texture atlas packing | Apache 2.0 |

## Reference: How to Copy from Wezterm

| Module | What to Take | License |
|--------|-------------|---------|
| `pty/` (portable-pty) | Use as crate dependency | MIT |
| `wezterm-font/` | Font fallback ordering logic | MIT |
| `term/src/terminalstate/` | Escape sequence handling patterns | MIT |

---

## Decision Log

### 2025-06-11: Rejected libghostty-vt

**Context:** `libghostty-vt` is a Rust crate that wraps Ghostty's terminal emulation engine.

**Reasons for rejection:**
- Requires Zig 0.15.x at build time (user has no interest in Zig)
- Windows build has documented issues with Zig path resolution
- C API still unstable (v0.1.1)
- Pure Rust alternatives (`vt100`) are mature and sufficient

**Decision:** Use `vt100` crate as the terminal engine.

### 2025-06-11: Rejected restty + Tauri approach

**Context:** Ghostty WASM in Tauri WebView worked on macOS but was unstable on Windows.

**Reason:** WebView rendering and PTY handling differs significantly between macOS and Windows. WebView abstraction leaks platform differences. Native approach (egui + wgpu) eliminates this variable.

**Decision:** Native Rust stack (eframe + wgpu) for all platforms.

### 2025-06-11: Rejected Wezterm as base

**Context:** Successfully modded Wezterm for workspace tabs but found it heavy (600MB RAM, 413k LOC, 64 crate workspace, slow compile).

**Decision:** Build from scratch with focused scope (terminal + tabs + notifications), referencing Wezterm's `portable-pty` crate and learning from its architecture.
