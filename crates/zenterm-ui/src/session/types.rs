//! Core types for terminal sessions.
//!
//! Defines [`SessionId`], [`NotificationState`], and the
//! [`TerminalSession`] struct that represents a single terminal tab.

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use zenterm_pty::PtySession;
use zenterm_render::CellInstance;
use zenterm_render::callback::CallbackHandle;
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
    pub const fn new(id: u64) -> Self {
        Self(id)
    }
    pub const fn raw(self) -> u64 {
        self.0
    }
}

// ── Notification state placeholder ─────────────────────────────────────

/// Per-session notification badge state.  Resolved from OSC 9 / OSC 99
/// / OSC 777 escape sequences.  Phase 2.4 (per `roadmap.md`) will
/// expand this with text payloads, timestamps, and click handlers.
/// The kind of hyperlink detected in terminal output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetectedLinkKind {
    /// A URL found in visible text.
    Url,
    /// An email address found in visible text.
    Email,
    /// An explicit OSC 8 hyperlink.
    Osc8,
}

/// One visible row segment belonging to a detected hyperlink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LinkSegment {
    pub row: usize,
    pub col_start: usize,
    pub col_end: usize,
}

/// A hyperlink with its displayed range and normalized launch target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DetectedLink {
    pub kind: DetectedLinkKind,
    /// Text or OSC 8 target as it appeared in the terminal data.
    pub original: String,
    /// Target passed to the OS opener after normalization and policy checks.
    pub target: String,
    pub segments: Vec<LinkSegment>,
}

impl DetectedLink {
    pub fn contains_cell(&self, row: usize, col: usize) -> bool {
        self.segments
            .iter()
            .any(|segment| segment.row == row && col >= segment.col_start && col < segment.col_end)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum NotificationState {
    #[default]
    None,
    Bell,
    /// Reserved for notification protocols that expose an unresolved state.
    #[allow(dead_code)]
    Pending,
    /// Reserved for platform notification backends with a visible payload.
    #[allow(dead_code)]
    Desktop {
        title: String,
        body: String,
    },
}

// ── TerminalSession ────────────────────────────────────────────────────

/// PTY, terminal-core, and terminal-protocol state for one session.
pub(super) struct SessionRuntime {
    pub(super) title: String,
    pub(super) title_override: Option<String>,
    pub(super) seen_terminal_title: bool,
    pub(super) shell: Option<PathBuf>,
    pub(super) cwd: Option<PathBuf>,
    pub(super) progress: zenterm_core::Progress,
    pub(super) latest_semantic_prompt: Option<zenterm_core::SemanticPrompt>,
    pub(super) terminal: Terminal,
    pub(super) pty: PtySession,
    pub(super) terminal_dirty: bool,
    pub(super) last_resize_at: Option<f64>,
    pub(super) pty_exited: bool,
    pub(super) exit_effect_sent: bool,
    pub(super) highlight_cursor_line: bool,
    pub(super) batch_buf: Vec<u8>,
    pub(super) pending_pty_data: VecDeque<Vec<u8>>,
    pub(super) pending_title: Option<(String, Instant)>,
}

/// Per-session GPU resources, geometry, and reusable render caches.
pub(super) struct SessionViewState {
    pub(super) gpu: SharedGpuContext,
    pub(super) atlas: Arc<SharedGlyphAtlas>,
    pub(super) callback: CallbackHandle,
    pub(super) cell_width: f32,
    pub(super) cell_height: f32,
    pub(super) last_vp_size_px: [f32; 2],
    pub(super) last_vp_origin_px: [f32; 2],
    pub(super) dock_vp_origin_px: [f32; 2],
    pub(super) dock_vp_size_px: [f32; 2],
    pub(super) default_bg: egui::Color32,
    pub(super) cached_bg: Vec<CellInstance>,
    pub(super) cached_glyph_per_atlas: Vec<Vec<CellInstance>>,
    pub(super) cached_deco: Vec<CellInstance>,
    pub(super) cached_image_below: Vec<Vec<CellInstance>>,
    pub(super) cached_image_above: Vec<Vec<CellInstance>>,
    /// Image source identities registered in the shared GPU cache.
    pub(super) image_sources: HashSet<usize>,
}

/// User-input, selection, clipboard, and pointer interaction state.
pub(super) struct SessionInputState {
    pub(super) selecting: bool,
    pub(super) blink_interval: u64,
    pub(super) blink_timeout: u64,
    pub(super) cursor_thickness: f32,
    pub(super) unfocused_hollow: bool,
    pub(super) window_focused: bool,
    pub(super) blink_epoch: Instant,
    pub(super) last_cursor_blink_on: Option<bool>,
    pub(super) save_to_clipboard: bool,
    pub(super) clipboard: Option<arboard::Clipboard>,
    pub(super) preedit_text: Option<String>,
    pub(super) url_open: bool,
    pub(super) url_hover_underline: bool,
    pub(super) hover_cell: Option<(usize, usize)>,
    pub(super) hovered_link: Option<usize>,
    pub(super) detected_links: Vec<DetectedLink>,
    pub(super) url_click_handled: bool,
    pub(super) scrollbar_dragging: bool,
    pub(super) scrollbar_drag_start_y: f32,
    pub(super) scrollbar_drag_start_offset: usize,
    pub(super) scroll_accumulator_y: f64,
    pub(super) sgr_mouse_buttons: Vec<u8>,
    pub(super) last_sgr_motion_pos: Option<(usize, usize)>,
}

/// Displayed notifications and the Kitty OSC 99 response channel.
pub(super) struct SessionNotificationState {
    pub(super) notification: NotificationState,
    pub(super) badge_format: Option<String>,
    pub(super) notification_resp_tx: mpsc::Sender<String>,
    pub(super) notification_resp_rx: mpsc::Receiver<String>,
    pub(super) tab_active: bool,
}

/// Coordinates the independent runtime, view, input, and notification state
/// of a single terminal tab.
pub struct TerminalSession {
    pub id: SessionId,
    pub(super) runtime: SessionRuntime,
    pub(super) view: SessionViewState,
    pub(super) input: SessionInputState,
    pub(super) notifications: SessionNotificationState,
}

impl Drop for SessionViewState {
    fn drop(&mut self) {
        for source_id in self.image_sources.drain() {
            self.atlas.release_image_source(source_id);
        }
    }
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

// ── Title resolution ─────────────────────────────────────────────────

impl TerminalSession {
    /// Expose the terminal core for read-only UI decisions without exposing
    /// session storage layout to application modules.
    pub(crate) fn terminal(&self) -> &Terminal {
        &self.runtime.terminal
    }

    /// Mutably access the terminal core for the small set of app-level
    /// operations that are not yet session-owned behaviours.
    pub(crate) fn terminal_mut(&mut self) -> &mut Terminal {
        &mut self.runtime.terminal
    }

    /// Access the PTY only through the session boundary.
    pub(crate) fn pty_mut(&mut self) -> &mut PtySession {
        &mut self.runtime.pty
    }

    /// Access the persistent clipboard handle through the input boundary.
    pub(crate) fn clipboard_mut(&mut self) -> Option<&mut arboard::Clipboard> {
        self.input.clipboard.as_mut()
    }

    pub(crate) fn title(&self) -> &str {
        &self.runtime.title
    }

    pub(crate) fn title_override(&self) -> Option<&str> {
        self.runtime.title_override.as_deref()
    }

    pub(crate) fn set_title_override(&mut self, title: String) {
        self.runtime.title_override = Some(title);
    }

    pub(crate) fn clear_title_override(&mut self) {
        self.runtime.title_override = None;
    }

    pub(crate) fn working_directory(&self) -> Option<&std::path::Path> {
        self.runtime.cwd.as_deref()
    }

    pub(crate) fn shell_path(&self) -> Option<&std::path::Path> {
        self.runtime.shell.as_deref()
    }

    pub(crate) fn progress(&self) -> zenterm_core::Progress {
        self.runtime.progress
    }

    pub(crate) fn notification(&self) -> &NotificationState {
        &self.notifications.notification
    }

    pub(crate) fn background_color(&self) -> egui::Color32 {
        self.view.default_bg
    }

    pub(crate) fn paint_callback(&self) -> CallbackHandle {
        self.view.callback.clone()
    }

    pub(crate) fn cell_height(&self) -> f32 {
        self.view.cell_height
    }

    pub(crate) fn viewport_rect(&self, pixels_per_point: f32) -> egui::Rect {
        egui::Rect::from_min_size(
            egui::pos2(
                self.view.last_vp_origin_px[0] / pixels_per_point,
                self.view.last_vp_origin_px[1] / pixels_per_point,
            ),
            egui::vec2(
                self.view.last_vp_size_px[0] / pixels_per_point,
                self.view.last_vp_size_px[1] / pixels_per_point,
            ),
        )
    }

    pub(crate) fn resize_overlay_rect(&self, pixels_per_point: f32) -> Option<egui::Rect> {
        self.runtime
            .last_resize_at
            .map(|_| self.viewport_rect(pixels_per_point))
    }

    pub(crate) fn badge_format(&self) -> Option<&str> {
        self.notifications.badge_format.as_deref()
    }

    pub(crate) fn set_theme(
        &mut self,
        scheme: zenterm_term::ColorScheme,
        background: egui::Color32,
    ) {
        self.runtime.terminal.set_scheme(scheme);
        self.view.default_bg = background;
        self.runtime.terminal_dirty = true;
    }

    pub(crate) fn set_save_to_clipboard(&mut self, enabled: bool) {
        self.input.save_to_clipboard = enabled;
    }

    pub(crate) fn mark_terminal_dirty(&mut self) {
        self.runtime.terminal_dirty = true;
    }

    pub(crate) fn note_input_activity(&mut self) {
        self.input.blink_epoch = Instant::now();
        self.input.last_cursor_blink_on = None;
        self.runtime.terminal_dirty = true;
    }

    pub(crate) fn set_preedit_text(&mut self, text: Option<String>) {
        self.input.preedit_text = text.filter(|text| !text.is_empty());
        self.runtime.terminal_dirty = true;
    }

    /// Apply focus state and return the repaint interval when the current
    /// cursor style is actively blinking.
    pub(crate) fn update_window_focus(&mut self, focused: bool) -> Option<std::time::Duration> {
        if self.input.window_focused != focused {
            self.input.window_focused = focused;
            self.input.last_cursor_blink_on = None;
            self.runtime.terminal_dirty = true;
        }
        let cursor = self.runtime.terminal.cursor();
        let blinking = cursor.style.blinking
            && !matches!(
                cursor.style.shape,
                alacritty_terminal::vte::ansi::CursorShape::Block
            );
        if blinking {
            let elapsed = self.input.blink_epoch.elapsed().as_millis();
            let timeout_ms = self.input.blink_timeout.saturating_mul(1000) as u128;
            let blink_on = if timeout_ms > 0 && elapsed >= timeout_ms {
                true
            } else {
                let period = self.input.blink_interval.max(100) as u128 * 2;
                (elapsed % period) < period / 2
            };
            if self.input.last_cursor_blink_on != Some(blink_on) {
                self.input.last_cursor_blink_on = Some(blink_on);
                self.runtime.terminal_dirty = true;
            }
            if timeout_ms > 0 && elapsed >= timeout_ms {
                None
            } else {
                Some(std::time::Duration::from_millis(
                    self.input.blink_interval.max(100),
                ))
            }
        } else {
            if self.input.last_cursor_blink_on.take().is_some() {
                self.runtime.terminal_dirty = true;
            }
            None
        }
    }

    pub(crate) fn ime_cursor_rect(&self, pixels_per_point: f32) -> egui::Rect {
        let cursor = self.runtime.terminal.cursor();
        let origin_x = self.view.last_vp_origin_px[0] / pixels_per_point;
        let origin_y = self.view.last_vp_origin_px[1] / pixels_per_point;
        egui::Rect::from_min_size(
            egui::pos2(
                origin_x + cursor.pos.column as f32 * self.view.cell_width / pixels_per_point,
                origin_y + cursor.pos.line as f32 * self.view.cell_height / pixels_per_point,
            ),
            egui::vec2(
                self.view.cell_width / pixels_per_point,
                self.view.cell_height / pixels_per_point,
            ),
        )
    }

    pub(crate) fn set_tab_active(&mut self, active: bool) {
        self.notifications.tab_active = active;
    }

    /// Resolve the effective display title using the priority chain:
    ///
    /// 1. [`Self::title_override`] — manually set by user (highest priority)
    /// 2. [`Self::title`] — OSC-set terminal title (only if
    ///    [`Self::seen_terminal_title`] is true and title is non-empty)
    /// 3. **Inferred title** — basename of [`Self::cwd`] (working directory)
    /// 4. `"terminal"` — ultimate hardcoded fallback
    pub fn title_effective(&self) -> String {
        // ① Manual override
        if let Some(ref t) = self.runtime.title_override
            && !t.is_empty()
        {
            return t.clone();
        }

        // ② Terminal / initial title (non-empty)
        // This covers both the OSC-set terminal title and the initial
        // best-guess title set by the constructor (e.g. "bash" from
        // `$SHELL`).  We do NOT gate on `seen_terminal_title` here so
        // that the initial title shows immediately at startup; once the
        // shell sends a real OSC title it replaces this value.
        if !self.runtime.title.is_empty() {
            return self.runtime.title.clone();
        }

        // ③ Inferred from cwd basename
        if let Some(ref cwd) = self.runtime.cwd
            && let Some(name) = cwd.file_name().and_then(|n| n.to_str())
            && !name.is_empty()
        {
            return name.to_string();
        }

        // ④ Ultimate fallback
        "terminal".to_string()
    }
}
