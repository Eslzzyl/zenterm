//! Hyperlink detection shared by rendering and mouse handling.

use linkify::{LinkFinder, LinkKind};

use zenterm_core::cell::Cell;
use zenterm_term::GridView;

use super::types::{DetectedLink, DetectedLinkKind, LinkSegment, TerminalSession};

/// Schemes accepted for automatic terminal links.
///
/// Default schemes accepted for automatic detection and opening.  Other
/// schemes supported by peer terminals can be added later through an
/// explicit handler policy; they must not reach the OS opener implicitly.
const ALLOWED_LINK_SCHEMES: &[&str] = &["http", "https", "mailto"];

#[derive(Debug, Clone, Copy)]
struct MappedChar {
    row: usize,
    col_start: usize,
    col_end: usize,
}

#[derive(Debug, Default)]
struct MappedText {
    text: String,
    chars: Vec<MappedChar>,
}

fn scheme(target: &str) -> Option<&str> {
    let (candidate, _) = target.split_once(':')?;
    let mut chars = candidate.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return None;
    }
    Some(candidate)
}

fn is_allowed_target(target: &str) -> bool {
    scheme(target).is_some_and(|value| {
        ALLOWED_LINK_SCHEMES
            .iter()
            .any(|allowed| value.eq_ignore_ascii_case(allowed))
    })
}

fn normalize_plain_link(kind: &LinkKind, raw: &str) -> Option<(DetectedLinkKind, String)> {
    match kind {
        LinkKind::Email => Some((DetectedLinkKind::Email, format!("mailto:{raw}"))),
        LinkKind::Url if is_allowed_target(raw) => Some((DetectedLinkKind::Url, raw.to_owned())),
        _ => None,
    }
}

fn normalize_osc8_target(raw: &str) -> Option<String> {
    is_allowed_target(raw).then(|| raw.to_owned())
}

fn row_last_content(cells: &[Cell]) -> Option<usize> {
    cells
        .iter()
        .rposition(|cell| cell.c != ' ' || cell.hyperlink.is_some())
}

/// Build text and a terminal-cell mapping while omitting wide-character
/// spacer cells.  A soft-wrapped row is joined without inserting a newline.
fn build_mapped_text(rows: &[&[Cell]]) -> MappedText {
    let mut mapped = MappedText {
        text: String::new(),
        chars: Vec::new(),
    };

    for (row, cells) in rows.iter().enumerate() {
        if let Some(last) = row_last_content(cells) {
            for col in 0..=last {
                let cell = &cells[col];
                if cell.is_spacer {
                    continue;
                }

                let col_end = if cells.get(col + 1).is_some_and(|next| next.is_spacer) {
                    col + 2
                } else {
                    col + 1
                };
                mapped.text.push(cell.c);
                mapped.chars.push(MappedChar {
                    row,
                    col_start: col,
                    col_end,
                });
            }
        }

        let wraps = cells.last().is_some_and(|cell| cell.is_wrapline);
        if row + 1 < rows.len() && !wraps {
            mapped.text.push('\n');
        }
    }

    mapped
}

fn byte_to_char_index(text: &str, byte_offset: usize) -> usize {
    text[..byte_offset].chars().count()
}

fn segments_for_chars(chars: &[MappedChar]) -> Vec<LinkSegment> {
    let Some(first) = chars.first() else {
        return Vec::new();
    };

    let mut segments = vec![LinkSegment {
        row: first.row,
        col_start: first.col_start,
        col_end: first.col_end,
    }];

    for mapped in chars.iter().skip(1) {
        let previous = segments.last_mut().expect("segment is non-empty");
        if previous.row == mapped.row && previous.col_end == mapped.col_start {
            previous.col_end = mapped.col_end;
        } else {
            segments.push(LinkSegment {
                row: mapped.row,
                col_start: mapped.col_start,
                col_end: mapped.col_end,
            });
        }
    }

    segments
}

fn overlaps_explicit(segments: &[LinkSegment], explicit: &[DetectedLink]) -> bool {
    segments.iter().any(|segment| {
        explicit.iter().any(|link| {
            link.segments.iter().any(|other| {
                segment.row == other.row
                    && segment.col_start < other.col_end
                    && other.col_start < segment.col_end
            })
        })
    })
}

fn detect_osc8_links(rows: &[&[Cell]]) -> Vec<DetectedLink> {
    let mut links = Vec::new();

    for (row, cells) in rows.iter().enumerate() {
        let mut col = 0;
        while col < cells.len() {
            let Some(target) = cells[col].hyperlink.as_deref() else {
                col += 1;
                continue;
            };

            let target = target.to_owned();
            let start = col;
            while col < cells.len() && cells[col].hyperlink.as_deref() == Some(target.as_str()) {
                col += 1;
            }

            let Some(normalized) = normalize_osc8_target(&target) else {
                log::debug!(
                    "link detection: ignored OSC 8 target with disallowed scheme: {target}"
                );
                continue;
            };

            links.push(DetectedLink {
                kind: DetectedLinkKind::Osc8,
                original: target,
                target: normalized,
                segments: vec![LinkSegment {
                    row,
                    col_start: start,
                    col_end: col,
                }],
            });
        }
    }

    links
}

fn detect_plain_links(mapped: &MappedText, explicit: &[DetectedLink]) -> Vec<DetectedLink> {
    let mut finder = LinkFinder::new();
    finder.kinds(&[LinkKind::Url, LinkKind::Email]);
    finder.url_must_have_scheme(true);

    finder
        .links(&mapped.text)
        .filter_map(|link| {
            // A physical newline is a hard line break.  Only soft-wrapped
            // rows are joined in `build_mapped_text`.
            if link.as_str().contains('\n') {
                return None;
            }

            let (kind, target) = match normalize_plain_link(link.kind(), link.as_str()) {
                Some(value) => value,
                None => {
                    log::debug!(
                        "link detection: ignored plain link with disallowed scheme: {}",
                        link.as_str()
                    );
                    return None;
                }
            };
            let start = byte_to_char_index(&mapped.text, link.start());
            let end = byte_to_char_index(&mapped.text, link.end());
            let chars = mapped.chars.get(start..end)?;
            let segments = segments_for_chars(chars);
            if overlaps_explicit(&segments, explicit) {
                return None;
            }

            Some(DetectedLink {
                kind,
                original: link.as_str().to_owned(),
                target,
                segments,
            })
        })
        .collect()
}

fn can_join_soft_wrap(previous: &LinkSegment, next: &LinkSegment, rows: &[&[Cell]]) -> bool {
    previous.row + 1 == next.row
        && next.col_start == 0
        && rows
            .get(previous.row)
            .and_then(|row| row.last())
            .is_some_and(|cell| cell.is_wrapline)
        && rows
            .get(previous.row)
            .is_some_and(|row| previous.col_end == row.len())
}

fn merge_soft_wrapped_osc8_links(
    mut links: Vec<DetectedLink>,
    rows: &[&[Cell]],
) -> Vec<DetectedLink> {
    let mut merged: Vec<DetectedLink> = Vec::with_capacity(links.len());

    for link in links.drain(..) {
        if let Some(previous) = merged.last_mut()
            && previous.kind == link.kind
            && previous.target == link.target
            && can_join_soft_wrap(
                previous.segments.last().expect("link has a segment"),
                link.segments.first().expect("link has a segment"),
                rows,
            )
        {
            previous.segments.extend(link.segments);
        } else {
            merged.push(link);
        }
    }

    merged
}

fn detect_links_from_rows(rows: &[&[Cell]]) -> Vec<DetectedLink> {
    let explicit = merge_soft_wrapped_osc8_links(detect_osc8_links(rows), rows);
    let mapped = build_mapped_text(rows);
    let mut links = explicit.clone();
    links.extend(detect_plain_links(&mapped, &explicit));
    links.sort_by_key(|link| {
        link.segments
            .first()
            .map_or((usize::MAX, usize::MAX), |segment| {
                (segment.row, segment.col_start)
            })
    });
    links
}

fn detect_links(grid: &GridView<'_>) -> Vec<DetectedLink> {
    let rows: Vec<&[Cell]> = grid.rows().collect();
    detect_links_from_rows(&rows)
}

impl TerminalSession {
    pub(crate) fn refresh_detected_links(&mut self) {
        let grid = self.runtime.terminal.visible_cells();
        self.input.detected_links = detect_links(&grid);
        log::debug!(
            "link detection: {} visible links, hover_cell={:?}",
            self.input.detected_links.len(),
            self.input.hover_cell
        );
    }

    pub(crate) fn hovered_link_index(&self) -> Option<usize> {
        self.input.hover_cell.and_then(|(row, col)| {
            self.input
                .detected_links
                .iter()
                .position(|link| link.contains_cell(row, col))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenterm_core::Rgba;

    fn row(text: &str) -> Vec<Cell> {
        let mut cells = Vec::new();
        for character in text.chars() {
            cells.push(Cell::new(character, Rgba::WHITE, Rgba::BLACK));
            if character == '前' {
                let mut spacer = Cell::new(' ', Rgba::WHITE, Rgba::BLACK);
                spacer.is_spacer = true;
                cells.push(spacer);
            }
        }
        cells
    }

    fn links(rows: &[Vec<Cell>]) -> Vec<DetectedLink> {
        let refs: Vec<&[Cell]> = rows.iter().map(Vec::as_slice).collect();
        detect_links_from_rows(&refs)
    }

    #[test]
    fn detects_url_and_normalizes_email() {
        let found = links(&[row("https://example.com foo@example.com")]);

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].kind, DetectedLinkKind::Url);
        assert_eq!(found[0].target, "https://example.com");
        assert_eq!(found[1].kind, DetectedLinkKind::Email);
        assert_eq!(found[1].original, "foo@example.com");
        assert_eq!(found[1].target, "mailto:foo@example.com");
    }

    #[test]
    fn excludes_trailing_punctuation() {
        let found = links(&[row("see (https://example.com/path),")]);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].original, "https://example.com/path");
    }

    #[test]
    fn range_starts_at_link_text() {
        let found = links(&[row("prefix https://example.com")]);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].segments[0].col_start, 7);
        assert_eq!(found[0].segments[0].col_end, 26);
    }

    #[test]
    fn maps_wide_character_cells_without_breaking_url_range() {
        let found = links(&[row("前https://example.com")]);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].segments[0].col_start, 2);
        assert_eq!(found[0].segments[0].col_end, 21);
    }

    #[test]
    fn joins_soft_wrapped_url() {
        let mut first = row("https://example.");
        first.last_mut().expect("non-empty row").is_wrapline = true;
        let found = links(&[first, row("com/path")]);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].original, "https://example.com/path");
        assert_eq!(found[0].segments.len(), 2);
    }

    #[test]
    fn rejects_dangerous_scheme() {
        let found = links(&[row("javascript://evil.example file://C:/secret.txt")]);

        assert!(found.is_empty());
        assert!(!is_allowed_target("javascript://evil.example"));
        assert!(!is_allowed_target("file://C:/secret.txt"));
    }

    #[test]
    fn detects_osc8_target_and_keeps_visible_range() {
        let mut cells = row("open");
        for cell in &mut cells {
            cell.hyperlink = Some("https://example.com".to_owned());
        }

        let found = links(&[cells]);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, DetectedLinkKind::Osc8);
        assert_eq!(found[0].original, "https://example.com");
        assert_eq!(found[0].segments[0].col_start, 0);
        assert_eq!(found[0].segments[0].col_end, 4);
    }
}
