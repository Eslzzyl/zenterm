# Terminal Emulator Glossary

Not sure what a term means? Start here.

---

## A

### Alternate Screen Buffer
A separate screen buffer that full-screen programs (vim, less, htop) switch to. When they exit, the original screen content is restored. Controlled by `\x1b[?1049h` (enable) / `\x1b[?1049l` (disable).

### ANSI Escape Sequence
A standardized way to control terminal formatting, color, and cursor positioning. Sequences start with `ESC [` (the Control Sequence Introducer, CSI). Example: `\x1b[31m` = set text color to red.

---

## B

### BiDi (Bidirectional Text)
Support for mixing left-to-right (English) and right-to-left (Arabic, Hebrew) text in the same document.

### Bracketed Paste Mode
When enabled (`\x1b[?2004h`), pasted text is surrounded by `\x1b[200~` and `\x1b[201~`. This lets applications distinguish typed text from pasted text, preventing accidental command execution.

---

## C

### CallbackTrait
An egui-wgpu trait that lets you inject custom wgpu rendering commands into egui's render pass. The terminal grid renders via this trait — it runs inside egui's render pass at the correct position in the layout, no intermediate textures needed. NOT for egui text rendering (that's what Terminal Studio did wrong).

### Cell
The fundamental unit of a terminal display. Each cell holds one character plus its style attributes (color, bold, italic, underline, etc.). 80×24 terminal = 1,920 cells.

### ConPTY (Console Pseudo-Terminal)
Windows's equivalent of Unix PTY. Introduced in Windows 10. Uses `CreatePseudoConsole` and handles communication via pipes. More complex than Unix PTY.

### Control Sequence (CSI)
Terminal commands starting with `\x1b[` (0x1B 0x5B). Examples:
- `\x1b[31m` — Set foreground color to red
- `\x1b[2J` — Clear entire screen
- `\x1b[1;1H` — Move cursor to row 1, column 1

### cosmic-text
Pure Rust text shaping library by System76. Wraps `rustybuzz` (pure Rust HarfBuzz) for shaping + `swash` for rasterization + `fontdb` for font discovery. Supports ligatures, BiDi, emoji, and font fallback with system-defined priority lists. **Zenterm's font backend** — pure Rust, zero C dependencies, full shaping from day one. Published on crates.io at `cosmic-text`.

### crossfont
Alacritty's font loading and rasterization library. Handles font discovery, size loading, and glyph bitmap generation via platform-native backends (FreeType on Linux/BSD, DirectWrite on Windows, CoreText on macOS). Lacks shaping/ligature support. **Not used in Zenterm** — Zenterm uses `cosmic-text` for full shaping + ligatures + emoji + font fallback from day one.

---

## D

### Damage Tracking
Recording which parts of the screen have changed since the last frame. Instead of redrawing everything, only redraw the changed cells. Alacritty tracks damage per-line and per-column range.

### DEC Private Mode
Vendor-specific terminal modes. The "DEC" prefix is historical (Digital Equipment Corporation). Examples:
- `\x1b[?1049h` — Enable alternate screen buffer
- `\x1b[?1000h` — Enable mouse tracking
- `\x1b[?25h` — Show cursor

---

## E

### eframe
egui's application framework. Handles window creation, event loop, OpenGL/wgpu context, and WASM support. You write `impl eframe::App for MyApp` and override `update()`.

### egui
An immediate-mode GUI library for Rust. Cross-platform (native + web). You rebuild the UI every frame in `update()`. Popular, mature, 30k+ GitHub stars. Maintained by emilk (also of Rerun).

### egui_dock
Community crate for tab/docking support in egui. 594 stars, 30+ contributors. Supports tab opening/closing/reordering, pane splitting, drag-to-window.

### etagere
Automatic rectangle packing library. Used for organizing glyph bitmaps in a GPU texture atlas. Given a set of rectangles (glyph dimensions), it finds an efficient arrangement in a larger rectangle (the atlas).

### epaint
egui's built-in 2D rendering library. Handles text, shapes, and images. **Not suitable for high-performance terminal text rendering** — designed for UI text, not per-cell terminal grids.

---

## F

### Font Fallback
When a font doesn't contain a glyph for a character (e.g., Chinese character in a Latin font), the system tries other fonts in order until it finds one that has it.

### Font Shaping (see Shaping)

---

## G

### Glyph
A specific visual representation of a character in a font. "A" and "a" are the same character but different glyphs. Ligatures like "fi" are a single glyph representing multiple characters.

### Glyph Atlas (see Texture Atlas)

### Grid
The 2D array of cells representing the terminal screen. Usually implemented as a ring buffer for efficient scrolling. Alacritty's implementation in `alacritty_terminal/src/grid/` is a textbook example.

---

## H

### HarfBuzz
The industry-standard text shaping engine (used by Chrome, Firefox, Android, etc.). Written in C++ with C bindings. `rustybuzz` is a pure Rust port. `cosmic-text` wraps it internally.

### Hot Reload
Re-reading configuration files without restarting the application. Alacritty uses `notify` crate for file watching.

---

## I

### Immediate Mode
A UI paradigm where the entire UI is rebuilt from scratch every frame. egui is immediate-mode. Contrast with retained mode (e.g., React, Flutter) where the UI is a persistent tree that's diffed. Pro: no state management, always consistent. Con: layout overhead every frame.

### Instanced Rendering
A GPU rendering technique where one draw call renders many copies of the same geometry (e.g., a quad) with per-instance attributes (position, color, texture coordinates). Essential for terminal performance — 1 draw call for all 1,920+ cells.

---

## K

### Kitty Keyboard Protocol
A modern keyboard protocol (`\x1b[=...u`) that encodes key events unambiguously. Distinguishes between `Ctrl+Shift+c` and `Ctrl+c`, which standard encodings cannot. Progressive enhancement — works if terminal and application both support it.

---

## L

### Ligature
Multiple characters rendered as a single typographic glyph. Examples: `fi`, `fl`, `->`, `=>`, `!=`. Requires HarfBuzz-level shaping. Alacritty chose not to support ligatures for performance.

### libghostty-vt
Ghostty's terminal emulation engine exposed as a C library. Available as a Rust crate via `libghostty-vt-sys` + `libghostty-vt`. **Requires Zig to build.** Officially supports Windows. New (v0.1.1, March 2026) — C API is still in flux.

---

## O

### OSC (Operating System Command)
Terminal escape sequences for higher-level operations, starting with `\x1b]`:
- `\x1b]0;Title\x1b\\` — Set window title
- `\x1b]9;Notification text\x1b\\` — Desktop notification (used by cmux)
- `\x1b]777;command...\x1b\\` — Custom notification protocols

---

## P

### portable-pty
Cross-platform PTY abstraction crate by the Wezterm team. Provides a single API for Unix and Windows PTYs. MIT license, pure Rust.

### PTY (Pseudo-Terminal)
An OS facility that creates a pair of file descriptors — one for your terminal app, one for the child process (shell). Data written to one end appears as input at the other end.

---

## R

### Ring Buffer (Circular Buffer)
A fixed-size buffer where new elements overwrite the oldest when full. Used for terminal scrollback — efficient O(1) scrolling without memory allocation per scroll.

### rustybuzz
A pure Rust port of HarfBuzz. Allows text shaping without any C library dependency.

---

## S

### Scrollback
Lines that have scrolled off the visible terminal screen, stored in memory for viewing by scrolling up. Typically 10,000+ lines.

### SGR (Select Graphic Rendition)
The subset of ANSI escape sequences that control text formatting: colors, bold, italic, underline, etc. Sequence format: `\x1b[N1;N2;...Nm`.

### Shaping
The process of converting a sequence of Unicode codepoints into positioned glyphs. Handles:
- Substitution: `f` + `i` → single `ﬁ` glyph
- Positioning: Kerning, mark positioning (accents)
- Line breaking: Where to wrap text
- BiDi reordering: Mixing LTR and RTL text

### swash
A pure Rust font loading, shaping, and rasterization library. Used internally by cosmic-text. Can be used standalone for lower-level control.

### Screen Buffer (see Grid)

### SGR Mouse Mode
Mouse tracking protocol enabled by `\x1b[?1006h` (DECSET 1006). Encodes mouse events as `\x1b[<row>;<col>;<btn>M` (press) / `m` (release). Superior to older X10 mode because it disambiguates button numbers and supports drag events. Required by vim, htop, mc, and most TUI applications.

---

## T

### Texture Atlas (Glyph Atlas)
A large GPU texture containing many smaller glyph images. Instead of uploading each glyph to the GPU as a separate texture (slow, many draw calls), glyphs are packed into one atlas texture, and the GPU uses UV coordinates to pick the right glyph. All cells can then be rendered in one draw call.

### True Color (24-bit Color)
Color support using 8 bits per channel (16.7 million colors). Terminal sequence: `\x1b[38;2;R;G;Bm` (foreground) and `\x1b[48;2;R;G;Bm` (background).

---

## V

### vte (Virtual Terminal Emulation)
The standard Rust crate for parsing ANSI/VT escape sequences. Implements Paul Williams' terminal parser state machine. Zero dependencies. Used by Alacritty, Kitty, and most Rust terminals.

### VSync (Vertical Synchronization)
Synchronizing frame rendering with the display's refresh cycle. Prevents screen tearing. Adds ~16.6ms (60Hz) or ~8.3ms (120Hz) of latency from GPU queue to visible output.

### vt100
A higher-level Rust crate built on `vte`. Provides complete terminal state (parser + screen grid + colors + scrollback) in one package. 7M+ downloads, pure Rust, MIT license. A black-box design — not used in Zenterm, which instead uses `vte` + alacritty's grid/term for full control.

### wezterm-font
Wezterm's font stack: wraps FreeType (rasterization), HarfBuzz (shaping + ligatures), and Cairo (rendering, Linux/macOS). Handles font discovery, shaping (ligatures, BiDi, emoji), rasterization, and font fallback with ranking. Not published on crates.io (`publish = false`); deeply coupled to wezterm's internal workspace crates. **Not used in Zenterm** — kept as a reference for font shaping patterns. Zenterm uses `cosmic-text` (pure Rust).

---

## W

### Workspace
A named grouping of terminal tabs.  Each workspace owns its own `DockState<SessionId>` (egui_dock layout tree), so different workspaces can have independent tab arrangements.  The session pool is shared — a `SessionId` is globally unique across all workspaces.  Managed by `WorkspaceManager` in `workspace.rs`.  Users can create, rename, switch, and close workspaces via the sidebar or keyboard shortcuts (`Ctrl+1..9`, `Ctrl+Tab`).

### wgpu
The standard Rust graphics API. Translates to Vulkan, Metal, DirectX 12, or WebGPU depending on platform. Cross-platform, safe, modern. Successor to OpenGL in the Rust ecosystem.

### WGSL (WebGPU Shading Language)
The shader language used by wgpu. Similar to Rust in syntax. Used to write vertex and fragment shaders for terminal grid rendering.

### Winit
Cross-platform window creation and event handling library. Underlies both eframe and most Rust GUI apps. Handles the actual OS window, input events, and rendering surface.
