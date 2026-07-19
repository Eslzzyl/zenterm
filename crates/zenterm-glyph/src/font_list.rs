//! System font enumeration using fontdb.
//!
//! Provides cross-platform monospace font discovery.  The underlying fontdb
//! crate scans predefined directories (e.g. `/System/Library/Fonts` on macOS,
//! `~/.fonts` on Linux) and does not interact with OS-level font services.
//! On a modern Mac (M1) the scan takes ≈9 ms for ~900 faces; on Linux with a
//! hot cache it is ≈20 ms for ~1900 faces.
//!
//! All results are deduplicated and sorted.  Call [`list_monospace_families`]
//! once and cache the output — the scan touches the file system on every call.

use std::io::{Read, Seek, SeekFrom};

/// Return all monospace font families installed on the system.
///
/// Each family name appears exactly once, sorted alphabetically.
///
/// fontdb determines monospacing solely from the `post` table's
/// `isFixedPitch` field.  Some fonts (notably Nerd Font patches) lose this
/// flag but still carry the correct proportion in the OS/2 table's panose
/// bytes.  We fall back to checking panose proportion (9 = monospaced)
/// so that patched fonts like "JetBrainsMono Nerd Font" are not missed.
pub fn list_monospace_families() -> Vec<String> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();

    let mut seen_file = std::collections::HashSet::<std::path::PathBuf>::new();
    let mut families: Vec<String> = Vec::new();

    for face in db.faces() {
        if face.monospaced || is_monospaced_by_os2(&face.source, &mut seen_file) {
            if let Some((name, _)) = face.families.first() {
                families.push(name.clone());
            }
        }
    }

    families.sort();
    families.dedup();
    families
}

/// Check whether a font's source is monospaced by reading OS/2 panose.
///
/// Caches results per file path in `seen` to avoid re-reading the same
/// file for every face in a collection (`.ttc`).
fn is_monospaced_by_os2(
    source: &fontdb::Source,
    seen: &mut std::collections::HashSet<std::path::PathBuf>,
) -> bool {
    let path = match source {
        fontdb::Source::File(p) | fontdb::Source::SharedFile(p, _) => p,
        fontdb::Source::Binary(_) => return false,
    };
    if seen.contains(path) {
        return true;
    }
    if check_panose_monospaced(path) {
        seen.insert(path.clone());
        return true;
    }
    false
}

/// Check whether a font file is monospaced by reading its OS/2 table's
/// panose proportion byte.  Returns `true` when the proportion field is 9
/// (Monospaced), `false` otherwise (file not found, unreadable, or truly
/// proportional).
///
/// This reads only the file header + table directory + OS/2 table
/// (≈200 bytes total), so it is cheap to call on many files.
fn check_panose_monospaced(path: &std::path::Path) -> bool {
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };

    // Read sfVersion (4) + numTables (2) = 6 bytes.
    let mut header = [0u8; 6];
    if file.read_exact(&mut header).is_err() {
        return false;
    }

    let num_tables = u16::from_be_bytes([header[4], header[5]]) as usize;
    // Table directory starts at offset 12, after the 12-byte offset table.
    if file.seek(SeekFrom::Start(12)).is_err() {
        return false;
    }
    // Clamp to a reasonable maximum (256 tables) to avoid OOM on corrupt files.
    if num_tables > 256 {
        return false;
    }
    let dir_size = num_tables * 16;
    let mut table_dir = vec![0u8; dir_size];
    if file.read_exact(&mut table_dir).is_err() {
        return false;
    }

    // Walk the directory looking for "OS/2".
    for entry in table_dir.chunks_exact(16) {
        let tag = &entry[0..4];
        if tag != b"OS/2" {
            continue;
        }
        let offset = u32::from_be_bytes([
            entry[8],
            entry[9],
            entry[10],
            entry[11],
        ]);
        let length = u32::from_be_bytes([
            entry[12],
            entry[13],
            entry[14],
            entry[15],
        ]);
        // Need at least 36 bytes to reach the panose proportion byte.
        if length < 36 {
            return false;
        }

        // Seek to OS/2 table and read panose-proportion byte.
        if file.seek(SeekFrom::Start(offset as u64)).is_err() {
            return false;
        }
        // Panose is 10 bytes at offset 32 inside OS/2 table.
        // Proportion is panose[3] (0-indexed), so offset 32 + 3 = 35.
        let mut proportion = [0u8; 1];
        if file.seek(SeekFrom::Start(offset as u64 + 35)).is_err() {
            return false;
        }
        if file.read_exact(&mut proportion).is_err() {
            return false;
        }
        return proportion[0] == 9;
    }

    false // no OS/2 table found
}

/// Describes where a font face lives on disk and which face index to use
/// (relevant for TrueType Collections / `.ttc` files).
#[derive(Debug)]
pub struct FontSource {
    /// Path to the font file on disk.
    pub path: std::path::PathBuf,
    /// Face index inside a font collection (0 for simple `.ttf`/`.otf`).
    pub index: u32,
}

/// Find the file path and face index for the best face matching `family_name`.
///
/// Prefers a Regular‑weight, Normal‑style face.  Falls back to the first
/// face that matches.
///
/// Returns `None` when the family is unknown or when every matching face
/// comes from in-memory data rather than a file.
pub fn find_font_source(db: &fontdb::Database, family_name: &str) -> Option<FontSource> {
    // Collect all faces belonging to the family.
    let faces: Vec<&fontdb::FaceInfo> = db
        .faces()
        .filter(|face| face.families.iter().any(|(name, _)| name == family_name))
        .collect();

    if faces.is_empty() {
        return None;
    }

    // Prefer the Regular/Normal face; fall back to the first one.
    let face = faces
        .iter()
        .copied()
        .min_by_key(|f| {
            // Lower score = better match.
            let weight_penalty = if f.weight == fontdb::Weight::NORMAL { 0 } else { 100 };
            let style_penalty = if f.style == fontdb::Style::Normal { 0 } else { 10 };
            weight_penalty + style_penalty
        })
        .unwrap_or(faces[0]);

    match &face.source {
        fontdb::Source::File(path) | fontdb::Source::SharedFile(path, _) => {
            Some(FontSource {
                path: path.clone(),
                index: face.index,
            })
        }
        fontdb::Source::Binary(_) => None,
    }
}

/// Check that the font data at `index` is parseable and has a usable
/// `units_per_em` (> 0).  egui internally uses `skrifa` to lay out text;
/// some font files that pass `ttf-parser` (used by fontdb) still produce a
/// zero `units_per_em` when interpreted by skrifa, which causes a
/// division-by-zero panic in epaint's `GlyphCacheKey`.
///
/// Call this *before* passing the raw bytes to
/// [`egui::FontData`] to pre-emptively reject problematic fonts.
pub fn validate_font_data(data: &[u8], index: u32) -> bool {
    match ttf_parser::Face::parse(data, index) {
        Ok(face) => face.units_per_em() > 0,
        Err(_) => false,
    }
}
pub fn find_font_path(db: &fontdb::Database, family_name: &str) -> Option<std::path::PathBuf> {
    let src = find_font_source(db, family_name)?;
    Some(src.path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monospace_list_is_nonempty() {
        let families = list_monospace_families();
        assert!(!families.is_empty(), "expected at least one monospace font");
        // macOS always ships Menlo.
        assert!(
            families.iter().any(|f| f.contains("Menlo")),
            "Menlo should be present on macOS: {:?}",
            families
        );
    }

    #[test]
    fn find_font_path_works() {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let path = find_font_path(&db, "Menlo");
        assert!(path.is_some(), "Menlo should have a file path");
        assert!(
            path.as_ref().unwrap().exists(),
            "path should exist: {:?}",
            path
        );
    }

    #[test]
    fn find_font_source_has_correct_index() {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let src = find_font_source(&db, "Menlo");
        assert!(src.is_some(), "Menlo should have a font source");
        // Menlo.ttc normally has Regular at index 0.
        assert_eq!(src.unwrap().index, 0, "Menlo Regular should be index 0");
    }
}
