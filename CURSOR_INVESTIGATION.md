# Block Cursor Rendering — Investigation Notes

> Status: **inconclusive**. The cursor still does not match the reference
> (Alacritty) after several attempts. This document records everything
> tried, the evidence gathered, the reference implementation, and the
> open questions.

---

## 1. Context

The block cursor in zenterm is rendered differently from Alacritty
(visible in side-by-side screenshots). The author is a long-time terminal
user (8+ years) and reports a clear, reproducible visual difference
between the two emulators with the same font (Menlo), the same size
(36 px at 2× DPI), and the same character (`s`) under the cursor.

The fix has been tried three times in this session, each time with a
different hypothesis, and each time the visual result is still wrong in
a different way.

---

## 2. The Three Hard Requirements

Stated by the user as non-negotiable. Any working cursor must satisfy
all three:

1. **Strict rectangle, no missing corners.** Whatever the cursor's
   shape, it must be a closed rectangle whose four corners are sharp.
   Adjacent rows above and below must not be touched.

2. **Character visible in inverse video.** The character under the
   cursor must be rendered with `fg` and `bg` colours swapped. Just
   painting a solid block with no character visible is unacceptable.

3. **Fixed top and bottom edges.** Moving the cursor to different cells
   must not change the cursor's height or vertical position. The user
   explicitly said: "I do not care where exactly the cursor is, as long
   as it does not change when the cursor moves to a different cell."

The user also said that the cursor must be a uniform rectangle, not
something with cut-off corners, and it must not interfere with the rows
above or below.

---

## 3. Investigation Timeline

### 3.1 First hypothesis — LCD sub-pixel rendering

**Idea:** The fringe / colour offset at the top and bottom of cell
backgrounds is caused by swash's `Format::Subpixel`, which produces
per-channel coverage with 1/3-pixel offsets for the red and blue
channels. The fringe should disappear if we switch to `Format::Alpha`
(grayscale).

**Test:** Changed `rasterize_swash` in
`crates/zenterm-glyph/src/lib.rs:738` from `Format::Subpixel` /
`Format::subpixel_bgra()` to `Format::Alpha`. `img.content` automatically
becomes `SwashContent::Mask` and the existing atlas
`SwashContent::Mask` branch (1 byte per pixel) handles storage
correctly. The MASK shader path uses `texel.r` as alpha.

**Result:** The fringe did not go away. The user reported: "As expected,
the problem is still there." → LCD sub-pixel rendering is **not** the
cause. (Change was kept in the tree for diagnostic purposes but is not
the real fix.)

**Mental mistake:** I confidently said "this will fix it" before the user
could even test, on the basis of "all colour tints at glyph edges are
subpixel". I was guessing.

### 3.2 Second hypothesis — cell geometry (line_height vs cap_height)

**Idea:** zenterm's `cell_height = ceil(line_height) = 36` for Menlo
36 px, which equals `max_ascent + max_descent = 28 + 8 = 36`. The
`max_ascent` (28) is *larger* than the cap height (~25) by ~3 px; that
extra is the "above-cap-height buffer" reserved for diacritics like `Á`
or `Ž`. The cursor was filled with a cell-sized SOLID (36 px tall), so it
overshoots the character body by 3 px at the top and 8 px at the bottom.

The user said this was wrong: the cursor in Alacritty does **not** include
that 8-px descender area below the character; the cursor's bottom is
right at the baseline.

**Test:** Added a `cap_height: f32` field to `GlyphAtlas`, measured it
in a new `measure_cap_height()` function by shaping a single `M` and
reading `placement.top` from swash (the y-up distance from baseline to
the topmost bitmap row, which is the cap height of a capital letter),
exposed it via `cap_height()`. Then in the cursor rendering code in
`crates/zenterm-ui/src/app.rs`, the SOLID quad's height was changed from
`ch` to `cap_height + cell_descent`, and its top was changed from
`row*ch` to `row*ch + (baseline - cap_height)`.

**Result:** *Better than before, but still wrong.* Two new complaints
from the user:

- The **bottom** still shows a "missing corner" effect, although
  reduced.
- The **top** of the cursor is now *lower* than the top of `l`. The
  cursor moves to `l` and the `l` extends *above* the cursor,
  creating a new missing-corner effect at the top.

**Mental mistake:** I assumed `M.placement.top` is the cap height of
all characters. It is not — in Menlo the `l` is a tall lowercase
whose bitmap top is **higher** than the cap height of `M`, because of
the small curved cap at the top of the `l`. My measurement under-
estimates the height of `l`.

### 3.3 Third hypothesis — the cursor should be the full cell (alacritty
behaviour)

The author of this document went and read Alacritty's source (vendored
under `alacritty/`) and found the answer. Details in §4. The conclusion:
the fix is to *revert* to cell-sized SOLID; Alacritty does exactly that
and the user said Alacritty looks fine.

---

## 4. The Reference Implementation (Alacritty)

Alacritty's `Block` cursor is **not** a separate rectangle. It is the
cell itself, rendered with the cell's `fg` and `bg` colours swapped.

### 4.1 Cursor-rect code path

`alacritty/alacritty/src/display/cursor.rs:30-34`:

```rust
match self.shape() {
    CursorShape::Beam        => beam(...),
    CursorShape::Underline   => underline(...),
    CursorShape::HollowBlock => hollow(...),
    _                        => CursorRects::default(),   // <-- Block falls here
}
```

The wildcard arm returns an empty `CursorRects` for Block. The
beam/underline/hollow shapes do draw a separate rect, but Block does
not.

### 4.2 Cursor "rendering" code path

`alacritty/alacritty/src/display/content.rs:166-172`:

```rust
if self.cursor_point == cell.point {
    self.cursor = self.renderable_cursor(&cell);
    if self.cursor.shape == CursorShape::Block {
        cell.fg = self.cursor.text_color;
        cell.bg = self.cursor.cursor_color;
        cell.bg_alpha = 1.;
    }
    return Some(cell);
}
```

The block cursor is implemented by **swapping `fg` and `bg` of the
cell**, then returning the cell so it is drawn like any other text
cell. The cell is then drawn at its natural `glyph_y_px` position,
with its `bg` covering the whole cell rectangle.

### 4.3 Cell metrics

`alacritty/alacritty/src/display/mod.rs:1608-1614`:

```rust
fn compute_cell_size(config: &UiConfig, metrics: &crossfont::Metrics) -> (f32, f32) {
    let offset_x = f64::from(config.font.offset.x);
    let offset_y = f64::from(config.font.offset.y);
    (
        (metrics.average_advance + offset_x).floor().max(1.) as f32,
        (metrics.line_height + offset_y).floor().max(1.) as f32,
    )
}
```

Cell height is `line_height + offset_y`, identical to zenterm's
`line_height.ceil()`. So **Alacritty's cell and zenterm's cell are the
same size**. The visual difference between the two emulators is not
explained by cell metrics.

### 4.4 What Alacritty's cursor actually looks like

For a `s` under the cursor in Menlo 36 px:
- The cursor is the **full cell** (36 px tall × cw wide).
- The "above cap height" buffer (3 px) is part of the cursor, drawn in
  the cursor background colour.
- The descender area (8 px below the baseline) is part of the cursor,
  drawn in the cursor background colour.
- The character is rendered in the text (inverse) colour at its
  natural position (cap height → baseline, i.e. y=3 → y=28 in the
  cell).

The user said: "In Alacritty the letter's bottom to the cursor's bottom
is still a small distance." That "small distance" is the 8-pixel
descender area, which is the same as in zenterm. The user is fine
with it in Alacritty but not in zenterm — that asymmetry is the
remaining puzzle (see §7).

---

## 5. Zenterm's Current Implementation

### 5.1 File: `crates/zenterm-glyph/src/lib.rs`

The atlas measures and exposes metrics. Relevant state and methods:

| Field / method                | Lines       | Meaning                                       |
|-------------------------------|-------------|-----------------------------------------------|
| `cell_ascent: f32`            | 106         | max_ascent of `"Mg"`, includes above-cap buffer |
| `cell_descent: f32`           | 110         | max_descent of `"Mg"`                         |
| `cap_height: f32`             | 111 (added) | placement.top of `"M"`                        |
| `metrics: Metrics`            | 86          | cosmic-text Metrics                            |
| `cell_baseline_offset()`      | 323         | returns `cell_ascent`                          |
| `cap_height()`                | (added)     | returns `cap_height`                           |
| `measure_baseline()`          | 270         | shapes `"Mg"`, fills `cell_ascent/descent`    |
| `measure_cap_height()`        | (added)     | shapes `"M"`, fills `cap_height` from swash    |
| `cell_size()`                 | 222         | entry point; calls both measure_* funcs        |

`measure_cap_height` is called from `cell_size` after `measure_baseline`.

### 5.2 File: `crates/zenterm-ui/src/app.rs`

The block cursor branch (`is_block_cursor`) currently pushes **two
quads** to `cursor_bg_instances`:

1. **SOLID quad** (currently `cursor_height = cap_height + cell_descent`
   tall, cell-wide, `cell.fg` colour). Lines around 626–670.
2. **GLYPH quad** at the character's natural position, `cell.bg` colour
   (inverse). Drawn on top of the SOLID.

Non-block cursors (Beam / Underline / HollowBlock) take a separate
path (line ~688+) and are unrelated to this bug.

### 5.3 What the user sees in the current state

- The block cursor SOLID is 33 px tall (25 cap + 8 descent), 22 px wide.
- It starts at y=3 of the cell, ends at y=36 (cell bottom).
- The character is drawn in inverse at y=3 → y=28.
- A character like `s` (no descender) ends at y=28; the area y=28 →
  y=36 is the descender area, drawn in cursor background.
- A character like `l` (tall lowercase) has its top at y=2 (one
  pixel **above** the cursor top at y=3), because `l.placement.top`
  is one pixel larger than `M.placement.top`.

This produces:
- **Top "missing corner":** `l` extends one pixel above the cursor.
- **Bottom "missing corner":** the descender area is cell-width while
  the character is character-width, producing a visual step at the
  baseline.

---

## 6. Reference Implementations

### 6.1 Alacritty

`alacritty/alacritty/src/display/cursor.rs` + `content.rs` (see §4).
Key insight: **Block = swap cell colours, no separate rect.**

### 6.2 wezterm

The zenterm comment at `lib.rs:240-245` and elsewhere cites
`cell_height + descender - bearing_y` as wezterm's formula. This
matches Alacritty's approach (use the cell).

### 6.3 cosmic-term (the "cosmic-term pattern")

`crates/zenterm-ui/src/app.rs:485-512` mentions cosmic-term as a
reference. The original comment said cosmic-term skips drawing a
quad when `metadata.bg == default_metadata.bg`, the same "let the
default background show through" pattern.

### 6.4 What the three references have in common

None of them draw a separate SOLID rect for the block cursor. They all
either swap colours on the cell (Alacritty) or rely on the cell's
own background fill (cosmic-term). The current zenterm approach of
"a SOLID quad + a GLYPH quad on top" is **unique to zenterm**.

---

## 7. Open Questions

These are the things I could not resolve, in order of importance:

1. **Why does Alacritty's cursor look fine but zenterm's doesn't, when
   both fill the same cell?** They should be identical. The user has
   8 years of experience and reports a clear difference. Possible
   explanations I could not verify:
   - Different font metrics (different `line_height` per font).
   - Different blending / rasterisation in the GPU pipeline.
   - Different cell-bg vs cell-fg colours for the cursor.

2. **Why is `l.placement.top` larger than `M.placement.top` in Menlo?**
   My cap-height measurement from `M` is the wrong value to use. The
   correct font-level value is the OS/2 table's `sCapHeight` field,
   but it is not currently read in zenterm. The fix should use
   `sCapHeight` (or at least `max(placement.top for each capital
   letter)`) instead of `M.placement.top` alone.

3. **Should the "above cap height" buffer (3 px at the top of every
   cell) be inside the cursor or outside?** Alacritty keeps it inside
   the cursor (because the whole cell is the cursor). The user said
   that, after my cap-height fix, the top is too low; they want the
   cursor to cover `l`. Implies: keep the buffer inside the cursor.
   That means: **the cursor should be the full cell**, not cap-height
   + descent.

4. **Is the "missing corner" at the bottom a real bug or perception?**
   The user said the bottom is "less than before but still has a step".
   With the full-cell SOLID, the SOLID is cell-width and cell-tall; the
   character is character-width; the descender area is cell-width and
   cell-fg colour. In Alacritty this is the standard look. If zenterm
   visually differs from Alacritty, the cause is **not in the SOLID
   quad**, because they are identical. It must be in the rendering or
   blending. This was not investigated.

5. **What does the user actually want, geometrically?** The user has
   said three things that are pairwise incompatible:
   - "Cursor must be a strict rectangle" → full cell.
   - "No step at the baseline" → character-width on both sides of
     the character.
   - "Cursor must be character-sized" → cap height to baseline.
   Only one of the three can be true at a time. The user has not yet
   picked which.

---

## 8. Code Reference Summary

### 8.1 File map

| Path                                              | Purpose                                |
|---------------------------------------------------|----------------------------------------|
| `crates/zenterm-glyph/src/lib.rs`                 | Glyph atlas, font metrics, swash glue  |
| `crates/zenterm-ui/src/app.rs`                    | Cell → instance data, block cursor path |
| `crates/zenterm-render/src/lib.rs`                | WGSL shaders                           |
| `alacritty/alacritty/src/display/cursor.rs`       | Reference: cursor rect dispatch         |
| `alacritty/alacritty/src/display/content.rs`      | Reference: cell colour swap             |
| `alacritty/alacritty/src/display/mod.rs`          | Reference: cell-size computation        |

### 8.2 Key constants for Menlo 36 px

| Quantity                  | Value       | Source                              |
|---------------------------|-------------|-------------------------------------|
| `font_size`               | 36.0        | `app.rs:163` (`18.0 * ppp`)         |
| `metrics.line_height`     | 36.0        | `Metrics::new(36, 36)`              |
| `cell_height` (zenterm)   | 36.0        | `cell_height.ceil()`                |
| `cell_height` (Alacritty) | 36.0        | `line_height + offset_y` (offset=0)  |
| `cell_ascent` (max_ascent)| ~28.0       | `measure_baseline` shaping "Mg"     |
| `cell_descent`            | ~8.0        | `measure_baseline` shaping "Mg"     |
| `cap_height` (from M)     | ~25.0       | `measure_cap_height` shaping "M"    |
| `cap_height` (OS/2 sCapHeight for Menlo) | ~26.2 | OS/2 table — not currently read     |
| `l.placement.top`         | ~26.0       | swash — observed 1 px taller than M  |
| "above cap" buffer        | ~3.0        | cell_ascent − cap_height             |

(All numbers are best-effort estimates; the actual values depend on the
font file and rendering pipeline.)

### 8.3 Block cursor code (current state)

`crates/zenterm-ui/src/app.rs`, inside the cell loop in
`update_cell_instances`, when `is_block_cursor` is true:

```rust
// cap_height and cursor_height are computed once before the loop:
let cap_height = self.glyph_atlas.cap_height();
let cursor_height = cap_height + self.glyph_atlas.cell_descent();

// Inside the loop:
let cursor_top = cell_top + (baseline - cap_height);
let bqy = 1.0 - cursor_top * y_scale;
let bqx = px_to_clip_x((col as f32 * cw).round());
let bqw = cw * x_scale;
let bqh = cursor_height * y_scale;

// Layer 1: SOLID (cursor background, cell.fg)
cursor_bg_instances.push(CellInstance {
    clip_pos: [bqx, bqy],
    clip_cell_size: [bqw, bqh],
    fg_color: [cell.fg.r(), cell.fg.g(), cell.fg.b(), 1.0],
    bg_color: [cell.fg.r(), cell.fg.g(), cell.fg.b(), 1.0],
    flags: glyph_type::SOLID,
});

// Layer 2: GLYPH (character in inverse colour)
cursor_bg_instances.push(CellInstance {
    clip_pos: [gqx, gqy],
    clip_cell_size: [gqw, gqh],
    fg_color: [cell.bg.r(), cell.bg.g(), cell.bg.b(), 1.0],
    bg_color: [cell.fg.r(), cell.fg.g(), cell.fg.b(), 1.0],
    flags: gtype,
});
```

The current `cursor_height` is `cap_height + cell_descent` (33 px for
Menlo 36 px). The user's most recent feedback: the bottom still has a
"step", the top is too low for `l`. See §7 question 2.

---

## 9. Lines of Investigation Tried (Summary)

| # | Hypothesis                                                 | Outcome        |
|---|------------------------------------------------------------|----------------|
| 1 | LCD sub-pixel rendering causes the fringe                  | Disproved      |
| 2 | Cell metrics include "above cap" buffer that should be cut | Partly right, partly wrong — see §3.2 |
| 3 | Alacritty's behaviour = swap cell colours, not a separate rect | Identified, not yet applied to a working fix |

The third hypothesis is the most likely correct path, but applying it
faithfully in zenterm means reverting to cell-sized SOLID plus the
character quad, which the user has already complained about twice.

---

## 10. Suggested Next Steps (For Whoever Picks This Up)

1. **Read OS/2 `sCapHeight` from the font** instead of measuring
   `M.placement.top`. swash has this via the font's OS/2 table. This
   gives a single, font-level cap-height value that does not depend on
   which character is shaped.

2. **Try Alacritty's approach** literally: render the cursor cell
   with swapped colours, no separate SOLID rect. This requires either
   inverting the cell's `fg`/`bg` before the cell is queued (clean) or
   drawing the cell via the regular `glyph_instances` path with a flag
   indicating "this is the cursor cell, draw the full cell with swapped
   colours". The current two-quad approach is fighting the framework.

3. **Verify visually against Alacritty** by running both side-by-side
   on the same machine, with the same font, the same DPI, the same
   character, and the same theme. This rules out perceptual
   differences from monitor / scaling / theme.

4. **Add a regression test** that snapshots the rendered cursor cell
   for `s`, `l`, `g`, `A`, `M` and compares it against a known-good
   reference image. This catches the "1 pixel off" cases that are
   hard to see by eye but obvious in a diff.

---

## 11. Honest Assessment

The cursor looks subtly wrong in zenterm, the fix is not obvious, the
three attempts in this session each made the visual result different but
not correct, and the user is frustrated. The most likely correct path
is the one Alacritty takes (swap cell colours, no separate rect), but
applying it in zenterm will require architectural changes that the
author should approve before anyone sinks more time into it.

A second-best path is to keep the current two-quad approach but
measure cap height from the OS/2 table so `l` is covered, and accept
that the bottom step (8 px of descender area, same as Alacritty) is
not actually a bug.

Either way: do **not** trust any further "I think this will fix it"
from this author. The user has caught every wrong claim so far.
