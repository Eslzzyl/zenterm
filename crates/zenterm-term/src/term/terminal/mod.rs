//! Terminal state machine and public API.
//!
//! Wraps [`alacritty_terminal::Term`] + [`vte::ansi::Processor`] and provides
//! methods for feeding bytes, resizing, scrolling, and reading the grid.

use std::collections::HashMap;
use std::sync::{mpsc, Arc};

use alacritty_terminal::event::{Event, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line};

use zenterm_core::image::ImageCell;

use crate::image::kitty::{KittyAccumulator, KittyImage};
use crate::image::sixel;
use crate::image::ImageCache;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{ClipboardType, Config as TermConfig, Term, TermDamage, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor};

use zenterm_core::cell::{Cell, UnderlineStyle};
use zenterm_core::color::Rgba;
use zenterm_core::damage::DamageSet;
use zenterm_core::position::TermPos;
use zenterm_core::size::TermSize;
use zenterm_core::{
    ITermProprietary, ITermUnicodeVersionOp,
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

mod unicode;
mod selection;
mod image;

use self::unicode::{PLACEHOLDER_CHAR, diacritic_value, VirtualPlacement};

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
