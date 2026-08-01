//! Terminal state machine and public API.
//!
//! Wraps [`alacritty_terminal::Term`] + [`vte::ansi::Processor`] and provides
//! methods for feeding bytes, resizing, scrolling, and reading the grid.

use std::collections::HashMap;
use std::sync::{Arc, mpsc};

use alacritty_terminal::event::{Event, WindowSize};
use alacritty_terminal::grid::Dimensions;

use zenterm_core::image::ImageCell;

use crate::image::ImageCache;
use crate::image::kitty::KittyAccumulator;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{ClipboardType, Config as TermConfig, Term, TermDamage};
use alacritty_terminal::vte::ansi::{Color, Processor, Rgb};

use zenterm_core::cell::{Cell, UnderlineStyle};
use zenterm_core::color::Rgba;
use zenterm_core::damage::DamageSet;
use zenterm_core::size::TermSize;
use zenterm_core::{ITermProprietary, KittyNotification, Progress, SemanticPrompt};

use super::TermDimensions;
use super::color_scheme::{ColorScheme, named_color_default_rgb};
use super::listener::Listener;
use super::osc::{KittyNotificationState, scan_oscs_with_remainder};

mod effects;
mod grid;
mod image;
mod osc_dispatch;
mod protocol;
mod selection;
mod unicode;

use self::unicode::VirtualPlacement;

type ClipboardLoad = Arc<dyn Fn(&str) -> String + Sync + Send + 'static>;

/// Cursor appearance preferences injected by the UI layer (mirrors
/// `[cursor]` in the config file).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorPrefs {
    /// Default cursor shape.  The terminal's own `DECSCUSR` escape
    /// sequences override this per-session once received.
    pub shape: vte::ansi::CursorShape,
    /// Blinking policy applied on top of the escape-sequence state.
    pub blink: BlinkPolicy,
}

/// Cursor blinking policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlinkPolicy {
    /// Never blink — overrides the terminal's blink state.
    Off,
    /// Always blink — overrides the terminal's blink state.
    On,
    /// Follow the terminal's cursor-blinking escape sequence.
    Terminal,
}

impl Default for CursorPrefs {
    fn default() -> Self {
        Self {
            shape: vte::ansi::CursorShape::Block,
            blink: BlinkPolicy::Terminal,
        }
    }
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
    /// Buffered bytes from an OSC sequence that spans across `feed()` calls.
    /// The VT processor already consumed the first chunk, so this remainder
    /// is used only by the custom OSC scanner and is not fed to vte again.
    osc_remainder: Vec<u8>,
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
    pending_clipboard_load: Option<ClipboardLoad>,
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
    /// Cursor blinking policy (see [`BlinkPolicy`]).
    blink_policy: BlinkPolicy,
    /// Cursor shape baked into `TermConfig` at construction.  Used to
    /// detect "terminal still showing the default shape" in [`Self::cursor`].
    fallback_shape: vte::ansi::CursorShape,
    /// Currently configured cursor shape (updatable via
    /// [`Self::set_cursor_prefs`]).
    prefs_shape: vte::ansi::CursorShape,
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
    pub fn new(size: TermSize, scheme: ColorScheme, cursor: CursorPrefs) -> Self {
        let config = TermConfig {
            kitty_keyboard: true,
            default_cursor_style: vte::ansi::CursorStyle {
                shape: cursor.shape,
                // The fallback default never blinks; `BlinkPolicy` is
                // enforced in [`Self::cursor`] on top of this.
                blinking: false,
            },
            ..TermConfig::default()
        };
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
            osc_remainder: Vec::new(),
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
            blink_policy: cursor.blink,
            fallback_shape: cursor.shape,
            prefs_shape: cursor.shape,
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

        // ── APC / DCS / special CSI scan ───────────────────────────────
        let t_apc_elapsed = self.scan_special_sequences(bytes, &mut replies);

        // ── Unified OSC scan ─────────────────────────────────────────
        // Collect all OSC sequences; they are handled below AFTER the
        // VT parser has processed the corresponding bytes, so that
        // cursor-dependent handlers (e.g. iTerm2 inline image) see the
        // correct cursor position.
        let t_osc_start = std::time::Instant::now();
        let osc_prefix_len = self.osc_remainder.len();
        let mut osc_scan_buf;
        let osc_scan_bytes: &[u8] = if osc_prefix_len == 0 {
            bytes
        } else {
            osc_scan_buf = std::mem::take(&mut self.osc_remainder);
            osc_scan_buf.extend_from_slice(bytes);
            &osc_scan_buf
        };
        let (oscs, incomplete_osc_start) = scan_oscs_with_remainder(osc_scan_bytes);
        if let Some(start) = incomplete_osc_start {
            self.osc_remainder
                .extend_from_slice(&osc_scan_bytes[start..]);
        }
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
            // A match that starts in the previous chunk was already partially
            // consumed by vte. Dispatch its custom side effect before feeding
            // this chunk, but do not skip any current bytes from the VT parser.
            let crosses_feed_boundary = osc.byte_start < osc_prefix_len;
            let vt_osc_start = if crosses_feed_boundary {
                0
            } else {
                (osc.byte_start - osc_prefix_len) + shift
            };
            let vt_osc_end = if crosses_feed_boundary {
                0
            } else {
                (osc.byte_end - osc_prefix_len) + shift
            };

            // Process bytes before this OSC (cursor positioning, text, etc.).
            if !crosses_feed_boundary && vt_osc_start > prev_vt_off {
                self.processor
                    .advance(&mut self.term, &vt_bytes[prev_vt_off..vt_osc_start]);
            }

            self.dispatch_osc(osc, &mut replies);

            // Skip the OSC bytes — the VT parser never sees them.  This is
            // safe because Term's osc_dispatch ignores all our custom OSC
            // numbers.
            if !crosses_feed_boundary {
                prev_vt_off = vt_osc_end;
            }
        }

        // Process remaining bytes after the last OSC.
        if prev_vt_off < vt_bytes.len() {
            self.processor
                .advance(&mut self.term, &vt_bytes[prev_vt_off..]);
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
                    let cell_w = if cols > 0 {
                        (self.pixel_width / cols as u32) as u16
                    } else {
                        0
                    };
                    let cell_h = if rows > 0 {
                        (self.pixel_height / rows as u32) as u16
                    } else {
                        0
                    };
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
                Event::CursorBlinkingChange | Event::MouseCursorDirty | Event::Wakeup => {
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
                bytes.len(),
                elapsed,
                t_apc_elapsed,
                t_osc_elapsed,
                t_vt_elapsed,
                t_damage_elapsed,
                t_evt_elapsed,
            );
        }

        replies
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
            fg: if flags.contains(Flags::INVERSE) {
                bg
            } else {
                fg
            },
            bg: if flags.contains(Flags::INVERSE) {
                fg
            } else {
                bg
            },
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
                let rgb =
                    self.scheme.colors[named].unwrap_or_else(|| named_color_default_rgb(named));
                Rgba::from_u8(rgb.r, rgb.g, rgb.b, 255)
            }
            Color::Spec(rgb) => Rgba::from_u8(rgb.r, rgb.g, rgb.b, 255),
            Color::Indexed(idx) => self.scheme.colors[idx as usize]
                .map(|rgb| Rgba::from_u8(rgb.r, rgb.g, rgb.b, 255))
                .unwrap_or(Rgba::WHITE),
        }
    }
}

/// Parse an OSC colour payload in `"#RRGGBB"` hex format, stripping an
/// optional leading `#`.  Returns `None` on invalid input.
fn parse_osc_hex_rgb(s: &str) -> Option<Rgb> {
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Rgb { r, g, b })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_osc_survives_feed_boundary() {
        let size = TermSize::new(24, 80, 0, 0);
        let mut terminal = Terminal::new(size, ColorScheme::default(), CursorPrefs::default());

        terminal.feed(b"\x1b]7;file://host/tmp");
        assert_eq!(terminal.take_current_directory(), None);

        terminal.feed(b"\x07");
        assert_eq!(
            terminal.take_current_directory().as_deref(),
            Some("file://host/tmp")
        );
    }

    #[test]
    fn osc_dispatch_continues_after_cursor_color() {
        let size = TermSize::new(24, 80, 0, 0);
        let mut terminal = Terminal::new(size, ColorScheme::default(), CursorPrefs::default());

        terminal.feed(b"\x1b]12;#010203\x07\x1b]7;file://host/next\x07");

        assert_eq!(
            terminal.take_current_directory().as_deref(),
            Some("file://host/next")
        );
    }

    #[test]
    fn blink_policy_overrides_terminal_state() {
        let size = TermSize::new(24, 80, 0, 0);

        // Off policy: never blink, even after DECSCUSR requests it.
        let mut off = Terminal::new(
            size,
            ColorScheme::default(),
            CursorPrefs {
                shape: vte::ansi::CursorShape::Block,
                blink: BlinkPolicy::Off,
            },
        );
        off.feed(b"\x1b[?12h"); // ATTRIBUTE_BLINK: set blinking
        off.feed(b"\x1b[5 q"); // DECSCUSR: blinking block
        assert!(!off.cursor().style.blinking);

        // On policy: always blink, even after DECSCUSR disables it.
        let mut on = Terminal::new(
            size,
            ColorScheme::default(),
            CursorPrefs {
                shape: vte::ansi::CursorShape::Block,
                blink: BlinkPolicy::On,
            },
        );
        on.feed(b"\x1b[?12l"); // ATTRIBUTE_BLINK: unset blinking
        on.feed(b"\x1b[0 q"); // DECSCUSR: steady block
        assert!(on.cursor().style.blinking);
    }
}
