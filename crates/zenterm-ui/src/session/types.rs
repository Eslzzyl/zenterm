//! Core types for terminal sessions.
//!
//! Defines [`SessionId`], [`NotificationState`], and the
//! [`TerminalSession`] struct that represents a single terminal tab.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use zenterm_pty::PtySession;
use zenterm_render::callback::CallbackHandle;
use zenterm_render::CellInstance;
use zenterm_term::Terminal;

use crate::glyph_cache::SharedGlyphAtlas;
use crate::gpu::SharedGpuContext;

// ── SessionId ──────────────────────────────────────────────────────────

/// Unique identifier for a terminal session within an application
/// process.  Monotonically increasing; the next id is allocated by
/// the dock state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SessionId(pub u64);

impl SessionId {
    pub const fn new(id: u64) -> Self { Self(id) }
    pub const fn raw(self) -> u64 { self.0 }
}

// ── Notification state placeholder ─────────────────────────────────────

/// Per-session notification badge state.  Resolved from OSC 9 / OSC 99
/// / OSC 777 escape sequences.  Phase 2.4 (per `roadmap.md`) will
/// expand this with text payloads, timestamps, and click handlers.
/// A URL span detected in the visible grid.
#[derive(Debug, Clone)]
pub(crate) struct UrlSpan {
    pub row: usize,
    pub col_start: usize,
    pub col_end: usize,
    #[allow(dead_code)]
    pub url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum NotificationState {
    #[default]
    None,
    Bell,
    Pending,
    Desktop { title: String, body: String },
}

// ── TerminalSession ────────────────────────────────────────────────────

/// All state and behaviour for a single terminal session.
pub struct TerminalSession {
    // ── Identity ─────────────────────────────────────────────────────
    pub id: SessionId,
    pub title: String,
    pub cwd: Option<PathBuf>,
    pub git_branch: Option<String>,
    pub notification: NotificationState,
    /// ConEmu OSC 9;4 progress-bar state reported by the shell.
    pub progress: zenterm_core::Progress,
    /// Most recent FinalTerm OSC 133 semantic prompt marker.
    pub latest_semantic_prompt: Option<zenterm_core::SemanticPrompt>,

    // ── Per-session state ───────────────────────────────────────────
    pub terminal: Terminal,
    pub pty: PtySession,

    // ── Shared resources (Arc, owned by the app) ────────────────────
    pub(crate) gpu: SharedGpuContext,
    pub atlas: Arc<SharedGlyphAtlas>,
    pub callback: CallbackHandle,

    // ── Cell metrics ─────────────────────────────────────────────────
    pub cell_width: f32,
    pub cell_height: f32,

    // ── Viewport tracking (last dock viewport we rendered for) ───────
    pub last_vp_size_px: [f32; 2],
    pub last_vp_origin_px: [f32; 2],

    // ── Dock-area viewport (single callback coordinate system) ────────
    pub dock_vp_origin_px: [f32; 2],
    pub dock_vp_size_px: [f32; 2],

    // ── Per-session flags ───────────────────────────────────────────
    pub selecting: bool,
    pub terminal_dirty: bool,
    pub last_resize_at: Option<f64>,
    pub blink_interval: u64,
    /// Epoch timestamp for time-based cursor blink phase computation.
    /// Replaces the old `frame_count`-based approach so we don't need
    /// to increment a counter every frame.
    pub blink_epoch: std::time::Instant,
    pub pty_exited: bool,
    /// Whether we have already emitted [`SessionEffect::CloseWindow`] for
    /// this session.  Guards against repeated emissions across frames.
    pub exit_effect_sent: bool,

    /// Cursor line highlight (OSC 1337 HighlightCursorLine).
    pub highlight_cursor_line: bool,
    /// Badge format template (OSC 1337 SetBadgeFormat).
    /// `None` = no badge; `Some(template)` = renders the badge.
    pub badge_format: Option<String>,

    // ── IME preedit (composition) text ────────────────────────────
    //
    // When the user is composing text with an IME (e.g. Chinese pinyin),
    // the preedit string is stored here and rendered directly through
    // the GPU glyph pipeline at the cursor position, matching the
    // terminal text style exactly.
    pub(crate) preedit_text: Option<String>,

    // ── Theming ─────────────────────────────────────────────────────
    pub default_bg: egui::Color32,

    // ── Cell-instance cache (avoids full rebuild when terminal is idle) ──
    pub(crate) cached_bg: Vec<CellInstance>,
    /// Per-atlas-slot glyph instance caches.  Indexed by atlas_index;
    /// grows dynamically as new slots are created.  Each inner vec holds
    /// the instances that belong to that slot's GPU texture.
    pub(crate) cached_glyph_per_atlas: Vec<Vec<CellInstance>>,
    pub(crate) cached_deco: Vec<CellInstance>,
    /// Image quads with z_index < 0 (render behind text), per atlas slot.
    pub(crate) cached_image_below: Vec<Vec<CellInstance>>,
    /// Image quads with z_index >= 0 (render on top of text), per atlas slot.
    pub(crate) cached_image_above: Vec<Vec<CellInstance>>,

    /// ── Title debounce ──────────────────────────────────────────────────
    ///
    /// Some shells (fish, zsh with plugins) send a transient title event
    /// (e.g. the command name "ls") just before executing a command, and
    /// then the real prompt title (e.g. "~") shortly after.  Without
    /// debouncing, both reach the UI as separate frames, causing a visible
    /// flicker.
    ///
    /// We buffer the incoming title and only apply it once it has been
    /// stable for [`TITLE_DEBOUNCE_MS`].
    pub(crate) pending_title: Option<(String, Instant)>,

    // ── URL detection ──────────────────────────────────────────────────
    pub(crate) url_open: bool,
    pub(crate) url_hover_underline: bool,
    /// Mouse-hovered cell position, updated every frame by `handle_mouse`.
    pub(crate) hover_cell: Option<(usize, usize)>,
    /// Cached URL spans for the visible grid, rebuilt on dirty.
    pub(crate) url_spans: Vec<UrlSpan>,
    /// Guards against processing the same Ctrl+Click across multiple frames.
    ///
    /// # Workaround
    ///
    /// `egui::Response::clicked()` sometimes returns `true` for two
    /// consecutive frames (root cause not yet identified).  Without this
    /// guard a single Ctrl+Click would open the URL twice.
    ///
    /// Set to `true` after opening a URL; cleared on the next click that
    /// does not open a URL.
    pub(crate) url_click_handled: bool,

    // ── Scrollbar state ────────────────────────────────────────────────
    pub(crate) scrollbar_dragging: bool,
    pub(crate) scrollbar_drag_start_y: f32,
    pub(crate) scrollbar_drag_start_offset: usize,

    // ── Scroll accumulation (alacritty-style pixel accumulator) ─────────
    // Accumulates sub-cell scroll deltas from the trackpad across frames.
    // Extracted as whole lines by dividing by cell_height; remainder is
    // preserved via `%= cell_height` to avoid losing fractional deltas.
    pub(crate) scroll_accumulator_y: f64,

    // ── SGR mouse state ─────────────────────────────────────────────────
    /// Tracked mouse-button codes for correct release encoding.
    /// Each entry is `base_button | mod_bits` (base: 0=left, 1=middle, 2=right).
    /// Popped on release so the release event carries the right button code.
    pub(crate) sgr_mouse_buttons: Vec<u8>,
    /// Last cell position for which we sent an SGR motion event.
    /// Used to suppress duplicate motion events when the pointer hasn't
    /// moved to a new cell.
    pub(crate) last_sgr_motion_pos: Option<(usize, usize)>,

    // ── Kitty OSC 99: notification response channel ──────────────────
    /// Sender half: cloned into notification threads so they can write
    /// escape-sequence responses back to the PTY (for `a=report`, `c=1`,
    /// and button clicks).
    pub(crate) notification_resp_tx: mpsc::Sender<String>,
    /// Receiver half: drained in [`Self::pump_pty`] after each feed.
    pub(crate) notification_resp_rx: mpsc::Receiver<String>,

    // ── Window / tab focus state (for Kitty OSC 99 `o=` filtering) ──
    /// Whether this session's tab is the currently active tab.
    /// Set by the app layer before [`Self::handle_side_effects`].
    pub(crate) tab_active: bool,
}

// ── Constants ──────────────────────────────────────────────────────────

/// Pixel width of the overlay scrollbar.
pub(crate) const SCROLLBAR_WIDTH: f32 = 10.0;

/// Minimum pixel height of the scrollbar thumb.
pub(crate) const SCROLLBAR_MIN_THUMB_HEIGHT: f32 = 24.0;

/// Debounce period for window/tab title updates (milliseconds).
///
/// Shells like fish send a transient title (the command name) just before
/// executing a command, then the real prompt title shortly after.  Without
/// debouncing both reach the UI as separate frames, causing a visible
/// flicker.  This value should be longer than the typical gap between the
/// pre-exec and post-exec title events (usually < 20 ms on a local PTY).
pub(crate) const TITLE_DEBOUNCE_MS: f64 = 80.0;
