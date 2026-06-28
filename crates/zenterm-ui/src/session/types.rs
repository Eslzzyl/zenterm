//! Core types for terminal sessions.
//!
//! Defines [`SessionId`], [`NotificationState`], and the
//! [`TerminalSession`] struct that represents a single terminal tab.

use std::path::PathBuf;
use std::sync::Arc;
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum NotificationState {
    #[default]
    None,
    Bell,
    Pending,
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
    pub frame_count: u64,
    pub blink_interval: u64,
    pub pty_exited: bool,
    /// Whether we have already emitted [`SessionEffect::CloseWindow`] for
    /// this session.  Guards against repeated emissions across frames.
    pub exit_effect_sent: bool,

    // ── Theming ─────────────────────────────────────────────────────
    pub default_bg: egui::Color32,

    // ── Cell-instance cache (avoids full rebuild when terminal is idle) ──
    pub(crate) cached_bg: Vec<CellInstance>,
    pub(crate) cached_glyph: Vec<CellInstance>,
    pub(crate) cached_deco: Vec<CellInstance>,

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

    // ── Scrollbar state ────────────────────────────────────────────────
    pub(crate) scrollbar_dragging: bool,
    pub(crate) scrollbar_drag_start_y: f32,
    pub(crate) scrollbar_drag_start_offset: usize,
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
