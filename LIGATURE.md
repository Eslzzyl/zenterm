# Ligature Support — Current Status

> **Last updated:** 2026-06-27

## Summary

Ligature support has been implemented across Phases A–E (~850 lines) but has two
significant problems that prevent it from being practically usable:

| Problem | Severity | Root Cause |
|---------|----------|------------|
| **Ligatures don't actually work** | 🔴 | cosmic-text 0.19 word-splitting at Unicode line-break boundaries prevents harfbuzz from seeing ligature context ([#378](https://github.com/pop-os/cosmic-text/issues/378)) |
| **Per-cell strip splitting causes visual artifacts** | 🔴 | `Shaping::Advanced` vs `Shaping::Basic` produce different glyph bitmaps → visible "shadow" at cell boundaries |

---

## What Was Implemented

### Phase A — Run Shaping (`crates/zenterm-glyph/src/lib.rs`)

`shape_and_rasterize_run` uses `cosmic-text::Buffer` + `Shaping::Advanced` to
shape a run of characters as a single string.  When a ligature substitution
occurs (a single `LayoutGlyph` covering `end - start > 1` source characters),
one `ShapedGlyph` with `num_cells > 1` is produced.

- `rasterize_swash_entry` helper: rasterizes a `LayoutGlyph` via swash and packs
  it into the atlas (factored out of `rasterize_glyph`)
- `FontFeatures` with `liga`/`clig`/`calt`/`dlig`/`kern` are passed to harfbuzz
- `run_cache.clear()` in `grow_atlas()` prevents stale atlas entries after growth
- Glyphs are flattened from ALL layout runs (not just `.first()`) so font-fallback
  splits don't lose glyphs

### Phase B — Run Detection (`crates/zenterm-ui/src/session.rs`)

`detect_run_end` walks consecutive same-style cells and breaks at:
- Spaces, spacer cells (CJK/emoji), hidden cells
- Style boundaries (bold/italic change)
- Wide characters (cell whose right neighbour is a spacer)

Helpers added: `might_ligate()`, `extract_run_text()`, `emit_deco_for_cell()`,
`glyph_grid_num_cells()` (grid-aware cell-width computation).

### Phase C+D — Strip Splitting + Wiring

The ligature branch in `update_cell_instances`:
1. Shapes the entire run via `shape_and_rasterize_run`
2. Distributes every glyph across its covering cells via per-cell strip splitting
3. Each strip gets per-cell cursor/selection colours
4. `col = run_end; continue` skips past the entire run

### Phase E — Edge Cases

- Cursor at ligature boundary: per-cell colour works automatically
- Selection spanning part of a ligature: per-cell colour works automatically
- ZWNJ/ZWJ: `cosmic-text` handles these; `might_ligate` requires ASCII punctuation
- Wide chars (CJK/emoji): `detect_run_end` breaks at spacer cells
- Copy/paste: uses grid characters, not shaped glyphs → no changes needed
- Bold/italic styling: deferred (not wired through shaping yet)

---

## Why Ligatures Don't Work

### Problem 1: cosmic-text word-splitting (cosmic-text#378)

cosmic-text splits text into "words" at **Unicode line-break opportunities**
before passing each word to harfbuzz.  For a prompt like `C:\Users\Eslzzyl>=`,
the `>=` may be split across word boundaries, so harfbuzz only sees `>` and `=`
as separate words and never applies the ligature substitution.

A **ligature probe** (PR [#452](https://github.com/pop-os/cosmic-text/pull/452))
was added in cosmic-text 0.17 to detect potential ligatures and prevent those
specific line breaks, but it is incomplete — many ligatures still go undetected.

**Status:** Upstream issue, no fix available in cosmic-text 0.19.x.
The proper fix would be to shape the *entire span* as a unit then reshape
at line-break boundaries (see issue comments for the proposed architecture).

### Problem 2: Visual divergence between `Shaping::Advanced` and `Shaping::Basic`

The per-char path (`ensure_glyph` → `rasterize_glyph`) uses `Shaping::Basic`
for ASCII characters (fast path), while the run-based path
(`shape_and_rasterize_run`) uses `Shaping::Advanced` (required for ligatures).
These produce **different swash `CacheKey` values** → different rasterisation →
different bitmap dimensions/bearing → visible "shadow" or misalignment when
both paths render adjacent cells.

Attempted fix: pass `FontFeatures` and use `Shaping::Advanced` in both paths.
This made the glyphs consistent but still doesn't activate ligatures (Problem 1).

---

## Known Performance Issues

| Issue | Cause | Impact |
|-------|-------|--------|
| **Double atlas allocation on first frame** | `shape_and_rasterize_run` rasterizes into `run_cache`, per-char path re-rasterizes same chars into `glyph_cache` | ~2× atlas fill rate until `grow_atlas()` clears both caches |
| **`has_new_glyphs` not propagated** | Ligature branch rasterizes new glyphs but doesn't set `has_new_glyphs = true` | Atlas data may not reach GPU after ligature-only allocations |
| **`might_ligate` is ASCII-punctuation based** | Triggers the shaping codepath for any run containing `:`, `.`, `\`, etc. | Unnecessary Buffer creation + harfbuzz call for every file path |

---

## How to Fix (Future Work)

### Fix ligature detection (upstream or workaround)

The only reliable fix for Problem 1 is to shape the **entire span** as one
harfbuzz call, then re-shape grapheme clusters that cross line-break boundaries.
This is tracked in [cosmic-text#378](https://github.com/pop-os/cosmic-text/issues/378).

Until then, ligatures will not render regardless of what this project's code does.

### Revert to per-char path for visual correctness

If the strip-splitting codepath continues to cause visual artifacts, the ligature
branch should be disabled at compile time or gated behind a config flag that
defaults to `false`.  The `detect_run_end` improvements (wide-char break, spacer
handling) should remain enabled as they fix CJK rendering independent of ligatures.

---

## Risk Register

| Risk | Mitigation |
|------|-----------|
| UV mapping off-by-one → visible seams between strips | Use `0.5`-pixel inset (existing pattern in `GLYPH_CLIP.md`) |
| Ligature glyph wider than `num_cells * cell_width` → overflow into next cell | Existing clipping handles this per-strip |
| Font with no ligature rules behaves differently under `Shaping::Advanced` | `Shaping::Advanced` falls back to basic when no OpenType features match |
| First frame with many unique ligatures causes atlas growth + GPU upload spike | Run cache (`run_cache`) prevents re-shaping |
| `cosmic-text` font fallback returns glyphs from different fonts in same run | `rasterize_swash` handles per-glyph `font_id` |
