# Zenterm Configuration Reference

> **Config file path:** `~/.config/zenterm/config.toml`
> **Override:** Set the `ZENTERM_CONFIG` environment variable
> **Hot-reload:** `Ctrl+Shift+R` reloads the file at runtime
>
> Every section and field is optional. Missing values use the defaults
> documented below, which mirror the original hardcoded behaviour.

---

## `[window]` — Window settings

Controls the appearance and initial size of the terminal window.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `dimensions` | `{ columns, lines }` | `{ columns = 80, lines = 24 }` | Initial terminal size in cells (not pixels). The window is sized to fit this grid at the configured font size. |
| `padding` | `{ x, y }` | `{ x = 12, y = 10 }` | Inner padding between the window edge and the terminal grid, in logical pixels at 1× DPI. |
| `title` | `string` | `"Zenterm"` | Window title. The terminal can override this via OSC 0 / OSC 2 escape sequences. |
| `decorations` | `bool` | `true` | Show window decorations (title bar + borders). |

### Example

```toml
[window]
dimensions = { columns = 120, lines = 40 }
padding = { x = 4, y = 4 }
title = "Terminal"
decorations = true
```

---

## `[terminal]` — Terminal buffer

Controls how many lines of terminal output remain available when scrolling
back through the main screen.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `scrollback_lines` | `integer` | `10000` | Maximum retained history lines. `0` disables scrollback history. Changes apply immediately to existing sessions. |

### Example

```toml
[terminal]
scrollback_lines = 5000
```

---

## `[background]` — Background image

Show an image behind the terminal content.  The image is rendered through the
GPU pipeline and appears behind all cell instances (selection, cursor,
highlighted text are drawn on top of it).

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `image_path` | `string` | — | Path to an image file. Supports any format the `image` crate can decode (PNG, JPEG, GIF, WebP, BMP). Empty or absent = no background image. |
| `image_opacity` | `float` | `0.8` | Opacity of the image (`0.0` = fully transparent / theme colour only, `1.0` = image fully replaces the theme background). |
| `image_mode` | `string` | `"Cover"` | How the image fits the terminal area. One of: `"Cover"`, `"Contain"`, `"Stretch"`, `"Center"`. |

### `image_mode` values

| Value | Behaviour |
|-------|-----------|
| `Cover` | Scale the image to fill the entire viewport, cropping the longer dimension to preserve the aspect ratio. |
| `Contain` | Scale the image to fit within the viewport, letterboxing (adding empty bands) when the aspect ratios differ. |
| `Stretch` | Stretch the image to fill the entire viewport, ignoring the aspect ratio. |
| `Center` | Center the image at its native pixel size. Larger images are cropped; smaller images show the theme background colour around them. |

### Example

```toml
[background]
image_path = "~/Pictures/wallpaper.png"
image_opacity = 0.5
image_mode = "Cover"
```

---

## `[font]` — Font settings

Configure the typeface and rendering.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `size` | `float` | `18.0` | Font size in **logical pixels at 1× DPI**. On a standard display a value of `18.0` produces an 18 px font; on a 2× Retina display it produces 36 px. Roughly equivalent to 13.5 pt at 96 DPI. |
| `normal` | `{ family, style? }` | platform-dependent¹ | The regular (normal-weight) font face. |
| `ligatures` | `bool` | `true` | Enable OpenType ligature features (`liga`, `clig`). |
| `hinting` | `string` | `"Auto"` | Font hinting mode. One of: `"None"` (smoothest), `"Auto"` (low-DPI only), `"Full"` (sharpest). |
| `render_mode` | `string` | `"Grayscale"` | Anti-aliasing mode. One of: `"Grayscale"` (stable in a composited GPU surface), `"Subpixel"` (explicit LCD RGB subpixel mode). |

> ① **Default font family by platform:**
> - **macOS:** `"Menlo"`
> - **Windows:** `"Consolas"`
> - **Linux:** `"monospace"` (resolved via fontconfig)

### `FontDescription` `{ family, style? }`

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `family` | `string` | ✅ | Font family name (e.g. `"JetBrains Mono"`, `"Fira Code"`, `"Menlo"`). |
| `style` | `string` | ❌ | Font style name (e.g. `"Regular"`, `"Bold"`, `"Italic"`). Cosmetic / metadata only — cosmic-text resolves weight and style automatically from the font file. |

### Example

```toml
[font]
size = 14.0
normal = { family = "JetBrains Mono", style = "Regular" }
ligatures = true
hinting = "Auto"
render_mode = "Subpixel"
```

---

## `[colors]` — Colour theme

Controls all colours used by the terminal.

The Settings → Colors page includes live terminal/UI previews and four
one-click theme families: Default, Nord, Forest, and Sepia. Each family has
light and dark variants. Individual colour fields below remain available for
custom overrides.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `appearance` | `string` | `"System"` | Appearance mode. One of: `"Dark"`, `"Light"`, `"System"`. `"System"` follows the OS dark/light mode setting. |
| `palette` | `string` | `"Default"` | Colour theme family. One of: `"Default"`, `"Nord"`, `"Forest"`, `"Sepia"`. The selected appearance mode chooses its light or dark variant. |
| `primary` | `{ ... }` | — | Core foreground and background colours (see below). |
| `cursor` | `{ ... }` | — | Cursor colours. |
| `selection` | `{ ... }` | — | Selection highlight colours. |
| `normal` | `{ ... }` | — | 8 normal (dark) ANSI colours. |
| `bright` | `{ ... }` | — | 8 bright ANSI colours. |

All colour values are hex strings in `"#rrggbb"` or `"#rgb"` format.
Setting a colour to `"CellBackground"` or `"CellForeground"` (for cursor colours)
uses the cell's own background/foreground colour (inverse video).

### `[colors.primary]`

| Key | Default (Dark) | Default (Light) | Description |
|-----|---------------|-----------------|-------------|
| `foreground` | `"#d8dee9"` | `"#27313b"` | Default text colour. |
| `background` | `"#111318"` | `"#fbfaf7"` | Default background colour. |
| `dim_foreground` | `"#8e97a6"` | `"#6f7882"` | Text colour for the "dim" (half-intensity) SGR attribute. |
| `bright_foreground` | `"#ffffff"` | `"#192028"` | Text colour used when bold text is displayed. |

### `[colors.cursor]`

| Key | Default (Dark) | Default (Light) | Description |
|-----|----------------|-----------------|-------------|
| `text` | `"#111318"` | `"#fbfaf7"` | Colour of the text under the cursor. |
| `cursor` | `"#a78bfa"` | `"#6657b3"` | Colour of the cursor cell itself. |

### `[colors.selection]`

| Key | Default (Dark) | Default (Light) | Description |
|-----|---------------|-----------------|-------------|
| `foreground` | `"#f5f3ff"` | `"#27313b"` | Foreground colour of selected text. |
| `background` | `"#393052"` | `"#ddd8f3"` | Background colour of selected text. |

### `[colors.normal]` / `[colors.bright]`

Both sections accept the same 8 colour keys. Below are the built-in defaults.

**Dark theme:**

| Key | Normal | Bright |
|-----|--------|--------|
| `black` | `"#1b1f27"` | `"#5c6370"` |
| `red` | `"#e06c75"` | `"#ff7b86"` |
| `green` | `"#98c379"` | `"#b5e890"` |
| `yellow` | `"#e5c07b"` | `"#ffd58a"` |
| `blue` | `"#61afef"` | `"#7cc7ff"` |
| `magenta` | `"#c678dd"` | `"#e29aff"` |
| `cyan` | `"#56b6c2"` | `"#73e0e8"` |
| `white` | `"#abb2bf"` | `"#ffffff"` |

**Light theme:**

| Key | Normal | Bright |
|-----|--------|--------|
| `black` | `"#2e343d"` | `"#69707b"` |
| `red` | `"#b44347"` | `"#d1575b"` |
| `green` | `"#3f7e46"` | `"#539b5b"` |
| `yellow` | `"#91691b"` | `"#ab8028"` |
| `blue` | `"#355c9a"` | `"#4874b9"` |
| `magenta` | `"#7f488b"` | `"#9758a6"` |
| `cyan` | `"#267780"` | `"#30919b"` |
| `white` | `"#717982"` | `"#27313b"` |

### Complete colour example

```toml
[colors]
appearance = "Dark"
palette = "Nord"

[colors.primary]
foreground = "#d8d8d8"
background = "#181818"

[colors.cursor]
text = "CellBackground"
cursor = "CellForeground"

[colors.selection]
foreground = "#ffffff"
background = "#333333"

[colors.normal]
black   = "#0f0f0f"
red     = "#712b2b"
green   = "#5f6f3a"
yellow  = "#a17e4d"
blue    = "#456877"
magenta = "#704d68"
cyan    = "#4d7770"
white   = "#8e8e8e"

[colors.bright]
black   = "#1c1c1c"
red     = "#ac4242"
green   = "#8f9a6f"
yellow  = "#d3b36b"
blue    = "#7b9aa3"
magenta = "#a27e99"
cyan    = "#7ba39c"
white   = "#b3b3b3"
```

---

## `[cursor]` — Cursor settings

Controls the appearance and animation of the terminal cursor.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `style` | `{ shape, blinking? }` | `{ shape = "Block", blinking = "Off" }` | Cursor appearance and blink mode. |
| `unfocused_hollow` | `bool` | `true` | Show a hollow (`□`) cursor when the terminal window is not focused. |
| `thickness` | `float` | `0.15` | Thickness of the Underline or Beam cursor, as a fraction of cell height (`0.0`–`1.0`). |
| `blink_interval` | `int` | `30` | Number of frames between cursor blink toggles. At 60 FPS, `30` ≈ 500 ms. |
| `blink_timeout` | `int` | `5` | Time in seconds after which blinking stops (`0` = blink forever). |

### `CursorStyle` `{ shape, blinking? }`

**`shape`:** One of:
- `"Block"` — Solid rectangular block (default).
- `"Beam"` — Vertical bar at the left side of the cell.
- `"Underline"` — Horizontal bar at the bottom of the cell.

**`blinking`:** One of:
- `"Off"` — Never blink (default).
- `"On"` — Always blink, ignoring terminal escape sequences.
- `"Terminal"` — Follow the terminal's cursor-blinking escape sequence.

### Example

```toml
[cursor]
style = { shape = "Block", blinking = "Off" }
unfocused_hollow = true
thickness = 0.15
blink_interval = 30
blink_timeout = 5
```

---

## `[selection]` — Selection behaviour

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `save_to_clipboard` | `bool` | `false` | Automatically copy selected text to the system clipboard. |

```toml
[selection]
save_to_clipboard = true
```

---

## `[ui]` — UI chrome (tabs + sidebar)

Controls the optional **multi-tab workspace** and **cmux-style
sidebar**.  Both features are **opt-in**: with the default config
(`tabs_enabled = false`, `sidebar_enabled = false`), the application
behaves exactly like a single-terminal emulator (Phase 1).

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `tabs_enabled` | `bool` | `false` | Render the `egui_dock` tab bar.  When `false`, the app runs a single terminal session with no tab UI. |
| `sidebar_enabled` | `bool` | `false` | Render the workspace sidebar (vertical tab list with cwd).  Has no effect when `tabs_enabled = false`. |
| `sidebar_position` | `string` | `"Left"` | One of `"Left"` or `"Right"`. |
| `sidebar_width` | `float` | `220.0` | Default sidebar width in logical pixels. |
| `sidebar_min_width` | `float` | `160.0` | Minimum sidebar width (user-resize clamp). |
| `sidebar_max_width` | `float` | `480.0` | Maximum sidebar width (user-resize clamp). |
| `show_add_tab_button` | `bool` | `true` | Show the `+` button on the tab bar. |
| `show_close_tab_button` | `bool` | `true` | Show a `×` close button on each tab. |
| `restore_layout_on_startup` | `bool` | `true` | Restore the dock layout from `~/.config/zenterm/dock.json` on startup when present. |
| `persist_layout` | `bool` | `true` | Persist dock layout / session metadata to disk as the user mutates them. |
| `layout_debounce_ms` | `int` | `500` | Debounce window (milliseconds) between a layout mutation and the disk write. |

### Layout persistence

When `restore_layout_on_startup = true` and the file
`~/.config/zenterm/dock.json` exists, Zenterm reads it on startup
and restores the previous dock tree.  Per-session metadata
(title, working directory) is read from `sessions.json` in the
same directory.

Both files are written atomically (write to `*.tmp` then `rename`)
so a crash mid-write cannot leave a half-written file.  The
write is debounced by `layout_debounce_ms` to avoid hammering
the disk on every drag.  On clean exit (`App::on_exit`) the
layout is forced to disk.

> **Override the directory:** the persistence directory follows
> the active `config.toml` path (i.e. it respects the
> `ZENTERM_CONFIG` environment variable and the `~/.config/zenterm/`
> default).

### Example

```toml
[ui]
tabs_enabled = true
sidebar_enabled = true
sidebar_position = "Left"
sidebar_width = 240.0
show_add_tab_button = true
show_close_tab_button = true
restore_layout_on_startup = true
persist_layout = true
layout_debounce_ms = 500
```

---

## Built-in keyboard shortcuts

These shortcuts are currently hardcoded and not yet overridable via config:

| Shortcut | Action |
|----------|--------|
| `Ctrl+Shift+C` | Copy selected text to clipboard |
| `Ctrl+Shift+V` | Paste from clipboard |
| `Ctrl+Shift+P` (`Cmd+Shift+P` on macOS) | Open the command palette |
| `Ctrl+Shift+R` | **Hot-reload** config file from disk |

---

## Example: Full config file

```toml
[window]
dimensions = { columns = 120, lines = 40 }
padding = { x = 4, y = 4 }
title = "Zenterm"
decorations = true

[terminal]
scrollback_lines = 10000

[font]
size = 14.0
normal = { family = "JetBrains Mono" }

[colors]
appearance = "Dark"
palette = "Default"

[colors.primary]
foreground = "#d8d8d8"
background = "#181818"

[colors.normal]
black   = "#0f0f0f"
red     = "#712b2b"
green   = "#5f6f3a"
yellow  = "#a17e4d"
blue    = "#456877"
magenta = "#704d68"
cyan    = "#4d7770"
white   = "#8e8e8e"

[colors.bright]
black   = "#1c1c1c"
red     = "#ac4242"
green   = "#8f9a6f"
yellow  = "#d3b36b"
blue    = "#7b9aa3"
magenta = "#a27e99"
cyan    = "#7ba39c"
white   = "#b3b3b3"

[cursor]
style = { shape = "Block", blinking = "Off" }
unfocused_hollow = true
blink_interval = 30
blink_timeout = 5

[selection]
save_to_clipboard = false

[ui]
tabs_enabled = true
sidebar_enabled = true
```

---

## Keyboard shortcuts

Built-in shortcuts that work when `tabs_enabled = true`:

| Shortcut | Action |
|----------|--------|
| `Ctrl+Shift+C` | Copy selection to clipboard |
| `Ctrl+Shift+V` | Paste from clipboard |
| `Ctrl+Shift+P` (`Cmd+Shift+P` on macOS) | Open the command palette |
| `Ctrl+Shift+R` | Reload config file |
| `Ctrl+1` .. `Ctrl+9` | Switch to workspace by index |
| `Ctrl+Tab` | Cycle to next workspace |
| `Ctrl+Shift+Tab` | Cycle to previous workspace |

Sidebar-only shortcuts:

| Action | How |
|--------|-----|
| Switch workspace | Click workspace name in sidebar |
| Rename workspace | Double-click workspace name, or right-click → "Rename..." |
| Close workspace | Right-click workspace → "Close workspace" |
| New tab in workspace | Right-click workspace → "New Tab", or click "+ New shell" |
| New workspace | Click "+ New WS" |
| Switch to tab | Click tab name in sidebar |

---

## Behavioural notes

| Scenario | Behaviour |
|----------|-----------|
| **File not found** | Starts with full defaults. Logs `info` level message. |
| **File exists but invalid TOML** | Logs `error` with parse details. Falls back to full defaults at startup. On hot-reload, keeps the old config and shows an error banner. |
| **Unknown TOML keys** | Silently ignored (serde `deny_unknown_fields` is not set). |
| **Empty file** | Same as "file not found" — all defaults. |
| **Field type mismatch** | TOML parse error → logged + fallback/defaults. |
| **Window setting change (hot-reload)** | Some window settings (`dimensions`, `decorations`, `title`) require a restart. Font, colours, and cursor changes apply immediately. |
