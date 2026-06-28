//! Ligature detection and run-boundary helpers.
//!
//! These free functions are called by the render loop to group
//! consecutive same-style characters into "runs" that can be shaped
//! together via cosmic-text for ligature support.

use zenterm_term::GridView;

// ── Ligature pattern database ──────────────────────────────────────────

/// Known programming ligature patterns.
///
/// Covers Fira Code, JetBrainsMono, Cascadia Code, Iosevka, etc.
/// Ordered roughly by likelihood to improve short-circuiting.
const LIGATURE_PATTERNS: &[&str] = &[
    // Arrows (most common)
    "->", "=>", "<-", "<=", "->>", "-->>", "-->",
    "<--", "<<--", "->-", ">-", "<->", "<==>", "<==",
    // Comparison (very common)
    "==", "!=", "===", "!==", ">=", "<=", ">>=", "<<=",
    // Logical
    "||", "&&", "^^",
    // Assignment / lambda
    ":=", "::=", "=>",
    // Member access / range
    "::", "..", "...", "..=", ".=",
    // Pipe / compose
    "|>", "<|", "<|>",
    // Comments / preprocessor
    "//", "///", "//!", "/*", "*/",
    // Math / increment
    "+=", "-=", "*=", "/=", "**", "++", "--",
    // Bitwise / compound
    "&=", "|=", "^=", "%=",
    // Other
    "#(", "#{", "#[", "#![", "#!",
    "~=", "!~",
    ".<", ".>",
];

// ── Detection helpers ──────────────────────────────────────────────────

/// Quick heuristic: does this run potentially contain a ligature?
///
/// Checks against a fixed set of known programming ligature patterns,
/// which is far more precise than the old "any consecutive ASCII
/// punctuation pair" check that would false-positive on things like
/// `C:\Users` (matching `:\`) or `\n`.
pub(crate) fn might_ligate(text: &str) -> bool {
    if text.len() < 2 {
        return false;
    }

    // Fast rejection: must contain at least one ligature-seeding character.
    let bytes = text.as_bytes();
    if !bytes.iter().any(|b| {
        matches!(b, b'-' | b'=' | b'!' | b'>' | b'<' | b':' | b'|'
                    | b'&' | b'+' | b'#' | b'/' | b'*' | b'~' | b'^' | b'%')
    }) {
        return false;
    }

    LIGATURE_PATTERNS.iter().any(|pat| text.contains(pat))
}

/// Extract the concatenated character text for a run of cells.
///
/// The returned string is passed to
/// [`GlyphAtlas::shape_and_rasterize_run`] for ligature-aware shaping.
pub(crate) fn extract_run_text(grid: &GridView, row: usize, start: usize, end: usize) -> String {
    let mut s = String::with_capacity(end - start);
    for col in start..end {
        if let Some(cell) = grid.cell(row, col) {
            s.push(cell.c);
        }
    }
    s
}

/// Compute the actual number of terminal-grid cells covered by a shaped
/// glyph's character range.  This is grid-aware: CJK/emoji characters
/// (whose right neighbour is a spacer) count as 2 cells, ASCII as 1.
pub(crate) fn glyph_grid_num_cells(
    grid: &GridView,
    row: usize,
    run_start: usize,
    char_range: &std::ops::Range<usize>,
    cols: usize,
) -> usize {
    let mut total = 0usize;
    for ci in char_range.clone() {
        let col = run_start + ci;
        // If this char's right neighbour is a spacer, it's a wide
        // character (CJK / emoji) and contributes 2 cells.
        if col + 1 < cols {
            if let Some(next) = grid.cell(row, col + 1) {
                if next.is_spacer {
                    total += 2;
                    continue;
                }
            }
        }
        total += 1;
    }
    total
}

/// Detect the end of a consecutive-cell "run" for ligature shaping.
///
/// Starting at `start_col`, walk forward while cells share the same style
/// (same `bold`, `italic` flags) and are not spaces, spacers, or hidden.
/// Returns the first column *past* the run (i.e. `end_col` such that
/// `start_col .. end_col` is the run range).
///
/// When ligature shaping is enabled, the run text is passed to
/// [`GlyphAtlas::shape_and_rasterize_run`] so that OpenType ligature
/// rules (`liga`/`clig`) can substitute multi-cell glyphs (e.g. `->` →
/// one arrow glyph).
///
/// Run boundaries occur at:
///
/// * **End of row** — no more cells.
/// * **Space character** — spaces never participate in ligatures.
/// * **Spacer cell** — a CJK / emoji wide-character continuation.
/// * **Hidden cell** — invisible content should not be shaped.
/// * **Style change** — different `bold` or `italic` flags require
///   separate shaping with different [`cosmic_text::Attrs`].
pub(crate) fn detect_run_end(
    grid: &GridView,
    row: usize,
    start_col: usize,
    cols: usize,
) -> usize {
    let first = match grid.cell(row, start_col) {
        Some(c) => c,
        None => return start_col + 1,
    };

    // Wide characters (CJK, emoji) occupy 2 cells: the character cell
    // followed by a spacer.  Always return them as single-cell runs so
    // that the per-char path handles them with the correct 2-cell width
    // (num_cells from is_spacer check).  If a wide char were part of a
    // ligature run, its strip would get 1-cell width instead.
    if start_col + 1 < cols {
        if let Some(next) = grid.cell(row, start_col + 1) {
            if next.is_spacer {
                return start_col + 1;
            }
        }
    }

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
        // Wide character (CJK / emoji) check: if this cell's right
        // neighbour is a spacer, the cell occupies 2 cells and must
        // form its own single-cell run so that the per-char path
        // handles it with the correct 2-cell width.
        if col + 1 < cols {
            if let Some(next) = grid.cell(row, col + 1) {
                if next.is_spacer {
                    break;
                }
            }
        }
        col += 1;
    }
    col
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── might_ligate ────────────────────────────────────────────

    #[test]
    fn ligate_detects_arrow() {
        assert!(might_ligate("->"));
    }

    #[test]
    fn ligate_detects_not_equals() {
        assert!(might_ligate("!="));
    }

    #[test]
    fn ligate_rejects_plain_text() {
        assert!(!might_ligate("hello"));
    }

    #[test]
    fn ligate_rejects_single_char() {
        assert!(!might_ligate("a"));
    }

    #[test]
    fn ligate_rejects_empty() {
        assert!(!might_ligate(""));
    }

    #[test]
    fn ligate_ignores_non_ligature_punctuation() {
        // Backslash is not a ligature seeding char.
        assert!(!might_ligate(r"\n"));
    }
}
