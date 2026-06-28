# Ligature Support

> **Status:** Working for fonts that use OpenType `calt`/`liga`/`clig` features.
> Both true ligatures (multiple chars → one glyph) and contextual alternates
> (same glyph count, different glyphs, e.g. JetBrainsMono's `->`) are supported.

---

## Architecture Overview

Ligature handling involves two stages:

1. **GlyphAtlas** (`crates/zenterm-glyph/src/lib.rs`) — shapes a whole run of
   text as one unit so OpenType features can match across character boundaries.
2. **Session rendering** (`crates/zenterm-ui/src/session.rs`) — detects eligible
   runs, calls the atlas, and distributes the resulting glyphs across their
   covering terminal cells.

---

## Stage 1: Run Shaping (`GlyphAtlas::shape_and_rasterize_run`)

### Buffer setup

A `cosmic-text::Buffer` is created with `Wrap::None` so that ligature words are
not split into separate visual lines during layout.  When ligatures are enabled,
`FontFeatures` for `liga`, `clig`, `calt`, `dlig`, and `kern` are passed to
harfbuzz:

```rust
let mut buf = Buffer::new(&mut self.font_system, self.metrics);
buf.set_size(Some(self.font_size), None);
buf.set_wrap(Wrap::None);
```

The full run text is shaped as a single harfbuzz call, so OpenType context
rules (e.g. `calt` for `->`, `>=`) have access to all characters in the run.

### Output: `Vec<ShapedGlyph>`

Each glyph in the shaped output becomes a `ShapedGlyph`:

| Field | Meaning |
|-------|---------|
| `char_range` | Byte range in the source text this glyph covers |
| `num_cells` | Number of terminal cells this glyph occupies (`end - start`) |
| `run_x_offset` | Horizontal position within the run (advance-accumulated) |
| `entry` | Rasterised atlas entry (bitmap, bearing, advance) |

Glyph IDs from the font are preserved.  When a font's `calt` feature replaces
`hyphen→SPC` and `greater→hyphen_greater.liga` (JetBrainsMono's approach),
both IDs appear in the output — the glyph count doesn't change.  When a font
has a real ligature substitution (`fi` → ﬁ), one glyph covers 2+ characters
and `num_cells > 1`.

### Atlas rasterisation

Each `LayoutGlyph` from cosmic-text is rasterised via **swash** (the same
`rasterize_swash` function used by the per-char path), so the bitmap quality
is identical regardless of which path produced the glyph.

Results are cached in `run_cache` (keyed by text + font_size) to avoid
re-shaping the same run on every frame.

---

## Stage 2: Run Detection + Rendering (`session.rs`)

### Run detection

`detect_run_end()` walks forward through consecutive cells, stopping at:
- Spaces, spacer cells (CJK/emoji continuations), hidden cells
- Style boundaries (bold/italic change)
- Wide characters

The run text is extracted and checked by `might_ligate()`, which returns
`true` if any adjacent pair of characters are both ASCII punctuation.
This avoids shaping runs that cannot contain ligatures (e.g. alphabetic
words).

### Rendering branch

When a run is ligature-eligible, `shape_and_rasterize_run` shapes it.  The
shaped result is **always used** when shaping succeeds — the per-char
fallback is only for shaping failures.  This is critical for fonts that use
**contextual alternates** (same glyph count, different glyph IDs) rather
than true ligature substitutions.

Within the rendering branch, two sub-paths exist:

**Multi-cell glyphs** (`num_cells > 1` — true ligatures):  
Strip-based UV. The glyph's atlas texture is sliced horizontally, with each
covering cell showing one `cell_width`-wide strip.  UV coordinates for each
strip are adjusted for the glyph's bearing_x offset.

**Single-cell glyphs** (`num_cells == 1` — contextual alternates or regular
glyphs):  
Full-atlas-rect UV. The entire glyph bitmap is used as the texture source,
and the quad is positioned at `cell_col × cell_width + bearing_x`.  Cell
boundary clipping (via ratio-based UV adjustment) trims any overflow.

### Negative bearing handling

Some ligature mechanisms (notably JetBrainsMono's `calt`) produce glyphs with
a **negative left-side bearing** (lsb < 0).  For example, `hyphen_greater.liga`
has lsb = −515 EM units, meaning its ink starts 515 units **left** of the
glyph origin.  The preceding cell (glyph SPC) is designed to be empty, and
the negative-bearing glyph visually merges with the empty cell to create the
`→` appearance.

The renderer:
1. **Positions** the glyph at `cell_col × cw + bearing_x` — quad extends left
   into the previous cell.
2. **Does not clip** the left side when `bearing_x < 0`.
3. Uses the **full atlas rect** as UV source (for single-cell glyphs), so no
   ink is lost to strip boundaries.

---

## Interaction with per-char path

The per-char fallback (`ensure_glyph` → `rasterize_glyph`) is used when:
- Ligatures are disabled in config.
- `might_ligate()` returns false (no ASCII punctuation in the run).
- `shape_and_rasterize_run` fails.

Both paths use swash rasterisation with the same format (`Format::Subpixel`),
so glyph bitmaps are consistent regardless of which path produces them.
The per-char path uses `Shaping::Basic` for ASCII (fast path) and
`Shaping::Advanced` for non-ASCII (complex-script fallback).  The run-based
path always uses `Shaping::Advanced`.

---

## Performance

| Issue | Cause | Impact |
|-------|-------|--------|
| **`might_ligate` triggers on any ASCII punctuation** | Returns `true` if any adjacent pair are punctuation.  File paths like `C:\Users\name\project` match on `:\`, `\U`, `\n`, etc. | Every such run gets a full harfbuzz shape call per frame.  `run_cache` helps but cache hit rate is low during scrolling. |
| **No short-circuit for unchanged glyphs** | When shaping produces the same glyphs as the default per-char path (no ligature substitution occurred), the result is still used.  There is no fast-path back to per-char for runs that don't actually ligate. | Unnecessary Buffer creation + shaping for runs like `123`, `abc\ndef`. |
| **Double atlas rasterisation** | `shape_and_rasterize_run` stores glyphs in `run_cache`.  If the same glyph is later requested via `ensure_glyph` (per-char path), it is rasterised again into `glyph_cache`. | Atlas space used twice for the same bitmap until `grow_atlas()` clears both caches. |

### Possible improvements

- Narrow `might_ligate` to a fixed set of common programming ligatures
  (`->`, `>=`, `!=`, `::`, `//`, `||`, `&&`, etc.) instead of all punctuation.
- After shaping, compare glyphs against the per-char baseline; if identical,
  skip the shaped result and fall through to per-char.
- Share rasterisation between `run_cache` and `glyph_cache`.

---

## Known limitations

| Issue | Cause |
|-------|-------|
| **Run detection is ASCII-only** | `might_ligate` only checks ASCII punctuation pairs. Non-ASCII ligature sequences (e.g. Arabic) will fall through to per-char. |
| **`\` triggers shaping** | Backslash is ASCII punctuation, so file paths like `C:\Users\` trigger `shape_and_rasterize_run` unnecessarily. |
| **Cursor on ligature** | When the cursor is on a cell covered by a ligature, the ligature glyph is still used — the cursor colour is applied per-cell via the loop, but the glyph shape does not change. |
| **Bold/italic ligatures** | Styling variants are not yet wired through the shaping path. |

---

## Test module

A `#[cfg(test)]` module in `crates/zenterm-glyph/src/lib.rs` verifies ligature
shaping with JetBrainsMono Nerd Font.  It shapes each test case (`->`, `>=`,
`!=`, …) with and without font features, and reports whether the glyph count
differs from the character count (true ligature) or stays the same (contextual
alternate).
