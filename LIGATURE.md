# Ligature Support — Remaining Work

This document tracks what remains after the preparatory refactoring (run-aware
render loop, `shape_and_rasterize_run` API, `RunCacheKey`, `ShapedGlyph` types,
`ligatures` config field).  See [`docs/architecture.md`](./docs/architecture.md)
for the overall codebase structure.

---

## Summary

| Phase | Lines | Difficulty | Risk |
|-------|-------|------------|------|
| A — Run shaping | ~80 | Medium | cosmic-text Buffer API |
| B — Run detection | ~20 | Low | Boundary correctness |
| C — Wiring | ~15 | Low | — |
| D — Per-cell strip splitting | ~80 | **High** | UV math, clipping, visual |
| E — Edge cases (cursor, selection, ZWJ) | ~30 | Medium | Correctness |
| **Total** | **~225** | | |

The preparatory refactoring is ~260 lines with zero performance impact and safe
compilation.  The remaining ~225 lines carry visual risk — Phase D in particular
requires careful UV-coordinate math and visual verification.

---

## Phase A — Run Shaping (`.crates/zenterm-glyph/src/lib.rs`)

**File:** `crates/zenterm-glyph/src/lib.rs`
**Method:** `GlyphAtlas::shape_and_rasterize_run`

**Current (placeholder):**

```rust
for c in text.chars() {
    let (entry, _) = self.ensure_glyph(c)?;
    shaped.push(ShapedGlyph { char_range: i..i+1, num_cells: 1, .. });
}
```

**Required:**

```rust
let shaping = if self.ligatures_enabled {
    Shaping::Advanced   // enables liga/clig OpenType features
} else {
    Shaping::Basic      // fast path, no ligatures
};

let mut buf = Buffer::new(&mut self.font_system, self.metrics);
buf.set_size(Some(self.font_size), None);
let attrs = Attrs::new()
    .family(Family::Name(&self.font_family))
    .weight(/* bold? */)
    .style(/* italic? */);
buf.set_text(text, &attrs, shaping, None);
buf.shape_until_scroll(&mut self.font_system, true);

let glyphs = buf.lines[0].layout_opt()
    .and_then(|l| l.first())
    .map(|line| &line.glyphs[..])
    .unwrap_or_default();

for g in glyphs {
    let is_ligature = g.end - g.start > 1;
    // 1. Get physical glyph + cache_key
    let phys = g.physical((0.0, 0.0), 1.0);
    // 2. Rasterize via rasterize_swash(&phys.cache_key)
    // 3. Allocate in atlas + copy pixels
    // 4. Build ShapedGlyph:
    //      char_range = g.start .. g.end
    //      num_cells  = g.end - g.start
    //      run_x_offset = accumulated advance
    //      entry = GlyphEntry { atlas_rect, bearing_x/y, advance, .. }
}
```

**Key details:**
- `cosmic_text::LayoutGlyph::start` / `end`  are indices in the source string.
  When `end - start > 1`, a ligature substitution occurred and this one glyph
  covers multiple source characters (and therefore multiple terminal cells).
- The `glyph_id` in `LayoutGlyph` is the font's internal glyph ID — use
  `phys.cache_key` for swash rasterisation (same as current `rasterize_swash`).
- If the font has no ligature rules for this text, `glyphs.len()` will equal
  `text.len()` and each `end - start == 1` — the output is identical to the
  per-char path.

---

## Phase B — Run Detection (`.crates/zenterm-ui/src/session.rs`)

**File:** `crates/zenterm-ui/src/session.rs`
**Function:** `detect_run_end`

**Current:**

```rust
fn detect_run_end(..) -> usize {
    let _ = first;
    start_col + 1   // always 1-cell runs
}
```

**Required:** Uncomment the loop body:

```rust
fn detect_run_end(grid: &GridView, row: usize, start_col: usize, cols: usize) -> usize {
    let first = match grid.cell(row, start_col) {
        Some(c) => c,
        None => return start_col + 1,
    };

    let mut col = start_col + 1;
    while col < cols {
        let cell = match grid.cell(row, col) {
            Some(c) => c,
            None => break,
        };
        // Spaces never participate in ligatures.
        if cell.c == ' ' || cell.is_spacer || cell.hidden {
            break;
        }
        // Style boundary: different weight/style needs separate shaping.
        if cell.bold != first.bold || cell.italic != first.italic {
            break;
        }
        col += 1;
    }
    col
}
```

Add a fast pre-check to avoid pointless `Shaping::Advanced` calls:

```rust
/// Quick check: only multi-char runs containing ASCII punctuation or
/// operators are worth shaping with Shaping::Advanced.  Everything else
/// can take the per-char fast path.
fn might_ligate(text: &str) -> bool {
    text.len() > 1 && text.bytes().any(|b| b.is_ascii_punctuation())
}
```

This is used in the render loop:

```rust
if ligatures_enabled && run_text.len() > 1 && might_ligate(&run_text) {
    let shaped = atlas.shape_and_rasterize_run(&run_text)?;
    // Phase D: distribute glyphs across cells
} else {
    // per-char fast path (existing behaviour)
}
```

---

## Phase C — Wiring the Render Loop (`.crates/zenterm-ui/src/session.rs`)

**File:** `crates/zenterm-ui/src/session.rs`
**Location:** `update_cell_instances`, inside the row loop (currently uses
`_run_start` / `_run_end` placeholders)

**Current:**

```rust
let _run_start = col;
let _run_end = detect_run_end(&grid, row, col, cols);

// ... per-char ensure_glyph ...

col += 1;
// FUTURE: col = _run_end
```

**Required:**

```rust
let run_start = col;
let run_end = detect_run_end(&grid, row, col, cols);
let run_text = extract_run_text(&grid, row, run_start, run_end);

if ligatures_enabled && run_text.len() > 1 && might_ligate(&run_text) {
    let shaped = atlas.shape_and_rasterize_run(&run_text)?;
    for sg in &shaped {
        let cell_range = run_start + sg.char_range.start .. run_start + sg.char_range.end;
        // Phase D: build per-cell CellInstance with clipped UVs
    }
    col = run_end;
} else {
    // Per-char path (unchanged)
    // ...
    col += 1;
}
```

---

## Phase D — Per-Cell Strip Splitting (`.crates/zenterm-ui/src/session.rs`)

This is the most complex and highest-risk piece.

### The problem

A ligature glyph like `->` may be 2× the width of a single cell:

```
         cell 5              cell 6
┌────────────────────┬────────────────────┐
│←── ligature glyph ──→│                    │
│  atlas_rect = 40×20  │                    │
│  advance ≈ 2*cw      │                    │
└────────────────────┴────────────────────┘
```

We need to render it as two separate strips (one per cell), each with the
correct foreground/background colour for that cell (cursor, selection, etc.).

### Algorithm

For each `ShapedGlyph` with `num_cells > 1`:

```
for cell_offset in 0 .. sg.num_cells {
    cell_col = run_start + sg.char_range.start + cell_offset;

    // Strip boundaries in pixels, relative to the glyph origin.
    strip_left  = cell_offset * cell_width;
    strip_right = (cell_offset + 1) * cell_width;
    glyph_width = sg.entry.advance;  // total advance of this glyph

    // UV clamping: the strip is a fraction of the full glyph atlas rect.
    u_min = (sg.entry.atlas_rect.min.x + strip_left / scale) / tex_size;
    u_max = (sg.entry.atlas_rect.min.x + strip_right / scale) / tex_size;

    // Build CellInstance with:
    //   clip_pos      → cell_col * cell_width (session-local)
    //   clip_cell_size → cell_width, cell_height
    //   glyph_size    → strip_width, atlas_height (native resolution)
    //   glyph_offset  → bearing_x - strip_left, bearing_y
    //   uv_min/uv_max → computed above
    //   fg/bg_color   → per-cell (handles cursor & selection)
    //   flags         → SUBPIXEL or MASK or COLOR
}
```

**Important:** a ligature glyph may extend beyond `cell_width * num_cells` due
to kerning or overshoot.  The existing clipping code in
`update_cell_instances` (see [`GLYPH_CLIP.md`](./GLYPH_CLIP.md)) already
handles this: if the glyph quad exceeds the cell bounds, UVs are adjusted and
the quad is clipped.  For ligature strips, **each strip is also clipped**,
so overspill into adjacent cells is naturally prevented.

### Interaction with existing clipping

The current `GLYPH_CLIP.md` logic clips glyph quads to cell boundaries:

```rust
let cell_left   = col as f32 * cw;
let cell_top    = row as f32 * ch;
let cell_right  = cell_left + cw * num_cells;
let cell_bottom = cell_top + ch;

if clipped_h < scaled_h { /* adjust UV vertically */ }
if clipped_w < scaled_w { /* adjust UV horizontally */ }
```

For ligature strips, the same clipping applies — each strip is an independent
`CellInstance` and goes through the same pipeline.  No shader changes needed.

### Foreground colour per cell

Because each strip is its own `CellInstance`, cursor and selection highlighting
work for free:

| Cell state | `fg_color` / `bg_color` in `CellInstance` |
|------------|-------------------------------------------|
| Normal     | `cell.fg`, `cell.bg` |
| Cursor (block) | swapped: `cell.bg`, `cell.fg` |
| Selected   | selection colours |
| Cursor + selected | cursor colours (selection takes precedence?) |

No special ligature-aware logic required — the per-cell instance data handles
it automatically.

---

## Phase E — Edge Cases

### E.1 Cursor at ligature boundary

- Block cursor on cell `i` → that cell's strip gets cursor colours, adjacent
  cell's strip gets normal colours.  Already works via Phase D.

### E.2 Selection spanning part of a ligature

- If only cell 5 of a `->` ligature is selected, strip 5 gets selection
  colours, strip 6 gets normal colours.  Already works via Phase D.

### E.3 Zero-width characters (ZWNJ, ZWJ, combining marks)

- A ZWNJ (`U+200C`) between two characters should **prevent** ligation.
  `cosmic-text` respects this automatically when using `Shaping::Advanced`.
- Combining marks (e.g. `é` as `e` + `U+0301`) result in one glyph covering
  multiple chars.  These are **not** ligatures in the terminal sense — they
  still occupy 1 cell.  The run detection must not split between a base
  character and its combining mark.  This is handled by not treating
  non-ASCII-punctuation as a run boundary.

### E.4 Wide characters (CJK, emoji)

- Characters occupying 2 cells (`is_spacer == true`) are run boundaries
  already handled by `detect_run_end`.
- Emoji ZWJ sequences (e.g. `👨‍👩‍👧`) are shaped by `cosmic-text` into a single
  glyph.  These should be treated as non-ligature runs — `might_ligate`
  returns `false` because they contain no ASCII punctuation.

### E.5 Copy / paste

- The terminal grid always stores original characters.  Copy uses
  `Terminal::selected_text()` which returns the original character sequence,
  not shaped glyphs.  No changes needed.

### E.6 Bold / italic

Currently `ensure_glyph` does not consider `cell.bold` / `cell.italic` when
selecting font weight or style.  A correct ligature implementation needs to:

1. Pass `bold: bool, italic: bool` to `shape_and_rasterize_run`.
2. Set `Attrs::weight()` and `Attrs::style()` accordingly.
3. Extend `RunCacheKey` with style flags so that `->` in bold gets a
   different (or same) cache entry depending on the font.

**This is ~30 lines of additional work** across `ShapedGlyph`, `RunCacheKey`,
`GlyphAtlas::shape_and_rasterize_run`, and `SharedGlyphAtlas`.

For a minimal first implementation, bold/italic can be ignored (always use
regular weight) and added later.

---

## Risk Register

| Risk | Mitigation |
|------|-----------|
| UV mapping off-by-one → visible seams between strips | Use `0.5`-pixel inset (existing pattern in `GLYPH_CLIP.md`) |
| Ligature glyph wider than `num_cells * cell_width` → overflow into next cell strip | Existing clipping handles this per-strip |
| Font with no ligature rules behaves differently under `Shaping::Advanced` | `Shaping::Advanced` falls back to basic shaping when no OpenType features match; visually identical |
| First frame with many unique ligatures causes atlas growth + GPU upload spike | Run cache (`run_cache`) prevents re-shaping; pre-seed common ligatures (`->`, `=>`, `!=`, `<=`, `>=`, `::`) |
| `cosmic-text` font fallback returns glyphs from different fonts in the same run | `rasterize_swash` already handles per-glyph `font_id` — each glyph is rasterised from its own font |
