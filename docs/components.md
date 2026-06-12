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

### Two Levels of Crates

#### Low-Level: `vte`

- Provides a state machine that breaks byte streams into "actions" (print char, execute control, dispatch CSI/OSC)
- You implement the `Perform` trait to handle each action
- Alacritty and most terminals use this as the foundation
- Requires you to maintain your own screen buffer

#### High-Level: `vt100` ⭐

- Wraps `vte` internally
- Maintains the complete screen state (grid of cells with colors/styles)
- One-function API: `parser.process(bytes)` → `parser.screen().cell(row, col)`
- Pure Rust, 7M+ downloads, 121+ reverse dependencies
- Also provides: `contents_diff()` for damage tracking, scrollback, cursor state

### Crate: `vt100`

```rust
use vt100::Parser;

let mut parser = Parser::new(24, 80, 10000);  // rows, cols, scrollback_lines

// Feed shell output
parser.process(b"\x1b[31mHello\x1b[0m");

// Read cell state
let screen = parser.screen();
let cell = screen.cell(0, 0).unwrap();
println!("{:?}", cell.character());  // 'H'
println!("{:?}", cell.fgcolor());    // Color::Idx(1) = red

// Damage tracking
let diff = screen.contents_diff(&previous_screen);
```

### What You Need to Do

- Create one `vt100::Parser` per terminal tab
- On each frame: feed PTY bytes to parser, read screen cells for rendering
- Handle resize: call `parser.set_size(rows, cols)` when window changes size

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

### Implementation Note

The `vt100` crate **already includes** a complete screen buffer. You don't need a separate grid crate. However, if you want to understand the design, reference Alacritty's implementation:

- `alacritty_terminal/src/grid/` — Superb ring buffer design
- `alacritty_terminal/src/grid/storage.rs` — The `Storage` struct (circular buffer, O(1) scroll)
- `alacritty_terminal/src/term/cell.rs` — Cell representation

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

### Crate: `cosmic-text`

- **Author:** System76 (POP!_OS team)
- **Pure Rust:** Yes
- **Internally uses:** `swash` (rasterization) + `rustybuzz` (HarfBuzz shaping) + custom layout
- **Features:** Ligatures ✓, BiDi ✓, Emoji ✓, Font fallback ✓

```rust
use cosmic_text::{
    FontSystem, SwashCache, Attrs, Family, Buffer, Metrics, Shaping,
};

let mut font_system = FontSystem::new();
let mut swash_cache = SwashCache::new();

let mut buffer = Buffer::new(&mut font_system, Metrics::new(14.0, 20.0));
buffer.set_size(&mut font_system, 800.0, 600.0);
buffer.set_text(
    &mut font_system,
    "Hello -> World",
    Attrs::new().family(Family::Monospace),
    Shaping::Advanced,  // ← Advanced = HarfBuzz shaping with ligatures
);

// Buffer now contains positioned glyphs ready for rendering
```

### What You Need to Do

- **Don't** use cosmic-text for full line layout/word-wrap (too slow for terminal)
- **Do** use it per-character-cell: for each `Cell.character`, get the shaped glyph + position
- Cache the shaping results (the character set in a terminal session is usually small)
- Handle font fallback for missing glyphs (e.g., emoji, CJK)

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
   - Rasterize it via cosmic-text/swash → pixel buffer
   - Find empty space in atlas via etagere (`Allocator::allocate()`)
   - Upload pixel buffer to GPU texture at allocated position
   - Store the UV coordinates in a HashMap for fast lookup
2. On render: look up each cell's glyph UV, build instanced quad data

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

---

## Summary: Crate Dependency Graph

```
zenmux (your app)
├── eframe / egui / egui_dock     (UI framework)
├── egui-wgpu                      (wgpu backend)
├── wgpu                           (GPU API)
├── vt100                          (terminal emulation)
├── cosmic-text                    (font shaping, ligatures)
├── swash                          (glyph rasterization)
├── etagere                        (texture atlas packing)
├── portable-pty                   (cross-platform PTY)
├── serde + toml                   (config)
└── parking_lot                    (concurrency)
```

All crates are **pure Rust**, all available on crates.io, all with permissive open-source licenses (MIT/Apache 2.0).
