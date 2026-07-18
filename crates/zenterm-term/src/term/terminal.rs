//! Terminal state machine and public API.
//!
//! Wraps [`alacritty_terminal::Term`] + [`vte::ansi::Processor`] and provides
//! methods for feeding bytes, resizing, scrolling, and reading the grid.

use std::collections::HashMap;
use std::sync::{mpsc, Arc};

use alacritty_terminal::event::{Event, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Direction, Line, Point};

use zenterm_core::image::ImageCell;
use zenterm_core::image::{ImageData, ImageDataType};

use crate::image::kitty::{self, KittyAccumulator, KittyImage};
use crate::image::sixel::{self, SixelBuilder};
use crate::image::{PlacementParams, PlacementStyle, assign_image_to_cells};
use crate::image::ImageCache;
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{ClipboardType, Config as TermConfig, Term, TermDamage, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor};

use zenterm_core::cell::{Cell, UnderlineStyle};
use zenterm_core::color::Rgba;
use zenterm_core::damage::DamageSet;
use zenterm_core::position::TermPos;
use zenterm_core::size::TermSize;
use zenterm_core::{
    ITermDimension, ITermFileData, ITermProprietary, ITermUnicodeVersionOp,
    KittyNotification, Progress, SemanticPrompt,
};

use super::color_scheme::{named_color_default_rgb, ColorScheme};
use super::grid_view::{CursorInfo, GridView};
use super::listener::Listener;
use super::osc::{
    parse_conemu_progress, parse_osc133, parse_iterm_proprietary, scan_oscs,
    KittyNotificationState,
};
use super::TermDimensions;

// ── Unicode placeholder support (Kitty U=1) ────────────────────────────

/// Codepoint used as the Unicode placeholder character.
const PLACEHOLDER_CHAR: char = '\u{10EEEE}';

/// Row/column diacritics for encoding position in Unicode placeholders.
/// From Kitty: https://sw.kovidgoyal.net/kitty/_downloads/1792bad15b12979994cd6ecc54c967a6/rowcolumn-diacritics.txt
/// The index into the array determines the value.
const DIACRITICS: &[char] = &[
    '\u{305}', '\u{30D}', '\u{30E}', '\u{310}', '\u{312}',
    '\u{33D}', '\u{33E}', '\u{33F}', '\u{346}', '\u{34A}',
    '\u{34B}', '\u{34C}', '\u{350}', '\u{351}', '\u{352}',
    '\u{357}', '\u{35B}', '\u{363}', '\u{364}', '\u{365}',
    '\u{366}', '\u{367}', '\u{368}', '\u{369}', '\u{36A}',
    '\u{36B}', '\u{36C}', '\u{36D}', '\u{36E}', '\u{36F}',
    '\u{483}', '\u{484}', '\u{485}', '\u{486}', '\u{487}',
    '\u{592}', '\u{593}', '\u{594}', '\u{595}', '\u{597}',
    '\u{598}', '\u{599}', '\u{59C}', '\u{59D}', '\u{59E}',
    '\u{59F}', '\u{5A0}', '\u{5A1}', '\u{5A8}', '\u{5A9}',
    '\u{5AB}', '\u{5AC}', '\u{5AF}', '\u{5C4}', '\u{610}',
    '\u{611}', '\u{612}', '\u{613}', '\u{614}', '\u{615}',
    '\u{616}', '\u{617}', '\u{657}', '\u{658}', '\u{659}',
    '\u{65A}', '\u{65B}', '\u{65D}', '\u{65E}', '\u{6D6}',
    '\u{6D7}', '\u{6D8}', '\u{6D9}', '\u{6DA}', '\u{6DB}',
    '\u{6DC}', '\u{6DF}', '\u{6E0}', '\u{6E1}', '\u{6E2}',
    '\u{6E4}', '\u{6E7}', '\u{6E8}', '\u{6EB}', '\u{6EC}',
    '\u{730}', '\u{732}', '\u{733}', '\u{735}', '\u{736}',
    '\u{73A}', '\u{73D}', '\u{73F}', '\u{740}', '\u{741}',
    '\u{743}', '\u{745}', '\u{747}', '\u{749}', '\u{74A}',
    '\u{7EB}', '\u{7EC}', '\u{7ED}', '\u{7EE}', '\u{7EF}',
    '\u{7F0}', '\u{7F1}', '\u{7F3}', '\u{816}', '\u{817}',
    '\u{818}', '\u{819}', '\u{81B}', '\u{81C}', '\u{81D}',
    '\u{81E}', '\u{81F}', '\u{820}', '\u{821}', '\u{822}',
    '\u{823}', '\u{825}', '\u{826}', '\u{827}', '\u{829}',
    '\u{82A}', '\u{82B}', '\u{82C}', '\u{82D}', '\u{951}',
    '\u{953}', '\u{954}', '\u{F82}', '\u{F83}', '\u{F86}',
    '\u{F87}', '\u{135D}', '\u{135E}', '\u{135F}', '\u{17DD}',
    '\u{193A}', '\u{1A17}', '\u{1A75}', '\u{1A76}', '\u{1A77}',
    '\u{1A78}', '\u{1A79}', '\u{1A7A}', '\u{1A7B}', '\u{1A7C}',
    '\u{1B6B}', '\u{1B6D}', '\u{1B6E}', '\u{1B6F}', '\u{1B70}',
    '\u{1B71}', '\u{1B72}', '\u{1B73}', '\u{1CD0}', '\u{1CD1}',
    '\u{1CD2}', '\u{1CDA}', '\u{1CDB}', '\u{1CE0}', '\u{1DC0}',
    '\u{1DC1}', '\u{1DC3}', '\u{1DC4}', '\u{1DC5}', '\u{1DC6}',
    '\u{1DC7}', '\u{1DC8}', '\u{1DC9}', '\u{1DCB}', '\u{1DCC}',
    '\u{1DD1}', '\u{1DD2}', '\u{1DD3}', '\u{1DD4}', '\u{1DD5}',
    '\u{1DD6}', '\u{1DD7}', '\u{1DD8}', '\u{1DD9}', '\u{1DDA}',
    '\u{1DDB}', '\u{1DDC}', '\u{1DDD}', '\u{1DDE}', '\u{1DDF}',
    '\u{1DE0}', '\u{1DE1}', '\u{1DE2}', '\u{1DE3}', '\u{1DE4}',
    '\u{1DE5}', '\u{1DE6}', '\u{1DFE}', '\u{20D0}', '\u{20D1}',
    '\u{20D4}', '\u{20D5}', '\u{20D6}', '\u{20D7}', '\u{20DB}',
    '\u{20DC}', '\u{20E1}', '\u{20E7}', '\u{20E9}', '\u{20F0}',
    '\u{2CEF}', '\u{2CF0}', '\u{2CF1}', '\u{2DE0}', '\u{2DE1}',
    '\u{2DE2}', '\u{2DE3}', '\u{2DE4}', '\u{2DE5}', '\u{2DE6}',
    '\u{2DE7}', '\u{2DE8}', '\u{2DE9}', '\u{2DEA}', '\u{2DEB}',
    '\u{2DEC}', '\u{2DED}', '\u{2DEE}', '\u{2DEF}', '\u{2DF0}',
    '\u{2DF1}', '\u{2DF2}', '\u{2DF3}', '\u{2DF4}', '\u{2DF5}',
    '\u{2DF6}', '\u{2DF7}', '\u{2DF8}', '\u{2DF9}', '\u{2DFA}',
    '\u{2DFB}', '\u{2DFC}', '\u{2DFD}', '\u{2DFE}', '\u{2DFF}',
    '\u{A66F}', '\u{A67C}', '\u{A67D}', '\u{A6F0}', '\u{A6F1}',
    '\u{A8E0}', '\u{A8E1}', '\u{A8E2}', '\u{A8E3}', '\u{A8E4}',
    '\u{A8E5}', '\u{A8E6}', '\u{A8E7}', '\u{A8E8}', '\u{A8E9}',
    '\u{A8EA}', '\u{A8EB}', '\u{A8EC}', '\u{A8ED}', '\u{A8EE}',
    '\u{A8EF}', '\u{A8F0}', '\u{A8F1}', '\u{AAB0}', '\u{AAB2}',
    '\u{AAB3}', '\u{AAB7}', '\u{AAB8}', '\u{AABE}', '\u{AABF}',
    '\u{AAC1}', '\u{FE20}', '\u{FE21}', '\u{FE22}', '\u{FE23}',
    '\u{FE24}', '\u{FE25}', '\u{FE26}', '\u{10A0F}', '\u{10A38}',
    '\u{1D185}', '\u{1D186}', '\u{1D187}', '\u{1D188}', '\u{1D189}',
    '\u{1D1AA}', '\u{1D1AB}', '\u{1D1AC}', '\u{1D1AD}', '\u{1D242}',
    '\u{1D243}', '\u{1D244}',
];

/// Decode a diacritic combining mark into a numeric value.
/// Returns `None` if the character is not in the diacritics table.
fn diacritic_value(c: char) -> Option<u32> {
    DIACRITICS.iter().position(|&d| d == c).map(|i| i as u32)
}

/// A virtual placement created by `U=1`.
///
/// Stores the metadata needed to render image slices when the application
/// writes `U+10EEEE` placeholder characters to the grid.
#[derive(Debug, Clone)]
pub struct VirtualPlacement {
    /// The image ID this placement refers to.
    pub image_id: u32,
    /// The placement ID (optional).
    pub placement_id: Option<u32>,
    /// Number of columns this placement spans.
    pub columns: u32,
    /// Number of rows this placement spans.
    pub rows: u32,
    /// Source x in pixels (Kitty `x=`).
    pub source_x: Option<u32>,
    /// Source y in pixels (Kitty `y=`).
    pub source_y: Option<u32>,
    /// Source width in pixels (Kitty `w=`).
    pub source_w: Option<u32>,
    /// Source height in pixels (Kitty `h=`).
    pub source_h: Option<u32>,
    /// Cell x-offset (Kitty `X=`).
    pub x_offset: Option<u32>,
    /// Cell y-offset (Kitty `Y=`).
    pub y_offset: Option<u32>,
    /// Z-index for compositing.
    pub z_index: i32,
}

/// The terminal state machine.
///
/// Owns `alacritty_terminal::Term` for grid state and `vte::ansi::Processor`
/// for byte processing.
pub struct Terminal {
    term: Term<Listener>,
    rx: mpsc::Receiver<Event>,
    processor: Processor,
    damage: DamageSet,
    scheme: ColorScheme,
    grid_cache: Vec<Vec<Cell>>,

    // ── Image protocol state ────────────────────────────────────────────
    pub(crate) image_cache: ImageCache,
    /// Hashes of images that were removed and whose GPU atlas slots need
    /// to be freed.  Drained by the UI layer each frame.
    pub pending_image_deallocations: Vec<[u8; 32]>,
    /// Image placements keyed by grid (line, col) so they follow content
    /// during scroll.  `line` is a grid-relative `Line.0` (may be negative
    /// when viewport is at bottom).
    pub(crate) image_placements: HashMap<(i32, usize), ImageCell>,
    /// Virtual placements created by `U=1` (Unicode placeholder mode).
    /// Keyed by `(image_id, placement_id)`.  At render time, cells containing
    /// `U+10EEEE` are matched against these entries to determine which image
    /// slice to display.
    pub(crate) virtual_placements: HashMap<(u32, Option<u32>), VirtualPlacement>,
    /// Accumulator for multi-chunk Kitty image transmissions.
    #[allow(dead_code)]
    kitty_accumulator: KittyAccumulator,
    /// Buffered bytes from an APC sequence that spans across `feed()` calls.
    /// When the APC scanner finds `ESC _ G` but cannot find the ST (`ESC \`)
    /// within the current batch, the bytes from `ESC _ G` onward are saved
    /// here and prepended to the next `feed()` call.
    apc_remainder: Vec<u8>,
    /// Cell pixel dimensions (set by the UI layer).
    pub cell_pixel_width: u32,
    pub cell_pixel_height: u32,

    // ── Total text-area pixel dimensions ───────────────────────────
    /// Total text-area width in pixels (set by the UI layer on resize).
    pub pixel_width: u32,
    /// Total text-area height in pixels (set by the UI layer on resize).
    pub pixel_height: u32,

    // ── Pending side-effects (consumed by the app after each feed()) ────
    pending_title: Option<String>,
    pending_bell: bool,
    pending_exit: bool,
    pending_child_exit: Option<std::process::ExitStatus>,
    pending_clipboard_store: Option<String>,
    pending_clipboard_load: Option<Arc<dyn Fn(&str) -> String + Sync + Send + 'static>>,
    /// Most recent OSC 7 working-directory URL (e.g. `file://host/path`).
    /// Populated by [`Self::feed`] by scanning the input stream for
    /// `\x1b]7;…\x07` / `\x1b]7;…\x1b\\` sequences.  Consumed via
    /// [`Self::take_current_directory`].
    pending_current_directory: Option<String>,
    /// Most recent OSC 9 / OSC 777 desktop notification.
    /// Populated by [`Self::feed`]; consumed via [`Self::take_notification`].
    pending_notification: Option<(String, String)>,
    /// Most recent ConEmu OSC 9;4 progress-bar state.
    /// Populated by [`Self::feed`]; consumed via [`Self::take_progress`].
    pending_progress: Option<Progress>,
    /// Most recent FinalTerm OSC 133 semantic prompt marker.
    /// Populated by [`Self::feed`]; consumed via [`Self::take_semantic_prompt`].
    pending_semantic_prompt: Option<SemanticPrompt>,
    /// Flag indicating a fresh-line (\r\n) should be injected before the
    /// next batch of PTY bytes.  Set by OSC 133 commands L, A, N.
    pending_fresh_line: bool,
    /// Kitty OSC 99 notification state — manages chunked notification
    /// accumulation and query responses.
    kitty_state: KittyNotificationState,
    /// Most recent completed Kitty OSC 99 notification.
    /// Populated by [`Self::feed`]; consumed via [`Self::take_kitty_notification`].
    pending_kitty_notification: Option<KittyNotification>,

    // ── OSC 1337 (iTerm2 proprietary) state ─────────────────────────
    /// Pending iTerm2 proprietary action for the UI layer.
    /// Populated by [`Self::feed`]; consumed via [`Self::take_iterm_action`].
    pending_iterm_action: Option<ITermProprietary>,
    /// User-defined variables set via `OSC 1337;SetUserVar=…`.
    pub(crate) user_vars: HashMap<String, String>,
    /// Current Unicode version.
    unicode_version: u8,
    /// Stack of (version, optional_label) for `UnicodeVersion=push/pop`.
    unicode_version_stack: Vec<(u8, Option<String>)>,
    /// Navigation marks recorded via `OSC 1337;SetMark`.
    /// Each entry is `(column, viewport_line)`.
    marks: Vec<(usize, usize)>,
    /// Auto-incrementing number for iTerm2 inline image cache entries.
    next_iterm_image_number: u32,
}

impl Terminal {
    /// Create a new terminal with the given dimensions.
    pub fn new(size: TermSize, scheme: ColorScheme) -> Self {
        let config = TermConfig::default();
        let dim = TermDimensions(size);

        // Create the event channel and listener — this replaces the previous
        // `VoidListener` so that terminal queries (DA, DSR, DECRPM, OSC
        // colour queries, …) are properly answered.
        let (tx, rx) = mpsc::channel();
        let listener = Listener { tx };
        let term = Term::new(config, &dim, listener);

        let cols = dim.columns();
        let rows = dim.screen_lines();

        Self {
            term,
            rx,
            processor: Processor::new(),
            damage: DamageSet::new(rows),
            scheme,
            grid_cache: vec![vec![Cell::blank(); cols]; rows],
            image_cache: ImageCache::new(),
            image_placements: HashMap::new(),
            virtual_placements: HashMap::new(),
            pending_image_deallocations: Vec::new(),
            kitty_accumulator: KittyAccumulator::default(),
            apc_remainder: Vec::new(),
            cell_pixel_width: 0,
            cell_pixel_height: 0,
            pixel_width: size.pixel_width as u32,
            pixel_height: size.pixel_height as u32,
            pending_title: None,
            pending_bell: false,
            pending_exit: false,
            pending_child_exit: None,
            pending_clipboard_store: None,
            pending_clipboard_load: None,
            pending_current_directory: None,
            pending_notification: None,
            pending_progress: None,
            pending_semantic_prompt: None,
            pending_fresh_line: false,
            kitty_state: KittyNotificationState::default(),
            pending_kitty_notification: None,
            pending_iterm_action: None,
            user_vars: HashMap::new(),
            unicode_version: 0,
            unicode_version_stack: Vec::new(),
            marks: Vec::new(),
            next_iterm_image_number: 1,
        }
    }

    /// Feed raw bytes from the PTY into the VT processor.
    ///
    /// The processor calls `Handler` methods on the inner `Term`, updating
    /// grid state.  Damage is propagated from `alacritty_terminal`'s
    /// internal tracking so only changed rows are re-resolved.
    ///
    /// Returns response bytes that the caller **must** write back to the PTY
    /// (terminal query replies such as DA, DSR, DECRPM, OSC colour reports,
    /// clipboard load, …).  Other side-effects (title changes, bell, exit,
    /// clipboard store) are stored internally and can be retrieved via the
    /// `take_*` methods after this call.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        // Prepend any leftover bytes from an APC that spanned the previous feed() call.
        let mut combined;
        let bytes: &[u8] = if self.apc_remainder.is_empty() {
            bytes
        } else {
            log::debug!(
                "[img] prepending {} APC remainder bytes to new batch",
                self.apc_remainder.len(),
            );
            combined = std::mem::take(&mut self.apc_remainder);
            combined.extend_from_slice(bytes);
            combined.as_slice()
        };
        if bytes.is_empty() {
            return Vec::new();
        }
        let start = std::time::Instant::now();
        log::debug!("Terminal::feed: {} bytes", bytes.len());

        // Response bytes collected during processing; written back to PTY.
        let mut replies = Vec::new();

        // ── APC / DCS scan ──────────────────────────────────────────────
        // Use memchr to efficiently find ESC bytes (0x1b) that start APC
        // (ESC _ G ... ST) and DCS (ESC P ... ST) sequences, instead of
        // scanning byte-by-byte which is O(n²) in the naive loop.
        let t_apc_start = std::time::Instant::now();
        let esc_positions = memchr::memchr_iter(0x1b, bytes);
        let mut prev_end: Option<usize> = None;
        let mut apc_count: usize = 0;
        for esc_pos in esc_positions {
            // Skip positions we've already consumed as part of a prior match.
            if prev_end.is_some_and(|end| esc_pos < end) {
                continue;
            }
            if esc_pos + 2 >= bytes.len() {
                // Not enough bytes to check ESC _ G — buffer trailing bytes
                // (they may be the start of an APC spanning the next batch).
                self.apc_remainder.clear();
                self.apc_remainder.extend_from_slice(&bytes[esc_pos..]);
                break;
            }
            // Check for APC: ESC _ G
            if bytes[esc_pos + 1] == b'_' && bytes[esc_pos + 2] == b'G' {
                let payload_start = esc_pos + 2;
                // Find the string terminator ST: ESC \
                // Use SIMD-optimized memchr instead of windows(2) for ~5x faster search.
                let st_rel = memchr::memchr(0x1b, &bytes[payload_start..])
                    .and_then(|rel| {
                        let abs = payload_start + rel;
                        if abs + 1 < bytes.len() && bytes[abs + 1] == b'\\' {
                            Some(rel)
                        } else {
                            None
                        }
                    });
                if let Some(st_rel) = st_rel {
                    let payload = &bytes[payload_start..payload_start + st_rel];
                    if let Some(cmd) = KittyImage::parse_apc(payload) {
                        let reply = self.handle_kitty_command(cmd);
                        if let Some(r) = reply {
                            log::debug!(
                                "[img] Kitty query response: {} bytes",
                                r.len(),
                            );
                            replies.extend_from_slice(r.as_bytes());
                        }
                    } else {
                        log::warn!(
                            "[img] Kitty APC parse FAILED, first 80 bytes: {:?}",
                            String::from_utf8_lossy(&payload[..payload.len().min(80)]),
                        );
                    }
                    apc_count += 1;
                    prev_end = Some(payload_start + st_rel + 2);
                } else {
                    // ST not found — the APC may span the batch boundary.
                    // Save bytes from ESC _ G onward for the next feed() call.
                    // Break the loop since all remaining bytes belong to this
                    // incomplete APC (base64 data contains no ESC bytes).
                    log::warn!(
                        "[img] APC ST not found at offset={} ({} bytes from ESC _ G), \
                         buffering for next feed()",
                        esc_pos,
                        bytes.len() - esc_pos,
                    );
                    self.apc_remainder.clear();
                    self.apc_remainder.extend_from_slice(&bytes[esc_pos..]);
                    break;
                }
            }
            // Check for DCS: ESC P (Sixel)
            if bytes[esc_pos + 1] == b'P' {
                let param_start = esc_pos + 2;
                let mut j = param_start;
                while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b';') {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b'q' {
                    let payload_start = j + 1;
                    let st_rel = memchr::memchr(0x1b, &bytes[payload_start..])
                        .and_then(|rel| {
                            let abs = payload_start + rel;
                            if abs + 1 < bytes.len() && bytes[abs + 1] == b'\\' {
                                Some(rel)
                            } else {
                                None
                            }
                        });
                    if let Some(st_rel) = st_rel {
                        let params = sixel::parse_dcs_params(&bytes[param_start..j]);
                        self.handle_sixel(&bytes[payload_start..payload_start + st_rel], &params);
                        prev_end = Some(payload_start + st_rel + 2);
                    }
                }
            }
            // Check for CSI 16 t (Report Cell Size in pixels).
            // vte 0.15.0 does not dispatch param=16 for final byte 't',
            // so we handle it here directly.
            if bytes[esc_pos + 1] == b'[' {
                // ── CSI 2 J : Erase Display — clear image placements ──
                if esc_pos + 3 < bytes.len()
                    && bytes[esc_pos + 2] == b'2'
                    && bytes[esc_pos + 3] == b'J'
                {
                    if !self.image_placements.is_empty() || !self.virtual_placements.is_empty() {
                        log::debug!(
                            "[img] CSI 2J (Erase Display): clearing {} image placements, {} virtual placements",
                            self.image_placements.len(),
                            self.virtual_placements.len(),
                        );
                        self.image_placements.clear();
                        self.virtual_placements.clear();
                    }
                }

                let mut j = esc_pos + 2;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b't' && j > esc_pos + 2 {
                    if let Ok(param_str) = std::str::from_utf8(&bytes[esc_pos + 2..j]) {
                        if param_str == "16" {
                            let cols = self.term.columns();
                            let rows = self.term.screen_lines();
                            let cell_w = if cols > 0 { self.pixel_width / cols as u32 } else { 0 };
                            let cell_h = if rows > 0 { self.pixel_height / rows as u32 } else { 0 };
                            let response = format!("\x1b[6;{};{}t", cell_h, cell_w);
                            log::info!(
                                "[img] CSI 16t response: cell_w={cell_w}, cell_h={cell_h}, pixel={}x{}, grid={}x{}",
                                self.pixel_width, self.pixel_height, cols, rows,
                            );
                            replies.extend_from_slice(response.as_bytes());
                            prev_end = Some(j + 1);
                        }
                    }
                }
            }
        }
        log::debug!(
            "[img] APC scan: batch_len={}, apc_count={}, elapsed={:?}",
            bytes.len(), apc_count, t_apc_start.elapsed(),
        );
        let t_apc_elapsed = t_apc_start.elapsed();

        // ── Unified OSC scan ─────────────────────────────────────────
        // Collect all OSC sequences; they are handled below AFTER the
        // VT parser has processed the corresponding bytes, so that
        // cursor-dependent handlers (e.g. iTerm2 inline image) see the
        // correct cursor position.
        let t_osc_start = std::time::Instant::now();
        let oscs = scan_oscs(bytes);
        let t_osc_elapsed = t_osc_start.elapsed();

        // ── Fresh-line injection ─────────────────────────────────────────
        // OSC 133 commands L, A, and N signal that the terminal should
        // perform a fresh line (\r\n) before processing subsequent output.
        let injected_vec;
        let vt_bytes: &[u8] = if self.pending_fresh_line {
            self.pending_fresh_line = false;
            injected_vec = {
                let mut v = Vec::with_capacity(2 + bytes.len());
                v.push(b'\r');
                v.push(b'\n');
                v.extend_from_slice(bytes);
                v
            };
            &injected_vec
        } else {
            bytes
        };

        // ── VT parser + OSC dispatch (interleaved) ──────────────────────
        // Process the byte stream incrementally so each OSC handler sees
        // the terminal state (cursor position etc.) AFTER the bytes that
        // precede the OSC have been parsed by the VT parser.
        let t_vt_start = std::time::Instant::now();
        let shift = vt_bytes.len() - bytes.len();
        let mut prev_vt_off = 0;

        for osc in &oscs {
            let vt_osc_start = osc.byte_start + shift;
            let vt_osc_end = osc.byte_end + shift;

            // Process bytes before this OSC (cursor positioning, text, etc.).
            if vt_osc_start > prev_vt_off {
                self.processor.advance(&mut self.term, &vt_bytes[prev_vt_off..vt_osc_start]);
            }

            // Dispatch the OSC — cursor/grid state now reflects the prefix
            // bytes processed above.
            match osc.number {
                7 => {
                    // OSC 7 — current working directory.
                    self.pending_current_directory = Some(osc.payload.clone());
                }
                9 => {
                    // OSC 9 — iTerm2 notification OR ConEmu progress bar.
                    if let Some(prog) = parse_conemu_progress(&osc.payload) {
                        self.pending_progress = Some(prog);
                    } else {
                        // iTerm2-style notification.
                        self.pending_notification =
                            Some(("Zenterm".into(), osc.payload.clone()));
                    }
                }
                777 => {
                    // OSC 777 — rxvt notification (format: notify;title;body).
                    let mut parts = osc.payload.splitn(3, ';');
                    let _maybe_notify = parts.next(); // "notify"
                    let title = parts.next().unwrap_or("").to_string();
                    let body = parts.next().unwrap_or("").to_string();
                    self.pending_notification = Some((title, body));
                }
                99 => {
                    // OSC 99 — Kitty desktop notification.
                    let (notification, response) = self.kitty_state
                        .handle_event(&osc.payload, "");
                    if let Some(notif) = notification {
                        // Reuse the existing notification channel for display.
                        let title = if notif.title.is_empty() {
                            notif.body.clone()
                        } else {
                            notif.title.clone()
                        };
                        let body = if notif.title.is_empty() {
                            String::new()
                        } else {
                            notif.body.clone()
                        };
                        self.pending_notification = Some((title, body));
                        self.pending_kitty_notification = Some(notif);
                    }
                    if let Some(resp) = response {
                        log::debug!("Terminal::feed: OSC 99 response: {resp}");
                        replies.extend_from_slice(resp.as_bytes());
                    }
                }
                133 => {
                    // OSC 133 — FinalTerm semantic prompt.
                    if let Some(prompt) = parse_osc133(&osc.payload) {
                        // Commands L, A, N imply a fresh line.
                        match &prompt {
                            SemanticPrompt::FreshLine
                            | SemanticPrompt::FreshLineAndStartPrompt { .. }
                            | SemanticPrompt::MarkEndOfCommandWithFreshLine { .. } => {
                                self.pending_fresh_line = true;
                            }
                            _ => {}
                        }
                        self.pending_semantic_prompt = Some(prompt);
                    }
                }
                1337 => {
                    // OSC 1337 — iTerm2 proprietary.
                    if let Some(cmd) = parse_iterm_proprietary(&osc.payload) {
                        match cmd {
                            ITermProprietary::SetMark => {
                                let cursor = self.cursor();
                                self.marks.push((cursor.pos.column, cursor.pos.line));
                                log::debug!(
                                    "SetMark: mark recorded at ({}, {})",
                                    cursor.pos.column,
                                    cursor.pos.line,
                                );
                            }
                            ITermProprietary::StealFocus => {
                                self.pending_iterm_action =
                                    Some(ITermProprietary::StealFocus);
                            }
                            ITermProprietary::ClearScrollback => {
                                self.term.grid_mut().clear_history();
                                self.damage.mark_all();
                            }
                            ITermProprietary::CurrentDir(path) => {
                                self.pending_current_directory = Some(path);
                            }
                            ITermProprietary::SetProfile(name) => {
                                self.pending_iterm_action =
                                    Some(ITermProprietary::SetProfile(name));
                            }
                            ITermProprietary::HighlightCursorLine(enabled) => {
                                self.pending_iterm_action =
                                    Some(ITermProprietary::HighlightCursorLine(enabled));
                            }
                            ITermProprietary::RequestCellSize => {
                                if self.cell_pixel_width > 0
                                    && self.cell_pixel_height > 0
                                {
                                    let w = self.cell_pixel_width as f32;
                                    let h = self.cell_pixel_height as f32;
                                    let response = format!(
                                        "\x1b]1337;ReportCellSize={h};{w}\x1b\\"
                                    );
                                    log::debug!(
                                        "Terminal::feed: OSC 1337 RequestCellSize \
                                         response: {response}"
                                    );
                                    replies.extend_from_slice(response.as_bytes());
                                }
                            }
                            ITermProprietary::ReportCellSize { .. } => {}
                            ITermProprietary::Copy(text) => {
                                self.pending_clipboard_store = Some(text);
                            }
                            ITermProprietary::ReportVariable(name) => {
                                let value = self
                                    .iterm_builtin_var(&name)
                                    .or_else(|| self.user_vars.get(&name).cloned());
                                let response = if let Some(val) = value {
                                    let b64_val =
                                        crate::term::osc::base64_encode_for_response(
                                            val.as_bytes(),
                                        );
                                    let b64_name =
                                        crate::term::osc::base64_encode_for_response(
                                            name.as_bytes(),
                                        );
                                    format!(
                                        "\x1b]1337;ReportVariable={b64_name}={b64_val}\x1b\\"
                                    )
                                } else {
                                    let b64_name =
                                        crate::term::osc::base64_encode_for_response(
                                            name.as_bytes(),
                                        );
                                    format!(
                                        "\x1b]1337;ReportVariable={b64_name}=\x1b\\"
                                    )
                                };
                                log::debug!(
                                    "Terminal::feed: OSC 1337 ReportVariable({name}) \
                                     response: {response}"
                                );
                                replies.extend_from_slice(response.as_bytes());
                            }
                            ITermProprietary::SetUserVar { name, value } => {
                                self.user_vars.insert(name, value);
                            }
                            ITermProprietary::SetBadgeFormat(format) => {
                                self.pending_iterm_action =
                                    Some(ITermProprietary::SetBadgeFormat(format));
                            }
                            ITermProprietary::File(file_data) => {
                                if file_data.inline {
                                    // Cursor position is now correct because
                                    // the VT parser has processed all bytes
                                    // preceding this OSC in the buffer.
                                    self.handle_iterm_inline_image(file_data);
                                } else {
                                    self.pending_iterm_action =
                                        Some(ITermProprietary::File(file_data));
                                }
                            }
                            ITermProprietary::UnicodeVersion(op) => {
                                match op {
                                    ITermUnicodeVersionOp::Set(n) => {
                                        self.unicode_version = n;
                                    }
                                    ITermUnicodeVersionOp::Push(label) => {
                                        self.unicode_version_stack.push((
                                            self.unicode_version,
                                            label,
                                        ));
                                    }
                                    ITermUnicodeVersionOp::Pop(label) => {
                                        if let Some(l) = label {
                                            while let Some((ver, ol)) =
                                                self.unicode_version_stack.pop()
                                            {
                                                self.unicode_version = ver;
                                                if ol == Some(l.clone()) {
                                                    break;
                                                }
                                            }
                                        } else {
                                            if let Some((ver, _)) =
                                                self.unicode_version_stack.pop()
                                            {
                                                self.unicode_version = ver;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {
                    // Unknown OSC — ignored.
                }
            }

            // Skip the OSC bytes — the VT parser never sees them.  This is
            // safe because Term's osc_dispatch ignores all our custom OSC
            // numbers.
            prev_vt_off = vt_osc_end;
        }

        // Process remaining bytes after the last OSC.
        if prev_vt_off < vt_bytes.len() {
            self.processor.advance(&mut self.term, &vt_bytes[prev_vt_off..]);
        }
        let t_vt_elapsed = t_vt_start.elapsed();

        // Propagate damage from alacritty_terminal's internal tracker.
        // Each VT operation (write char, cursor move, scroll, etc.)
        // already marks the affected lines — we just read them out.
        let t_damage_start = std::time::Instant::now();
        match self.term.damage() {
            TermDamage::Full => self.damage.mark_all(),
            TermDamage::Partial(iter) => {
                for line in iter {
                    self.damage.mark(line.line);
                }
            }
        }
        self.term.reset_damage();
        let t_damage_elapsed = t_damage_start.elapsed();

        // ── Drain the event channel ────────────────────────────────────
        // The custom `Listener` (above) receives every `Event::PtyWrite`,
        // `ColorRequest`, etc. that the `Handler` emits.  We process them
        // here and return the collected response bytes.
        let t_evt_start = std::time::Instant::now();
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::PtyWrite(text) => {
                    log::debug!("Terminal::feed: PtyWrite({:?})", text);
                    replies.extend_from_slice(text.as_bytes());
                }
                Event::ColorRequest(index, formatter) => {
                    log::debug!("Terminal::feed: ColorRequest(index={})", index);
                    let colors = self.term.colors();
                    if let Some(rgb) = colors[index] {
                        let response = formatter(rgb);
                        replies.extend_from_slice(response.as_bytes());
                    }
                }
                Event::TextAreaSizeRequest(formatter) => {
                    log::debug!("Terminal::feed: TextAreaSizeRequest");
                    let cols = self.term.columns() as u16;
                    let rows = self.term.screen_lines() as u16;
                    let cell_w = if cols > 0 { (self.pixel_width / cols as u32) as u16 } else { 0 };
                    let cell_h = if rows > 0 { (self.pixel_height / rows as u32) as u16 } else { 0 };
                    let size = WindowSize {
                        num_lines: rows,
                        num_cols: cols,
                        cell_width: cell_w,
                        cell_height: cell_h,
                    };
                    let response = formatter(size);
                    replies.extend_from_slice(response.as_bytes());
                }
                Event::ClipboardStore(_ty, text) => {
                    log::debug!(
                        "Terminal::feed: ClipboardStore({}, {} bytes)",
                        match _ty {
                            ClipboardType::Clipboard => "clipboard",
                            ClipboardType::Selection => "selection",
                        },
                        text.len(),
                    );
                    self.pending_clipboard_store = Some(text);
                }
                Event::ClipboardLoad(_ty, formatter) => {
                    log::debug!("Terminal::feed: ClipboardLoad");
                    self.pending_clipboard_load = Some(formatter);
                }
                Event::Title(title) => {
                    log::debug!("Terminal::feed: Title({:?})", title);
                    self.pending_title = Some(title);
                }
                Event::ResetTitle => {
                    log::debug!("Terminal::feed: ResetTitle (ignored — keep current title)");
                    // Do NOT overwrite the current title.  Some shells / prompt
                    // frameworks use the title-stack push/pop mechanism
                    // (DECPRA `ESC [ 22 t` / DECRPRA `ESC [ 23 t`) to save
                    // and restore the title around command execution.  If the
                    // stack entry is `None` (the terminal's initial state),
                    // popping it sends `ResetTitle` which would briefly flash
                    // "Zenterm" every time a command finishes.  Ignoring it
                    // lets the last non-ResetTitle value persist.
                }
                Event::Bell => {
                    log::debug!("Terminal::feed: Bell");
                    self.pending_bell = true;
                }
                Event::Exit => {
                    log::debug!("Terminal::feed: Exit");
                    self.pending_exit = true;
                }
                Event::ChildExit(status) => {
                    log::debug!("Terminal::feed: ChildExit({:?})", status);
                    self.pending_child_exit = Some(status);
                }
                Event::CursorBlinkingChange
                | Event::MouseCursorDirty
                | Event::Wakeup => {
                    // These events are handled internally by the term or
                    // are noise that we don't need to act on.
                }
            }
        }
        let t_evt_elapsed = t_evt_start.elapsed();

        let elapsed = start.elapsed();
        if elapsed > std::time::Duration::from_millis(50) {
            log::warn!(
                "[perf] Terminal::feed({} bytes) took {:?} (apc_scan={:?} osc_scan={:?} vt_parse={:?} damage={:?} events={:?})",
                bytes.len(), elapsed,
                t_apc_elapsed, t_osc_elapsed, t_vt_elapsed, t_damage_elapsed, t_evt_elapsed,
            );
        }

        replies
    }

    /// Resize the terminal grid.
    pub fn resize(&mut self, size: TermSize) {
        let dim = TermDimensions(size);
        let cols = dim.columns();
        let rows = dim.screen_lines();

        self.term.resize(dim);
        self.damage.resize(rows);
        self.grid_cache.resize(rows, vec![Cell::blank(); cols]);
        for row in self.grid_cache.iter_mut() {
            row.resize(cols, Cell::blank());
        }
        self.image_placements.clear();
        self.virtual_placements.clear();
        self.damage.mark_all();
        self.pixel_width = size.pixel_width as u32;
        self.pixel_height = size.pixel_height as u32;
    }

    /// Get the current terminal size (in cells and pixels).
    pub fn size(&self) -> TermSize {
        TermSize::new(
            self.term.screen_lines() as u16,
            self.term.columns() as u16,
            self.pixel_width as u16,
            self.pixel_height as u16,
        )
    }

    /// Return the visible text of a viewport row as a `String`.
    pub fn line_text(&self, row: usize) -> String {
        use alacritty_terminal::index::{Column, Line};
        let cols = self.term.columns();
        let display_offset = self.term.grid().display_offset();
        let grid_line = Line(row as i32 - display_offset as i32);
        let mut text = String::with_capacity(cols);
        for col in 0..cols {
            text.push(self.term.grid()[grid_line][Column(col)].c);
        }
        text
    }

    // ── Scrollback / display offset ─────────────────────────────────────

    /// Scroll the viewport by `count` lines.
    ///
    /// Positive = scroll up (into history), negative = scroll down (toward bottom).
    /// Returns `true` if the display offset actually changed.
    pub fn scroll_display(&mut self, count: i32) -> bool {
        let old = self.term.grid().display_offset();
        self.term.scroll_display(Scroll::Delta(count));
        if self.term.grid().display_offset() != old {
            self.damage.mark_all();
            return true;
        }
        false
    }

    /// Jump to the bottom of the scrollback (latest output).
    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
        self.damage.mark_all();
    }

    /// Jump to the top of the scrollback (oldest history).
    pub fn scroll_to_top(&mut self) {
        self.term.scroll_display(Scroll::Top);
        self.damage.mark_all();
    }

    /// Number of lines currently in scrollback history.
    pub fn history_size(&self) -> usize {
        self.term.grid().history_size()
    }

    /// Current scroll position. 0 = at bottom, larger = scrolled into history.
    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    /// Whether the viewport is at the bottom (showing latest output).
    pub fn is_at_bottom(&self) -> bool {
        self.term.grid().display_offset() == 0
    }

    /// Return the number of active image placements (for diagnostics).
    pub fn image_placements_count(&self) -> usize {
        self.image_placements.len() + self.virtual_placements.len()
    }

    /// Get a view of the visible grid with resolved colours.
    ///
    /// Only dirty rows are re-converted; clean rows come from the cache.
    pub fn visible_cells(&mut self) -> GridView<'_> {
        let cols = self.term.columns();
        let screen_lines = self.term.screen_lines();

        // Collect dirty row indices first to avoid borrow conflicts.
        let dirty: Vec<usize> = self.damage.iter().collect();
        let grid = self.term.grid();

        for &row_idx in &dirty {
            if row_idx >= screen_lines {
                continue;
            }
            let grid_line = Line(row_idx as i32 - grid.display_offset() as i32);
            for col_idx in 0..cols.min(self.grid_cache[row_idx].len()) {
                let alacell = &grid[grid_line][Column(col_idx)];
                self.grid_cache[row_idx][col_idx] = self.resolve_cell(alacell);
            }
        }

        // Clear the damage set — it has been consumed by the re-resolution above.
        self.damage.clear();

        // Attach image placements (keyed by grid line) to the grid cache.
        let display_offset = grid.display_offset() as i32;
        for (&(grid_line, col), img_cell) in &self.image_placements {
            let viewport_row = grid_line + display_offset;
            if viewport_row >= 0 && (viewport_row as usize) < self.grid_cache.len() {
                let row = viewport_row as usize;
                if col < self.grid_cache[row].len() {
                    self.grid_cache[row][col].image = Some(img_cell.clone());
                }
            }
        }

        // ── Unicode placeholder rendering (Kitty U=1) ──────────────────
        // Scan the visible grid for cells containing PLACEHOLDER_CHAR
        // (U+10EEEE).  For each one, decode the image ID from the fg color
        // and the row/col from combining diacritics, then create an
        // ImageCell pointing at the correct slice of the image.
        //
        // ratatui-image encodes image IDs using TrueColor fg (`38;2;R;G;B`)
        // → `Color::Spec(Rgb{r,g,b})` and only attaches diacritics to the
        // FIRST placeholder character per row.  Subsequent characters
        // inherit row/id_extra and auto-increment col.
        //
        // We collect render params in a first pass (while grid is borrowed),
        // then create ImageCells in a second pass (after grid is released).
        if !self.virtual_placements.is_empty() {
            // Per-row inheritance state for U+10EEEE without diacritics.
            let mut inh_image_id: Option<u32> = None;
            let mut inh_row: Option<u32> = None;
            let mut inh_id_extra: Option<u32> = None;
            let mut inh_col: u32 = 0;

            // (row_idx, col_idx, image_id, row_val, col_val, id_extra)
            let mut placeholders: Vec<(usize, usize, u32, u32, u32, u32)> = Vec::new();

            for row_idx in 0..screen_lines.min(self.grid_cache.len()) {
                let grid_line = Line(row_idx as i32 - grid.display_offset() as i32);
                for col_idx in 0..cols {
                    let alacell = &grid[grid_line][Column(col_idx)];
                    if alacell.c != PLACEHOLDER_CHAR {
                        // Reset inheritance when a non-placeholder is encountered.
                        inh_image_id = None;
                        inh_row = None;
                        inh_id_extra = None;
                        inh_col = 0;
                        continue;
                    }

                    // ── Extract image_id from foreground color ──────────
                    // ratatui-image uses TrueColor: \x1b[38;2;R;G;Bm
                    // which alacritty stores as Color::Spec(Rgb{r,g,b}).
                    let base_id = match alacell.fg {
                        Color::Spec(rgb) => {
                            (rgb.r as u32) << 16 | (rgb.g as u32) << 8 | rgb.b as u32
                        }
                        Color::Indexed(idx) => idx as u32,
                        _ => {
                            // Unknown color format — skip this cell.
                            continue;
                        }
                    };

                    // ── Extract row/col/id_extra from diacritics ───────
                    let zerowidth = alacell.zerowidth().unwrap_or(&[]);

                    if zerowidth.len() >= 2 {
                        // First character in a row — has full diacritics.
                        let row_val = match diacritic_value(zerowidth[0]) {
                            Some(v) => v,
                            None => {
                                log::warn!("[img] unicode placeholder: invalid row diacritic cp={:X}", zerowidth[0] as u32);
                                continue;
                            }
                        };
                        let col_val = match diacritic_value(zerowidth[1]) {
                            Some(v) => v,
                            None => {
                                log::warn!("[img] unicode placeholder: invalid col diacritic cp={:X}", zerowidth[1] as u32);
                                continue;
                            }
                        };
                        let high = if zerowidth.len() >= 3 {
                            diacritic_value(zerowidth[2]).unwrap_or(0)
                        } else {
                            0
                        };
                        let full_id = (high << 24) | base_id;

                        // Update inheritance state.
                        inh_image_id = Some(full_id);
                        inh_row = Some(row_val);
                        inh_id_extra = Some(high);
                        inh_col = col_val;

                        placeholders.push((row_idx, col_idx, full_id, row_val, col_val, high));
                    } else if inh_row.is_some() {
                        // Subsequent character — no diacritics, use inherited values.
                        let full_id = inh_image_id.unwrap_or(base_id);
                        let row_val = inh_row.unwrap();
                        let col_val = inh_col;
                        inh_col = inh_col.saturating_add(1);

                        placeholders.push((row_idx, col_idx, full_id, row_val, col_val,
                            inh_id_extra.unwrap_or(0)));
                    }
                    // else: no inherited state yet — skip.
                }
            }

            // Release grid borrow, then create ImageCells.
            let _ = grid; // release immutable borrow before mutable access
            for (row_idx, col_idx, image_id, row_val, col_val, id_extra) in placeholders {
                self.render_unicode_placeholder_cell(
                    row_idx, col_idx, image_id, row_val, col_val, id_extra,
                );
            }
        }

        GridView {
            rows: &self.grid_cache[..screen_lines.min(self.grid_cache.len())],
        }
    }

    /// Render a single Unicode placeholder cell: look up the virtual
    /// placement, compute the per-cell UV coordinates, and store an
    /// `ImageCell` in the grid cache.
    fn render_unicode_placeholder_cell(
        &mut self,
        row_idx: usize,
        col_idx: usize,
        image_id: u32,
        row_val: u32,
        col_val: u32,
        _id_extra: u32,
    ) {
        // Find the virtual placement for this image_id.
        let vp = self.virtual_placements.get(&(image_id, None))
            .or_else(|| {
                self.virtual_placements.iter()
                    .find(|((id, _), _)| *id == image_id)
                    .map(|(_, vp)| vp)
            });
        let vp = match vp {
            Some(vp) => vp,
            None => {
                log::warn!("[img] unicode placeholder: no virtual placement for image_id={image_id}");
                return;
            }
        };

        let data = match self.image_cache.get(vp.image_id) {
            Some(d) => d.clone(),
            None => return,
        };
        let img_w = data.data().width();
        let img_h = data.data().height();

        if self.cell_pixel_width == 0 || self.cell_pixel_height == 0 {
            return;
        }

        // ── Grid size ──────────────────────────────────────────────
        let grid_cols = if vp.columns > 0 {
            vp.columns
        } else {
            ((img_w + self.cell_pixel_width - 1) / self.cell_pixel_width).max(1)
        };
        let grid_rows = if vp.rows > 0 {
            vp.rows
        } else {
            ((img_h + self.cell_pixel_height - 1) / self.cell_pixel_height).max(1)
        };

        // ── Aspect-ratio-preserving scale ─────────────────────────
        let placement_px_w = grid_cols as f64 * self.cell_pixel_width as f64;
        let placement_px_h = grid_rows as f64 * self.cell_pixel_height as f64;
        let src_x = vp.source_x.unwrap_or(0) as f64;
        let src_y = vp.source_y.unwrap_or(0) as f64;
        let src_w = vp.source_w.unwrap_or(img_w).min(img_w) as f64;
        let src_h = vp.source_h.unwrap_or(img_h).min(img_h) as f64;

        let scale = if src_w * placement_px_h > src_h * placement_px_w {
            placement_px_w / src_w.max(1.0)
        } else {
            placement_px_h / src_h.max(1.0)
        };

        let scaled_src_w = src_w * scale;
        let scaled_src_h = src_h * scale;
        let center_offset_x = (placement_px_w - scaled_src_w) / 2.0;
        let center_offset_y = (placement_px_h - scaled_src_h) / 2.0;

        // ── Per-cell UV calculation ────────────────────────────────
        let cell_px_x = col_val as f64 * self.cell_pixel_width as f64;
        let cell_px_y = row_val as f64 * self.cell_pixel_height as f64;

        let cell_src_x = src_x + (cell_px_x - center_offset_x) / scale;
        let cell_src_y = src_y + (cell_px_y - center_offset_y) / scale;
        let cell_src_w = self.cell_pixel_width as f64 / scale;
        let cell_src_h = self.cell_pixel_height as f64 / scale;

        let clamped_x = cell_src_x.max(src_x);
        let clamped_y = cell_src_y.max(src_y);
        let clamped_w = (cell_src_x + cell_src_w - clamped_x)
            .min(src_x + src_w - clamped_x)
            .max(0.0);
        let clamped_h = (cell_src_y + cell_src_h - clamped_y)
            .min(src_y + src_h - clamped_y)
            .max(0.0);

        if clamped_w <= 0.0 || clamped_h <= 0.0 || img_w == 0 || img_h == 0 {
            return;
        }

        let u0 = clamped_x / img_w as f64;
        let v0 = clamped_y / img_h as f64;
        let u1 = (clamped_x + clamped_w) / img_w as f64;
        let v1 = (clamped_y + clamped_h) / img_h as f64;

        let top_left = zenterm_core::image::TextureCoordinate::new(u0 as f32, v0 as f32);
        let bottom_right = zenterm_core::image::TextureCoordinate::new(u1 as f32, v1 as f32);

        let img_cell = ImageCell {
            top_left,
            bottom_right,
            data,
            z_index: vp.z_index,
            padding_left: vp.x_offset.unwrap_or(0) as u16,
            padding_top: vp.y_offset.unwrap_or(0) as u16,
            padding_right: 0,
            padding_bottom: 0,
            image_id: Some(vp.image_id),
            placement_id: vp.placement_id,
        };

        if row_idx < self.grid_cache.len() && col_idx < self.grid_cache[row_idx].len() {
            self.grid_cache[row_idx][col_idx].image = Some(img_cell);
        }
    }

    /// Drain the current damage set (marking everything clean).
    pub fn drain_damage(&mut self) -> DamageSet {
        let mut ds = DamageSet::new(self.term.screen_lines());
        std::mem::swap(&mut ds, &mut self.damage);
        ds
    }

    /// Get cursor information.
    pub fn cursor(&self) -> CursorInfo {
        let point = self.term.grid().cursor.point;
        // Convert from absolute grid line to viewport row so the
        // caller can compare directly with visual row indices.
        let display_offset = self.term.grid().display_offset();
        let viewport_line = point.line.0 + display_offset as i32;
        CursorInfo {
            pos: TermPos::new(viewport_line.max(0) as usize, point.column.0),
            style: self.term.cursor_style(),
            visible: self.term.mode().contains(TermMode::SHOW_CURSOR),
        }
    }

    /// Get terminal mode flags (needed by the input mapper).
    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    /// Replace the colour scheme (e.g. when the user switches themes).
    ///
    /// Marks the entire grid as dirty so cells are re-resolved next frame.
    pub fn set_scheme(&mut self, scheme: ColorScheme) {
        self.scheme = scheme;
        self.damage.mark_all();
    }

    /// Get the current colour scheme (for inspection).
    pub fn scheme(&self) -> &ColorScheme {
        &self.scheme
    }

    // ── Pending side-effect accessors ──────────────────────────────────
    //
    // These are populated during [`Self::feed()`] and should be queried by
    // the application after each feed call so it can react to terminal
    // requests that cannot be satisfied by merely writing bytes back to the
    // PTY.

    /// Take a pending window title change, if any.
    pub fn take_title(&mut self) -> Option<String> {
        self.pending_title.take()
    }

    /// Take a pending bell request.
    pub fn take_bell(&mut self) -> bool {
        let val = self.pending_bell;
        self.pending_bell = false;
        val
    }

    /// Take a pending exit request.
    pub fn take_exit(&mut self) -> bool {
        let val = self.pending_exit;
        self.pending_exit = false;
        val
    }

    /// Take a pending child-exit notification.
    pub fn take_child_exit(&mut self) -> Option<std::process::ExitStatus> {
        self.pending_child_exit.take()
    }

    /// Take text that the terminal wants stored in the system clipboard.
    pub fn take_clipboard_store(&mut self) -> Option<String> {
        self.pending_clipboard_store.take()
    }

    /// Take a pending desktop notification (title, body) from OSC 9/777.
    pub fn take_notification(&mut self) -> Option<(String, String)> {
        self.pending_notification.take()
    }

    /// Take the most recent ConEmu progress-bar state (OSC 9;4).
    pub fn take_progress(&mut self) -> Option<Progress> {
        self.pending_progress.take()
    }

    /// Take the most recent FinalTerm OSC 133 semantic prompt marker.
    ///
    /// Returns `None` if no new OSC 133 was seen since the last call.
    pub fn take_semantic_prompt(&mut self) -> Option<SemanticPrompt> {
        self.pending_semantic_prompt.take()
    }

    /// Take the most recent completed Kitty OSC 99 notification.
    ///
    /// Returns `None` if no new OSC 99 notification was completed since
    /// the last call.
    pub fn take_kitty_notification(&mut self) -> Option<KittyNotification> {
        self.pending_kitty_notification.take()
    }

    /// Take the most recent pending iTerm2 proprietary action (OSC 1337).
    ///
    /// Returns `None` if no new OSC 1337 action is pending since the last call.
    pub fn take_iterm_action(&mut self) -> Option<ITermProprietary> {
        self.pending_iterm_action.take()
    }

    /// Get a reference to the user-defined variables map.
    pub fn user_vars(&self) -> &HashMap<String, String> {
        &self.user_vars
    }

    /// Take accumulated navigation marks from `OSC 1337;SetMark`.
    ///
    /// Each mark is `(column, viewport_line)`.
    pub fn take_marks(&mut self) -> Vec<(usize, usize)> {
        std::mem::take(&mut self.marks)
    }

    /// Take the most recent OSC 7 working-directory URL (if any).
    ///
    /// The value is the raw URL as emitted by the application
    /// (typically `file://host/path` or just `/abs/path`); callers are
    /// responsible for URL-decoding and stripping the host component.
    /// Returns `None` if no new OSC 7 was seen since the last call.
    pub fn take_current_directory(&mut self) -> Option<String> {
        self.pending_current_directory.take()
    }

    /// Take a clipboard-load request.
    ///
    /// The returned closure is a formatter: the application should read the
    /// current system clipboard text and pass it to the closure.  The
    /// closure returns the escape-sequence bytes that must be written back
    /// to the PTY.
    pub fn take_clipboard_load(
        &mut self,
    ) -> Option<Arc<dyn Fn(&str) -> String + Sync + Send + 'static>> {
        self.pending_clipboard_load.take()
    }

    // ── Selection support ──────────────────────────────────────────────────

    /// Start a new selection at the given viewport position.
    ///
    /// `line` is a viewport row (0 = top).  It is converted to grid
    /// coordinates internally so the selection tracks the correct cells
    /// even when the viewport is scrolled into history.
    pub fn start_selection(&mut self, line: usize, col: usize) {
        let display_offset = self.term.grid().display_offset();
        let grid_line = (line as i32) - (display_offset as i32);
        let point = Point::new(Line(grid_line), Column(col));
        self.term.selection = Some(Selection::new(
            SelectionType::Simple,
            point,
            Direction::Left,
        ));
    }

    /// Extend the current selection to the given viewport position.
    pub fn update_selection(&mut self, line: usize, col: usize) {
        let display_offset = self.term.grid().display_offset();
        let grid_line = (line as i32) - (display_offset as i32);
        if let Some(ref mut sel) = self.term.selection {
            let point = Point::new(Line(grid_line), Column(col));
            sel.update(point, Direction::Left);
        }
    }

    /// Clear the active selection.
    pub fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    /// Check whether a selection is currently active.
    pub fn has_selection(&self) -> bool {
        self.term.selection.is_some()
    }

    /// Check whether a specific cell (in viewport coordinates) is within the selection range.
    pub fn is_selected(&self, line: usize, col: usize) -> bool {
        let range = match self
            .term
            .selection
            .as_ref()
            .and_then(|s| s.to_range(&self.term))
        {
            Some(r) => r,
            None => return false,
        };
        let display_offset = self.term.grid().display_offset();
        let grid_line = (line as i32) - (display_offset as i32);
        let point = Point::new(Line(grid_line), Column(col));
        range.contains(point)
    }

    /// Extract selected text as a `String`, if any selection is active.
    pub fn selected_text(&self) -> Option<String> {
        self.term.selection_to_string()
    }

    /// Return the raw selection range, if any, so callers can check
    /// cell membership without an extra `&self` borrow.
    pub fn selection_range(&self) -> Option<alacritty_terminal::selection::SelectionRange> {
        self.term
            .selection
            .as_ref()
            .and_then(|s| s.to_range(&self.term))
    }

    /// Selection background colour (RGBA).
    pub fn selection_bg(&self) -> Rgba {
        self.scheme.selection_bg
    }

    /// Selection foreground colour, if configured.  `None` means keep fg.
    pub fn selection_fg(&self) -> Option<Rgba> {
        self.scheme.selection_fg
    }

    /// Default background colour — the resolved `NamedColor::Background`.
    ///
    /// Cells whose `cell.bg` equals this value don't need their own
    /// background quad: the terminal-wide `rect_filled` (or, with
    /// `viewport.transparent(true)`, the OS desktop through a
    /// transparent clear) already covers them.  This is the same
    /// pattern cosmic-term uses in `terminal_box.rs:576`
    /// (`if metadata.bg != default_metadata.bg`).
    pub fn default_bg(&self) -> Rgba {
        self.resolve_color(Color::Named(NamedColor::Background))
    }

    // ---- Helpers ----

    fn resolve_cell(&self, alacell: &alacritty_terminal::term::cell::Cell) -> Cell {
        let c = alacell.c;
        let fg = self.resolve_color(alacell.fg);
        let bg = self.resolve_color(alacell.bg);
        let flags = alacell.flags;

        let underline_style = if flags.contains(Flags::DOUBLE_UNDERLINE) {
            UnderlineStyle::Double
        } else if flags.contains(Flags::UNDERCURL) {
            UnderlineStyle::Curly
        } else if flags.contains(Flags::DOTTED_UNDERLINE) {
            UnderlineStyle::Dotted
        } else if flags.contains(Flags::DASHED_UNDERLINE) {
            UnderlineStyle::Dashed
        } else if flags.contains(Flags::UNDERLINE) {
            UnderlineStyle::Normal
        } else {
            UnderlineStyle::None
        };

        Cell {
            c,
            fg: if flags.contains(Flags::INVERSE) { bg } else { fg },
            bg: if flags.contains(Flags::INVERSE) { fg } else { bg },
            bold: flags.contains(Flags::BOLD),
            italic: flags.contains(Flags::ITALIC),
            underline_style,
            strikethrough: flags.contains(Flags::STRIKEOUT),
            inverse: flags.contains(Flags::INVERSE),
            dim: flags.contains(Flags::DIM),
            hidden: flags.contains(Flags::HIDDEN),
            is_spacer: flags.contains(Flags::WIDE_CHAR_SPACER),
            image: None,
        }
    }

    fn resolve_color(&self, color: Color) -> Rgba {
        match color {
            Color::Named(named) => {
                let rgb = self.scheme.colors[named]
                    .unwrap_or_else(|| named_color_default_rgb(named));
                Rgba::from_u8(rgb.r, rgb.g, rgb.b, 255)
            }
            Color::Spec(rgb) => Rgba::from_u8(rgb.r, rgb.g, rgb.b, 255),
            Color::Indexed(idx) => self.scheme.colors[idx as usize]
                .map(|rgb| Rgba::from_u8(rgb.r, rgb.g, rgb.b, 255))
                .unwrap_or(Rgba::WHITE),
        }
    }
}

// ── APC / DCS scan helpers ─────────────────────────────────────────────

/// Scan for the next Kitty APC sequence starting at `offset`.
/// Returns `(payload, end_pos)` where `end_pos` is the byte after `\x1b\\`.


// ── Kitty protocol handler ─────────────────────────────────────────────

impl Terminal {
    /// Handle a parsed Kitty image command.
    /// Returns `Some(response_bytes)` for `a=q` queries that must be
    /// written back to the PTY.
    fn handle_kitty_command(&mut self, cmd: KittyImage) -> Option<String> {
        // Feed through the accumulator to support multi-chunk transmissions.
        let assembled = match self.kitty_accumulator.feed(cmd) {
            Ok(Some(assembled)) => assembled,
            Ok(None) => return None, // waiting for more chunks
            Err(e) => {
                log::error!("[img] kitty accumulator error: {e}");
                return None;
            }
        };

        log::debug!(
            "[img] handle_kitty_command: variant={}, cache_images={}, placements={}",
            kitty_cmd_variant_name(&assembled),
            self.image_cache.all_hashes().len(),
            self.image_placements.len(),
        );

        match assembled {
            KittyImage::TransmitData { transmit, verbosity } => {
                log::debug!(
                    "[img] TransmitData: fmt={:?}, w={:?}, h={:?}, id={:?}, num={:?}",
                    transmit.format, transmit.width, transmit.height,
                    transmit.image_id, transmit.image_number,
                );
                // Implicit ID (i=0, I=0): do not respond.
                let implicit = transmit.image_id == Some(0) && transmit.image_number == Some(0);
                let resp_id = transmit.image_id;
                let resp_num = transmit.image_number;
                if verbosity != kitty::KittyImageVerbosity::Quiet {
                    match kitty::decode_image_data(transmit, &mut self.image_cache) {
                        Ok(id) => {
                            log::debug!("[img] TransmitData decode OK, image_id={id}");
                            if implicit { return None; }
                            return Some(kitty::kitty_response(
                                Some(id), None, "OK",
                            ));
                        }
                        Err(e) => {
                            log::error!("[img] TransmitData decode FAILED: {e}");
                            if implicit { return None; }
                            return Some(kitty::kitty_response(
                                resp_id, resp_num,
                                &format!("ERROR:{e}"),
                            ));
                        }
                    }
                } else {
                    let _ = kitty::decode_image_data(transmit, &mut self.image_cache);
                }
                None
            }
            KittyImage::TransmitDataAndDisplay { transmit, placement, .. } => {
                log::debug!(
                    "[img] TransmitDataAndDisplay: fmt={:?}, w={:?}, h={:?}, id={:?}, num={:?}, virtual={}",
                    transmit.format, transmit.width, transmit.height,
                    transmit.image_id, transmit.image_number,
                    placement.virtual_placement,
                );
                match kitty::decode_image_data(transmit, &mut self.image_cache) {
                    Ok(image_id) => {
                        log::debug!("[img] decode OK, image_id={image_id}, calling kitty_place_image");
                        self.kitty_place_image(Some(image_id), None, placement);
                    }
                    Err(e) => log::error!("[img] decode FAILED: {e}"),
                }
                None
            }
            KittyImage::Display { image_id, image_number, placement, .. } => {
                log::debug!(
                    "[img] Display: image_id={image_id:?}, num={image_number:?}, virtual={}",
                    placement.virtual_placement,
                );
                self.kitty_place_image(image_id, image_number, placement);
                None
            }
            KittyImage::Delete { what, .. } => {
                log::debug!("[img] Delete");
                self.handle_kitty_delete(what);
                None
            }
            KittyImage::Query { transmit } => {
                log::debug!(
                    "[img] Query: id={:?}, num={:?}",
                    transmit.image_id, transmit.image_number,
                );
                // EINVAL: image ID required for query.
                if transmit.image_id == Some(0) && transmit.image_number == Some(0) {
                    return Some(kitty::kitty_response(
                        transmit.image_id, transmit.image_number,
                        "EINVAL: image ID required",
                    ));
                }
                Some(kitty::kitty_response(
                    transmit.image_id,
                    transmit.image_number,
                    "OK",
                ))
            }
            KittyImage::TransmitFrame { transmit, frame, verbosity } => {
                log::debug!("[img] TransmitFrame");
                let result = kitty::decode_image_frame(transmit, frame, &mut self.image_cache);
                match &result {
                    Ok(()) => {
                        if verbosity != kitty::KittyImageVerbosity::Quiet {
                            // No image_id readily available from frame result; respond generically.
                            return Some(kitty::kitty_response(None, None, "OK"));
                        }
                    }
                    Err(e) => {
                        log::error!("[img] frame transmit FAILED: {e}");
                        if verbosity != kitty::KittyImageVerbosity::OnlyErrors {
                            return Some(kitty::kitty_response(None, None, &format!("ERROR:{e}")));
                        }
                    }
                }
                None
            }
            KittyImage::ComposeFrame { frame, verbosity } => {
                log::debug!("[img] ComposeFrame");
                let resp_id = frame.image_id;
                let resp_num = frame.image_number;
                let result = kitty::handle_compose_frame(frame, &mut self.image_cache);
                match &result {
                    Ok(()) => {
                        if verbosity != kitty::KittyImageVerbosity::Quiet {
                            return Some(kitty::kitty_response(
                                resp_id, resp_num, "OK",
                            ));
                        }
                    }
                    Err(e) => {
                        log::error!("[img] compose frame FAILED: {e}");
                        if verbosity != kitty::KittyImageVerbosity::OnlyErrors {
                            return Some(kitty::kitty_response(
                                resp_id, resp_num,
                                &format!("ERROR:{e}"),
                            ));
                        }
                    }
                }
                None
            }
            KittyImage::AnimationControl { control, verbosity } => {
                log::debug!(
                    "[img] AnimationControl: action={:?}, frame={:?}, gap={:?}",
                    control.action, control.frame, control.gap_ms,
                );
                // Animation playback control is not yet supported; return error.
                if verbosity != kitty::KittyImageVerbosity::OnlyErrors {
                    return Some(kitty::kitty_response(None, None, "ERROR: animation control not implemented"));
                }
                None
            }
        }
    }

    fn kitty_place_image(
        &mut self,
        image_id: Option<u32>,
        image_number: Option<u32>,
        placement: kitty::KittyImagePlacement,
    ) {
        let id = self.image_cache.assign_id(image_id, image_number);
        log::debug!(
            "[img] kitty_place_image: resolved_id={id}, image_id={image_id:?}, \
             num={image_number:?}, virtual={}, cell_pixel={}x{}, do_not_move={}",
            placement.virtual_placement,
            self.cell_pixel_width, self.cell_pixel_height,
            placement.do_not_move_cursor,
        );

        // U=1 — virtual placement: store metadata for later rendering via
        // Unicode placeholder characters.  No direct image is placed.
        if placement.virtual_placement {
            // EINVAL: virtual placement cannot refer to a parent.
            if placement.parent_id.is_some_and(|p| p > 0) {
                log::error!(
                    "[img] EINVAL: virtual placement cannot refer to a parent (parent_id={})",
                    placement.parent_id.unwrap(),
                );
                return;
            }
            let vp = VirtualPlacement {
                image_id: id,
                placement_id: placement.placement_id,
                columns: placement.columns.unwrap_or(0),
                rows: placement.rows.unwrap_or(0),
                source_x: placement.x,
                source_y: placement.y,
                source_w: placement.w,
                source_h: placement.h,
                x_offset: placement.x_offset,
                y_offset: placement.y_offset,
                z_index: placement.z_index.unwrap_or(0),
            };
            log::debug!(
                "[img] virtual placement stored: id={}, p={:?}, grid={}x{}",
                vp.image_id, vp.placement_id, vp.columns, vp.rows,
            );
            self.virtual_placements.insert(
                (vp.image_id, vp.placement_id),
                vp,
            );
            // Virtual placements do not move the cursor.
            return;
        }

        // Direct placement path (U=0 or absent).
        let data = match self.image_cache.get(id) {
            Some(d) => d.clone(),
            None => {
                log::error!("[img] kitty place: image id {id} not found in cache");
                return;
            }
        };

        let img_w = data.data().width();
        let img_h = data.data().height();

        if self.cell_pixel_width == 0 || self.cell_pixel_height == 0 {
            log::warn!(
                "[img] kitty_place_image: cell_pixel is 0 ({}x{}), SKIPPING placement",
                self.cell_pixel_width, self.cell_pixel_height,
            );
            return;
        }

        let cursor = self.cursor();
        let cols = self.term.columns();
        let rows = self.term.screen_lines();

        // X/Y (unsigned) are the primary cell padding offsets.
        // H/V (signed) are for relative placements (P/Q parent);
        // since parent placement is not yet supported, H/V are stored
        // but do not affect the placement coordinates.
        let params = PlacementParams {
            columns: placement.columns.map(|c| c as usize),
            rows: placement.rows.map(|r| r as usize),
            source_x: placement.x,
            source_y: placement.y,
            source_w: placement.w,
            source_h: placement.h,
            cell_padding_left: placement.x_offset.unwrap_or(0) as u16,
            cell_padding_top: placement.y_offset.unwrap_or(0) as u16,
            z_index: placement.z_index.unwrap_or(0),
            do_not_move_cursor: placement.do_not_move_cursor,
            image_id: Some(id),
            placement_id: placement.placement_id,
            style: PlacementStyle::Kitty,
        };

        let result = assign_image_to_cells(
            data,
            img_w,
            img_h,
            &params,
            self.cell_pixel_width,
            self.cell_pixel_height,
            cursor.pos.column,
            cursor.pos.line.min(rows.saturating_sub(1)),
            cols,
            rows,
        );

        // Store placements keyed by grid-relative line so they follow
        // content when the viewport scrolls.
        let display_offset = self.term.grid().display_offset() as i32;
        for (col, viewport_row, cell) in &result.cells {
            // viewport_row is in [0, screen_lines).  Convert to grid line.
            let grid_line = *viewport_row as i32 - display_offset;
            self.image_placements.insert((grid_line, *col), cell.clone());
        }

        let new_cursor = if result.move_cursor {
            let new_col = (cursor.pos.column + result.width_in_cells).min(cols.saturating_sub(1));
            let new_row = (cursor.pos.line + result.height_in_cells)
                .saturating_sub(1)
                .min(rows.saturating_sub(1));
            (new_col, new_row)
        } else {
            (cursor.pos.column, cursor.pos.line)
        };
        log::debug!(
            "[img] placed {} cells ({}x{}), total_placements={}, \
             img={}x{}px, cursor ({},{})→({},{})",
            result.cells.len(), result.width_in_cells, result.height_in_cells,
            self.image_placements.len(),
            img_w, img_h,
            cursor.pos.column, cursor.pos.line,
            new_cursor.0, new_cursor.1,
        );

        if result.move_cursor {
            // Kitty moves cursor to after the bottom-right of the image.
            self.term.grid_mut().cursor.point.column = alacritty_terminal::index::Column(new_cursor.0);
            self.term.grid_mut().cursor.point.line = alacritty_terminal::index::Line(new_cursor.1 as i32);
        }

        self.damage.mark_all();
    }

    /// Look up a built-in iTerm2 session variable by name.
    ///
    /// Returns `Some(value)` for recognised variables, `None` otherwise.
    /// The caller falls back to `user_vars` if this returns `None`.
    fn iterm_builtin_var(&self, name: &str) -> Option<String> {
        match name {
            "session.terminalName" => Some("Zenterm".into()),
            "session.name" => {
                // Use the tab title if available, otherwise "zenterm".
                Some(
                    self.pending_title
                        .as_ref()
                        .cloned()
                        .unwrap_or_else(|| "zenterm".into()),
                )
            }
            "session.hostname" => {
                // Try environment variables, fall back to hostname command.
                let from_env = std::env::var("HOSTNAME")
                    .or_else(|_| std::env::var("HOST"))
                    .ok();
                if let Some(host) = from_env {
                    Some(host)
                } else {
                    std::process::Command::new("hostname")
                        .output()
                        .ok()
                        .and_then(|o| {
                            if o.status.success() {
                                String::from_utf8(o.stdout)
                                    .ok()
                                    .map(|s| s.trim().to_string())
                            } else {
                                None
                            }
                        })
                }
            }
            "session.path" => {
                // Current working directory from OSC 7 if available.
                self.pending_current_directory.clone()
            }
            "session.tty" => {
                // Return the terminal device if we have it.
                None // Not tracked in current architecture.
            }
            _ => None,
        }
    }

    /// Handle iTerm2 inline image (`OSC 1337;File=…` with `inline=1`).
    ///
    /// Decodes the image data, stores it in the image cache, and places it
    /// on the terminal grid at the current cursor position.
    fn handle_iterm_inline_image(&mut self, file: ITermFileData) {
        log::debug!(
            "[iterm-img] inline image: name={:?}, size={:?}, data_len={}, \
             cell_pixel={}x{}, cursor={:?}",
            file.name,
            file.size,
            file.data.len(),
            self.cell_pixel_width,
            self.cell_pixel_height,
            self.cursor(),
        );

        if self.cell_pixel_width == 0 || self.cell_pixel_height == 0 {
            log::warn!(
                "[iterm-img] cell_pixel is 0 ({}x{}), SKIPPING placement",
                self.cell_pixel_width,
                self.cell_pixel_height,
            );
            return;
        }

        // Decode the image data using the `image` crate (PNG, JPEG, GIF, …).
        let decoded = match image::load_from_memory(&file.data) {
            Ok(img) => img.into_rgba8(),
            Err(e) => {
                log::error!("[iterm-img] failed to decode image: {e}");
                return;
            }
        };
        let (img_w, img_h) = decoded.dimensions();
        let rgba = decoded.into_vec();

        // Store in image cache with a unique id.
        let image_data = Arc::new(ImageData::new(ImageDataType::new_rgba8(
            rgba, img_w, img_h,
        )));
        // Use a unique auto-incrementing number so each image gets its own
        // cache slot, even when the application sends multiple `File=` sequences.
        let number = self.next_iterm_image_number;
        self.next_iterm_image_number += 1;
        let image_id = self.image_cache.assign_id(None, Some(number));
        self.image_cache.insert(image_id, image_data.clone());

        // Convert iTerm2 dimensions to columns/rows for PlacementParams.
        let cols = self.term.columns();
        let rows = self.term.screen_lines();

        let (columns, rows_opt) = self.iterm_dimensions_to_grid(
            file.width,
            file.height,
            img_w,
            img_h,
            cols,
            rows,
        );

        let cursor = self.cursor();
        let cursor_col = cursor.pos.column;
        let cursor_row = cursor.pos.line.min(rows.saturating_sub(1));

        let params = PlacementParams {
            columns: Some(columns),
            rows: rows_opt,
            source_x: None,
            source_y: None,
            source_w: None,
            source_h: None,
            cell_padding_left: 0,
            cell_padding_top: 0,
            z_index: 0,
            do_not_move_cursor: file.do_not_move_cursor,
            image_id: Some(image_id),
            placement_id: None,
            style: PlacementStyle::Iterm,
        };

        let result = assign_image_to_cells(
            image_data,
            img_w,
            img_h,
            &params,
            self.cell_pixel_width,
            self.cell_pixel_height,
            cursor_col,
            cursor_row,
            cols,
            rows,
        );

        // Store placements.
        let display_offset = self.term.grid().display_offset() as i32;
        for (col, viewport_row, cell) in &result.cells {
            let grid_line = *viewport_row as i32 - display_offset;
            self.image_placements.insert((grid_line, *col), cell.clone());
        }

        // Move cursor if needed.
        if result.move_cursor {
            let new_col = (cursor_col + result.width_in_cells).min(cols.saturating_sub(1));
            let new_row = (cursor_row + result.height_in_cells)
                .saturating_sub(1)
                .min(rows.saturating_sub(1));
            self.term.grid_mut().cursor.point.column =
                alacritty_terminal::index::Column(new_col);
            self.term.grid_mut().cursor.point.line =
                alacritty_terminal::index::Line(new_row as i32);
        }

        log::debug!(
            "[iterm-img] placed {} cells ({}x{}), img={}x{}px",
            result.cells.len(),
            result.width_in_cells,
            result.height_in_cells,
            img_w,
            img_h,
        );

        self.damage.mark_all();
    }

    /// Convert iTerm2 `ITermDimension` width/height to grid columns/rows.
    fn iterm_dimensions_to_grid(
        &self,
        width: ITermDimension,
        height: ITermDimension,
        img_w: u32,
        img_h: u32,
        max_cols: usize,
        max_rows: usize,
    ) -> (usize, Option<usize>) {
        let cell_w = self.cell_pixel_width.max(1);
        let cell_h = self.cell_pixel_height.max(1);

        let calc_cols = |dim: ITermDimension| -> Option<usize> {
            match dim {
                ITermDimension::Automatic => None,
                ITermDimension::Cells(n) => Some(n.max(1) as usize),
                ITermDimension::Pixels(n) => {
                    Some((n.max(1) as u32 / cell_w).max(1) as usize)
                }
                ITermDimension::Percent(n) => {
                    let pct = n.max(1).min(100) as usize;
                    Some((max_cols * pct / 100).max(1))
                }
            }
        };
        let calc_rows = |dim: ITermDimension| -> Option<usize> {
            match dim {
                ITermDimension::Automatic => None,
                ITermDimension::Cells(n) => Some(n.max(1) as usize),
                ITermDimension::Pixels(n) => {
                    Some((n.max(1) as u32 / cell_h).max(1) as usize)
                }
                ITermDimension::Percent(n) => {
                    let pct = n.max(1).min(100) as usize;
                    Some((max_rows * pct / 100).max(1))
                }
            }
        };

        let columns = calc_cols(width).unwrap_or_else(|| {
            ((img_w + cell_w - 1) / cell_w).max(1) as usize
        });
        let rows_out = calc_rows(height).unwrap_or_else(|| {
            ((img_h + cell_h - 1) / cell_h).max(1) as usize
        });

        (columns.min(max_cols), Some(rows_out.min(max_rows)))
    }

    fn handle_kitty_delete(&mut self, what: kitty::KittyImageDelete) {
        match what {
            kitty::KittyImageDelete::All { delete } => {
                self.image_placements.clear();
                self.virtual_placements.clear();
                if delete {
                    // Collect all hashes before clearing for atlas cleanup.
                    let hashes: Vec<[u8; 32]> = self.image_cache.all_hashes();
                    self.pending_image_deallocations.extend(hashes);
                    self.image_cache.clear();
                }
            }
            kitty::KittyImageDelete::ByImageId { image_id, placement_id, delete } => {
                self.image_placements.retain(|_, v| {
                    if v.image_id != Some(image_id) { return true; }
                    placement_id.map_or(false, |p| v.placement_id != Some(p))
                });
                self.virtual_placements.retain(|(id, pid), _| {
                    *id != image_id || placement_id.is_some_and(|p| *pid != Some(p))
                });
                if delete {
                    if let Some(hash) = self.image_cache.remove(image_id) {
                        self.pending_image_deallocations.push(hash);
                    }
                }
            }
            kitty::KittyImageDelete::ByImageNumber { image_number: _, placement_id, delete } => {
                // Look up the image_id from the number mapping.
                let ids: Vec<u32> = self.image_placements.iter()
                    .filter(|(_, v)| v.placement_id == placement_id)
                    .map(|(_, v)| v.image_id)
                    .flatten()
                    .collect();
                for id in ids {
                    self.image_placements.retain(|_, v| v.image_id != Some(id));
                    self.virtual_placements.retain(|(vid, pid), _| {
                        *vid != id || placement_id.is_some_and(|p| *pid != Some(p))
                    });
                    if delete {
                        self.image_cache.remove(id);
                    }
                }
            }
            kitty::KittyImageDelete::AtCursorPosition { delete } => {
                let cursor = self.cursor();
                self.image_placements.retain(|&(line, col), _| {
                    let viewport_row = line + self.term.grid().display_offset() as i32;
                    viewport_row != cursor.pos.line as i32 || col != cursor.pos.column
                });
                if delete {
                    // Can't delete data without knowing the image_id.
                    log::warn!("kitty delete AtCursorPosition with delete=true: image_id unknown");
                }
            }
            kitty::KittyImageDelete::DeleteAt { x, y, delete } => {
                let display_offset = self.term.grid().display_offset() as i32;
                let del_grid_line = y as i32 - display_offset;
                self.image_placements.retain(|&(line, col), _| {
                    !(line == del_grid_line && col == x as usize)
                });
                if delete {
                    log::warn!("kitty delete DeleteAt with delete=true: image_id unknown");
                }
            }
            kitty::KittyImageDelete::DeleteColumn { x, delete: _ } => {
                let display_offset = self.term.grid().display_offset() as i32;
                self.image_placements.retain(|&(line, _), _| {
                    let viewport_row = line + display_offset;
                    viewport_row != x as i32
                });
            }
            kitty::KittyImageDelete::DeleteRow { y, delete: _ } => {
                self.image_placements.retain(|&(_, col), _| col != y as usize);
            }
            kitty::KittyImageDelete::DeleteZ { z, delete: _ } => {
                self.image_placements.retain(|_, v| v.z_index != z);
            }
            kitty::KittyImageDelete::DeleteAnimationFrames { delete } => {
                // For each image in the cache, if it is animated (AnimRgba8),
                // convert it to single-frame Rgba8 (keep first frame only).
                // Then remove all placements for that image.
                let all_ids: Vec<u32> = self.image_cache.all_image_ids();
                for id in all_ids {
                    let dominated = self.image_cache.get(id).map(|d| {
                        let guard = d.data();
                        matches!(&*guard, zenterm_core::image::ImageDataType::AnimRgba8 { .. })
                    }).unwrap_or(false);

                    if dominated {
                        // Convert AnimRgba8 → Rgba8 (keep first frame).
                        if let Some(d) = self.image_cache.get(id) {
                            let mut guard = d.data();
                            if let zenterm_core::image::ImageDataType::AnimRgba8 {
                                ref width, ref height, ref frames, ..
                            } = *guard {
                                if let Some(first_frame) = frames.first() {
                                    let new_data = zenterm_core::image::ImageDataType::new_rgba8(
                                        first_frame.clone(), *width, *height,
                                    );
                                    *guard = new_data;
                                }
                            }
                        }
                        // Remove all placements for this image since animation changed.
                        self.image_placements.retain(|_, v| v.image_id != Some(id));
                        self.virtual_placements.retain(|(vid, _), _| *vid != id);
                    }
                    if delete {
                        if let Some(hash) = self.image_cache.remove(id) {
                            self.pending_image_deallocations.push(hash);
                        }
                        self.image_placements.retain(|_, v| v.image_id != Some(id));
                        self.virtual_placements.retain(|(vid, _), _| *vid != id);
                    }
                }
            }
            kitty::KittyImageDelete::DeleteAtCellZ { x, y, z, delete } => {
                let display_offset = self.term.grid().display_offset() as i32;
                let del_grid_line = y as i32 - display_offset;
                self.image_placements.retain(|&(line, col), v| {
                    !(line == del_grid_line && col == x as usize && v.z_index == z)
                });
                if delete {
                    log::warn!("kitty delete DeleteAtCellZ with delete=true: image_id unknown");
                }
            }
            kitty::KittyImageDelete::DeleteRange { first, last, delete } => {
                // Delete all placements whose image_id is in [first, last].
                let ids_to_delete: Vec<u32> = self.image_placements.iter()
                    .filter(|(_, v)| {
                        v.image_id.map_or(false, |id| id >= first && id <= last)
                    })
                    .map(|(_, v)| v.image_id.unwrap())
                    .collect();
                for id in ids_to_delete {
                    self.image_placements.retain(|_, v| v.image_id != Some(id));
                    if delete {
                        if let Some(hash) = self.image_cache.remove(id) {
                            self.pending_image_deallocations.push(hash);
                        }
                    }
                }
            }
        }
        self.damage.mark_all();
    }

    /// Handle a sixel image transmission.
    fn handle_sixel(&mut self, payload: &[u8], params: &[i64]) {
        if self.cell_pixel_width == 0 || self.cell_pixel_height == 0 {
            log::warn!("sixel: cell pixel size not set, skipping");
            return;
        }

        let mut builder = SixelBuilder::new(params);
        for &b in payload {
            builder.push(b);
        }
        builder.finish();

        match sixel::render_sixel(&builder.sixel) {
            Ok(data) => {
                let cursor = self.cursor();
                let cols = self.term.columns();
                let rows = self.term.screen_lines();
                let img_w = data.data().width();
                let img_h = data.data().height();

                let par = PlacementParams {
                    columns: None,
                    rows: None,
                    source_x: None,
                    source_y: None,
                    source_w: None,
                    source_h: None,
                    cell_padding_left: 0,
                    cell_padding_top: 0,
                    z_index: 0, // sixel is behind text
                    do_not_move_cursor: false,
                    image_id: None,
                    placement_id: None,
                    style: PlacementStyle::Sixel,
                };

                let result = assign_image_to_cells(
                    data,
                    img_w,
                    img_h,
                    &par,
                    self.cell_pixel_width,
                    self.cell_pixel_height,
                    cursor.pos.column,
                    cursor.pos.line.min(rows.saturating_sub(1)),
                    cols,
                    rows,
                );

                let display_offset = self.term.grid().display_offset() as i32;
                for (col, viewport_row, cell) in &result.cells {
                    let grid_line = *viewport_row as i32 - display_offset;
                    self.image_placements.insert((grid_line, *col), cell.clone());
                }
                self.damage.mark_all();
            }
            Err(e) => log::error!("sixel render: {e}"),
        }
    }
}

// ── Diagnostic helpers ────────────────────────────────────────────────

fn kitty_cmd_variant_name(cmd: &KittyImage) -> &'static str {
    match cmd {
        KittyImage::TransmitData { .. } => "TransmitData",
        KittyImage::TransmitDataAndDisplay { .. } => "TransmitDataAndDisplay",
        KittyImage::Display { .. } => "Display",
        KittyImage::Delete { .. } => "Delete",
        KittyImage::Query { .. } => "Query",
        KittyImage::TransmitFrame { .. } => "TransmitFrame",
        KittyImage::ComposeFrame { .. } => "ComposeFrame",
        KittyImage::AnimationControl { .. } => "AnimationControl",
    }
}
