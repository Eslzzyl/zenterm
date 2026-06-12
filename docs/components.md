# Component Reference

This document explains every component of a terminal emulator — what it does, why it's needed, and which Rust crates implement it.

---

## 1. PTY (Pseudo-Terminal)

### What it is

A PTY is an operating system-provided "virtual serial cable" that connects your terminal application to a shell process (bash, zsh, pwsh). It has two ends:

```
Your App  ◄──pty master──►  Shell (bash/zsh)
    │                          │
    │  write("ls\n")           │
    ├─────────────────────────►│
    │                          ├── fork/exec ls
    │  read("src/\n")          │
    │◄─────────────────────────┤
```

The shell thinks it's talking to a real terminal. In reality, it's talking to your application.

### Platform Differences

| Platform | API | Complexity |
|----------|-----|------------|
| Linux | `posix_openpt()`, `grantpt()`, `unlockpt()` | Medium |
| macOS | Same as Linux, minor differences | Medium |
| Windows | ConPTY (`CreatePseudoConsole`, `OpenConPTY`) | High (COM API, pipes) |

### Crate: `portable-pty`

- **Author:** Wezterm team
- **License:** MIT
- **Pure Rust:** Yes
- **Usage:**
```rust
use portable_pty::{PtySize, native_pty_system, CommandBuilder};

let pty_system = native_pty_system();
let pair = pty_system.openpty(PtySize::new(24, 80, 0, 0))?;
let child = pair.slave.spawn_command(CommandBuilder::new("bash"))?;

// Read from shell
let mut reader = pair.master.try_clone_reader()?;
// Write to shell
pair.master.write(b"ls\n")?;
```

### What You Need to Do

- Wrap PTY read in a background thread (non-blocking I/O)
- Send PTY output bytes to the VT parser via channel
- Forward keyboard input bytes to PTY write

---

## 2. VT Parser (Virtual Terminal Parser)

### What it is

Shells output a mix of **text** and **control sequences**. Control sequences (starting with `ESC [`, i.e. `\x1b[`) tell the terminal to change colors, move cursor, clear screen, etc.

```
Raw shell output:  "Hello \x1b[31mRed\x1b[0m text"
                            │         │
                            │         └── Reset all styles
                            └── Set foreground to red
```

The VT parser reads the byte stream and updates the **screen state** accordingly.

### Crate: `vte` (low-level parser)

- Provides a state machine that breaks byte streams into "actions" (print char, execute control, dispatch CSI/OSC)
- You implement the `Perform` trait to handle each action
- Alacritty uses this as the foundation

```rust
use vte::Parser;
use vte::Perform;

struct MyPerform;

impl Perform for MyPerform {
    fn print(&mut self, c: char) {
        // A printable character was received — add it to the grid
        terminal.grid.current_cell_mut().character = c;
    }
    fn execute(&mut self, byte: u8) {
        // A control character was received (\\n, \\r, \\t, \\x07 BEL, etc.)
        match byte {
            b'\\n' => terminal.grid.new_line(),
            b'\\r' => terminal.grid.carriage_return(),
            // ...
        }
    }
    fn csi_dispatch(&mut self, params: &[i64], intermediates: &[u8], ignore: bool, action: u8) {
        // A CSI sequence was received (e.g., \\x1b[31m → set color)
        match action {
            b'm' => { /* SGR — set graphics rendition */ }
            b'A' => { /* CUU — cursor up */ }
            // ...
        }
    }
    // Also: osc_dispatch, esc_dispatch, hook/put/unhook, ...
}

let mut parser = Parser::new();
parser.advance(&mut performer, b"\\x1b[31mHello");
```

### Screen State: `alacritty_terminal::grid` + `alacritty_terminal::term`

The `vte` parser generates **actions**; your `Perform` impl applies those actions to a **screen buffer** (grid).

We use alacritty's grid, term, and selection modules from the vendored `alacritty/` directory:

| Alacritty Module | Path | What It Provides |
|------------------|------|------------------|
| `grid/` | `alacritty/alacritty_terminal/src/grid/` | Ring buffer grid with O(1) scrolling, row storage, resize |
| `term/` | `alacritty/alacritty_terminal/src/term/` | Terminal state: cursor, colors, modes, selection, alternate screen |
| `selection/` | `alacritty/alacritty_terminal/src/selection/` | Text selection (click-drag, double/triple click) |
| `index/` | `alacritty/alacritty_terminal/src/index/` | Row/Col/Line coordinate types |

This gives us:
- **Full control** over damage tracking (per-row dirty flags, column ranges)
- **Battle-tested** ring buffer (used by Alacritty for years)
- **Customizability** for future features (Kitty graphics protocol, OSC 8 hyperlinks, sixel)
- **Modifiability** — we own the code, we can change anything

### What You Need to Do

- Integrate `vte::Parser` with `alacritty_terminal::term::Term` via a `Perform` bridge
- On each frame: feed PTY bytes to the parser, read grid cells for rendering
- Handle resize: update `Term::resize()` when window changes size

---

## 3. Screen Buffer (Grid)

### What it is

The terminal screen is conceptually a 2D grid of **cells**. Each cell stores:

```
Cell {
    character: char,          // The displayed character
    fgcolor: Color,           // Foreground color
    bgcolor: Color,           // Background color
    bold: bool,               // Bold attribute
    italic: bool,             // Italic attribute
    underline: bool,          // Underline attribute
    strikethrough: bool,      // Strikethrough
    inverse: bool,            // Reverse video
    hidden: bool,             // Hidden text
    // ... plus wide-char flags for CJK
}
```

Above the visible viewport is the **scrollback buffer** — lines that scrolled off the top, stored in memory for scrolling up.

### Implementation

We use **alacritty's grid implementation** directly:

- `alacritty_terminal/src/grid/` — Ring buffer design with O(1) scroll
- `alacritty_terminal/src/grid/storage.rs` — The `Storage` struct (circular buffer)
- `alacritty_terminal/src/term/cell.rs` — Cell representation

This gives full control over damage tracking, selection rendering, and future custom features.

---

## 4. Font Shaping (Text Shaping)

### What it is

Converting a sequence of Unicode characters into positioned **glyphs** (visual shapes). This is surprisingly complex:

```
Input chars:         f  i    →    (ligature fi)
                     →  =    →    (ligature →)
                     A  B    →    A B (no ligature, 2 glyphs)
```

Shaping handles:
- **Ligatures** — `fi`, `fl`, `->`, `=>` become single glyphs
- **Complex scripts** — Arabic, Devanagari (characters change shape based on context)
- **Emoji sequences** — `👨‍💻` is multiple Unicode codepoints rendered as one glyph
- **Font fallback** — If the current font doesn't have a glyph, try another font

### Crate: `cosmic-text` ⭐

- **Author:** System76 (jackpot51, hecrj, and team)
- **License:** MIT / Apache 2.0
- **Pure Rust:** Yes — zero C dependencies
- **Internal stack:** `rustybuzz` (pure Rust HarfBuzz) for shaping + `swash` for rasterization + `fontdb` for font discovery
- **Features:** Full shaping (ligatures), BiDi, emoji sequences, font fallback with Chromium/Firefox priority lists
- **Published:** `cosmic-text` on crates.io (v0.19.0, 5M+ downloads, 135 dependents)

```rust
use cosmic_text::{FontSystem, SwashCache, Buffer, Metrics, Shaping, Attrs};

// One per application (font discovery + caching)
let mut font_system = FontSystem::new();
let mut swash_cache = SwashCache::new();

// Shape a line of text for a terminal row
let mut buffer = Buffer::new(&mut font_system, Metrics::new(14.0, 20.0));
buffer.set_text("ls -> file.rs", &Attrs::new(), Shaping::Advanced);
buffer.shape_until_scroll(&mut font_system, true);

// Extract positioned glyphs with ligature support
for run in buffer.layout_runs() {
    for glyph in &run.glyphs {
        // glyph.cache_key — lookup in SwashCache for rasterization
        // glyph.x, glyph.y — pixel positions (accounting for ligature widths)
        let image = swash_cache.get(&mut font_system, glyph.cache_key);
        // → Pack image.data into etagere atlas
    }
}
```

`cosmic-text` handles the full pipeline: font discovery → shaping (with ligatures) → rasterization. Each cell's character is shaped into positioned glyphs, then rasterized into pixel buffers for atlas upload.

For terminal rendering, we use `cosmic-text` at the **shaping + rasterization layer only** — we manage atlas packing and GPU instanced rendering ourselves for optimal performance.

**Not chosen:** `wezterm-font` (deeply coupled to wezterm internal crates, not on crates.io, C dependencies); `crossfont` (Alacritty's choice, no shaping/ligatures).

### What You Need to Do

- Initialize `FontSystem` at startup (font discovery + caching)
- For each unique character+style pair, or per-row shaping run:
  1. Shape via cosmic-text → get glyph indices + positions
  2. Rasterize via SwashCache → get pixel buffer
  3. Pack into GPU atlas via etagere
  4. Cache UV coordinates for fast lookup
- For cells with ligatures (e.g., `->`, `fi`), the shaped glyph may span multiple cells — handle via cell clustering

---

## 5. Glyph Atlas (Texture Atlas)

### What it is

A GPU texture that stores all the glyphs (character shapes) currently in use. Think of it as a "warehouse" on the GPU.

```
GPU Texture (e.g., 2048×2048 pixels)
┌────┬────┬────┬────┬────┬────┬────┬────┐
│ a  │ b  │ c  │ d  │ e  │ f  │ g  │ h  │  ← Each glyph rasterized once
├────┼────┼────┼────┼────┼────┼────┼────┤
│ i  │ j  │ k  │ l  │ m  │ n  │ o  │ p  │    Uploaded when first needed
├────┼────┼────┼────┼────┼────┼────┼────┤
│ q  │ r  │ s  │ t  │ u  │ v  │ w  │ x  │    Each character stored at a
├────┼────┼────┼────┼────┼────┼────┼────┤    specific (u, v) coordinate
│ y  │ z  │ →  │ fi │ 0  │ 1  │ 2  │ 3  │
├────┼────┼────┼────┼────┼────┼────┼────┤
│ │  │   │   │   │   │   │   │   │      │
└────┴────┴────┴────┴────┴────┴────┴────┘
```

**Why:** GPU can draw thousands of quads from one texture in a single draw call. Without an atlas, you'd need separate textures per glyph — thousands of draw calls, very slow.

### Crates

| Crate | Description |
|-------|-------------|
| **`etagere`** | Automatic rectangle packing for atlas. Pure Rust. Recommended. |
| `guillotiere` | Similar to etagere, more battle-tested. |
| **DIY** | Simple row-based packing is ~200 lines. See Alacritty's `atlas.rs`. |

### What You Need to Do

 1. On first encounter of a glyph:
     - Rasterize it via cosmic-text SwashCache → pixel buffer    - Find empty space in atlas via etagere (`Allocator::allocate()`)
    - Upload pixel buffer to GPU texture at allocated position
    - Store the UV coordinates in a HashMap for fast lookup2. On render: look up each cell's glyph UV, build instanced quad data

---

## 6. GPU Rendering (wgpu)

### What it is

wgpu is the Rust standard for cross-platform GPU programming. It translates to:

| Platform | Backend |
|----------|---------|
| macOS | Metal |
| Windows | DirectX 12 |
| Linux | Vulkan |
| Web | WebGPU / WebGL |

### How Terminal Rendering Works

Forget about 3D — terminal rendering is fundamentally a **2D sprite batching** problem:

1. Each terminal cell = one quad (rectangle) on screen
2. Each quad has: position (x,y), size (w,h), and UV coordinates into the glyph atlas
3. Render all 1920+ quads in **one draw call** using instanced rendering

### Pipeline (WGSL shader)

```wgsl
// Vertex shader — per-instance
struct CellInstance {
    position: vec2<f32>,   // Where on screen
    glyph_uv: vec2<f32>,   // Where in the atlas
    fg_color: vec4<f32>,   // Text color
    bg_color: vec4<f32>,   // Background color
};

// One draw call for the entire terminal grid
// Each instance = one character cell
```

### Integration with egui

Don't fight egui's renderer. Use its escape hatch:

```rust
use egui_wgpu::CallbackTrait;

struct TerminalRenderPass {
    pipeline: wgpu::RenderPipeline,
    atlas: wgpu::Texture,
    // ...
}

impl CallbackTrait for TerminalRenderPass {
    fn paint(
        &self,
        info: egui_wgpu::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &egui_wgpu::CallbackResources,
    ) {
        // Your wgpu draw commands here
        // Runs inside egui's render pass
        render_pass.set_pipeline(&self.pipeline);
        render_pass.draw_indexed(0..6, 0, 0..cell_count);
    }
}

// In egui update():
ui.painter().add(egui_wgpu::Callback::new().paint(terminal_pass));
```

---

## 7. UI Chrome (egui)

### What egui is NOT doing

egui will **NOT** render the terminal text. That's what the custom wgpu pipeline is for.

### What egui IS doing

- **Window management** — Opening, closing, resizing, minimizing
- **Tab bar** — Switching between terminal sessions
- **Workspace sidebar** — Vertical tab list (like cmux), showing git branch, directory, ports
- **Notifications** — Popover badges when an agent needs attention
- **Command palette** — Fuzzy-find actions
- **Settings panel** — Font size, theme, keybindings
- **Status bar** — Session name, shell type, cursor position
- **Context menus** — Right-click actions

Everything in this list is standard egui widget work — buttons, labels, text inputs, combo boxes. The performance cost is ~0.1-0.3ms per frame.

### Recommended Crates

| Crate | Purpose |
|-------|---------|
| `egui` + `eframe` | Core UI framework |
| `egui_dock` | Tab/docking system |
| `egui-wgpu` | wgpu backend + CallbackTrait |
| `egui_tiles` | Alternative tiling layout |

---

## 8. Input Handling

### What it is

Converting keyboard events into escape sequences that the shell understands:

```
User presses:       Encoded as:
─────────────────────────────────
'j'                 0x6A ('j')
Enter               0x0D ('\r')
Ctrl+C              0x03
↑                   \x1b[A
Alt+F               \x1bf
Ctrl+Shift+C        \x1b[99;6u  (kitty protocol)
```

### Reference Implementation

**Alacritty's `input.rs`** (`alacritty/src/input.rs`) is the best reference. It handles:
- Standard key encoding (ASCII, Ctrl, Alt, Meta)
- Application cursor keys (terminal mode-dependent)
- Kitty keyboard protocol (progressive enhancement)
- Mouse event encoding (SGR, UTF-8, X10 modes)
- Bracketed paste mode

### What You Need to Do

- Map egui's `KeyEvent` → terminal escape sequences
- Respect terminal mode flags (e.g., application cursor keys)
- Support at minimum: standard encoding + kitty protocol
- Mouse: SGR encoding (mode `?1006`)
- Handle selection mode vs. mouse-report mode routing

### Mouse Input

Mouse handling has two modes, determined by the shell application:

**Selection mode (default):**
- Left click-drag: select text in terminal grid
- Double-click: select word
- Triple-click: select line
- Right-click: context menu
- Scroll: scrollback navigation
- These are handled locally with egui's `Sense::click_and_drag()` and `PointerState`

**Mouse report mode (vim/htop/nano):**
- When terminal enables `DECSET 1000`/`1006`, all mouse events become SGR escape sequences
- Encoded as `\x1b[<row>;<col>;<btn>M` (press) / `m` (release)
- Forwarded directly to PTY via `self.pty_writer.write()`
- egui's `PointerState` gives same-frame mouse data — no latency penalty

```rust
// Inside App::update()
let pointer = ui.input(|i| i.pointer.clone());
if term_mode.contains(MOUSE_REPORT) {
    if let Some(btn) = pointer.press_origin() {
        let seq = encode_sgr_mouse(row, col, btn, true);
        pty.write(seq.as_bytes());
    }
    if let Some(_) = &pointer.any_released() {
        let seq = encode_sgr_mouse(row, col, 0, false);
        pty.write(seq.as_bytes());
    }
} else {
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    if resp.dragged() { selection.update(row, col); }
    if resp.double_clicked() { selection.select_word(row, col); }
}
```

---

## Summary: Crate Dependency Graph

```
zenterm (your app)
├── eframe / egui / egui_dock     (UI framework)
├── egui-wgpu                      (wgpu backend + CallbackTrait)
├── wgpu                           (GPU API)
├── vte                            (low-level VT state machine)
├── alacritty_terminal             (grid, term, selection — from alacritty/)
│   └── vte (already a dep)
├── cosmic-text                    (font shaping + rasterization + ligatures + emoji)
│   └── (pure Rust: rustybuzz + swash + fontdb)
├── etagere                        (texture atlas packing)
├── portable-pty                   (cross-platform PTY)
├── copypasta                      (clipboard)
├── linkify                        (URL detection, Phase 3)
├── serde + toml                   (config, Phase 3)
├── parking_lot                    (concurrency)
│
│   Future phases:
├── wezterm-toast-notification     (native notifications, Phase 2)
└── wezterm-font / crossfont       (reference only — not used)
```

All crates have permissive open-source licenses (MIT/Apache 2.0).
