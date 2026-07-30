# Ligature Support

> **Status:** Working for fonts that use OpenType `calt`/`liga`/`clig` features.
> Both true ligatures (multiple chars → one glyph) and contextual alternates
> (same glyph count, different glyphs, e.g. JetBrainsMono's `->`) are supported.
> Cursor-on-ligature: ligatures are automatically broken under the cursor so
> individual characters display correctly.

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
`true` if the text contains a known programming ligature pattern (e.g.
`->`, `>=`, `!=`, `::`, `//`).  This avoids shaping runs that cannot
contain ligatures (e.g. alphabetic words, file paths).

### Rendering branch

When a run is ligature-eligible, `shape_and_rasterize_run` shapes it.  The
shaped result is used when it produces an actual ligature/substitution
effect (`had_effect=true`) and the cursor is not on the run.  Otherwise
(when the cursor is on any cell of the run, or shaping had no effect),
the per-char path is used as a fast path.

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
- `might_ligate()` returns false (no known ligature pattern in the run).
- `shape_and_rasterize_run` fails.

Both paths use swash rasterisation with the same format (`Format::Subpixel`),
so glyph bitmaps are consistent regardless of which path produces them.
The per-char path uses `Shaping::Basic` for ASCII (fast path) and
`Shaping::Advanced` for non-ASCII (complex-script fallback).  The run-based
path always uses `Shaping::Advanced`.

---

## Performance

The following issues have been addressed:

| Issue | Resolution |
|-------|------------|
| **`might_ligate` triggers on any ASCII punctuation** | Replaced with a curated set of ~50 known programming ligature patterns (`->`, `=>`, `!=`, `::`, ...).  File paths like `C:\Users\name\project` no longer trigger shaping. |
| **No short-circuit for unchanged glyphs** | After shaping, `shape_and_rasterize_run` compares glyph IDs against a lightweight per-char baseline (`Shaping::Basic`, no layout/rasterization).  If identical, the run is added to `no_effect_cache` and subsequent calls skip shaping entirely, falling through to per-char. |
| **Double atlas rasterisation** | When shaping produces no effect (`had_effect=false`), callers discard the shaped result and use the per-char path instead.  Single-cell glyphs are already populated into `glyph_cache` during shaping (see `shape_and_rasterize_run` lines 467–474), so `ensure_glyph` finds them cached and avoids re-rasterization. |

---

## Known limitations

| Issue | Cause |
|-------|-------|
| **Run detection is ASCII-only** | `might_ligate` only checks ASCII punctuation pairs. Non-ASCII ligature sequences (e.g. Arabic) will fall through to per-char. |
| **Bold/italic ligatures** | Styling variants are not yet wired through the shaping path. |

---

## Test module

A `#[cfg(test)]` module in `crates/zenterm-glyph/src/lib.rs` verifies ligature
shaping with JetBrainsMono Nerd Font.  It shapes each test case (`->`, `>=`,
`!=`, …) with and without font features, and reports whether the glyph count
differs from the character count (true ligature) or stays the same (contextual
alternate).
