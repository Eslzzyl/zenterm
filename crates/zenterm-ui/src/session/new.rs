//! Terminal session construction.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;

use zenterm_config::cursor::{Blinking, CursorShape};
use zenterm_core::Result;
use zenterm_core::size::TermSize;
use zenterm_render::callback::CallbackHandle;
use zenterm_term::{BlinkPolicy, CursorPrefs, Terminal};

use super::factory::SessionRequest;
use super::types::{
    NotificationState, SessionInputState, SessionNotificationState, SessionRuntime,
    SessionViewState, TerminalSession,
};
use crate::glyph_cache::SharedGlyphAtlas;
use crate::gpu::SharedGpuContext;

/// Derive the initial tab title from the fixed shell path.  The `.exe` suffix
/// is stripped on Windows for cleaner display.
fn detect_shell_name(shell: Option<&Path>) -> String {
    let shell = shell
        .map(Path::to_path_buf)
        .or_else(zenterm_pty::default_shell)
        .unwrap_or_else(|| PathBuf::from(if cfg!(windows) { "cmd.exe" } else { "terminal" }));

    shell
        .as_path()
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.trim_end_matches(".exe").to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "terminal".into())
}

fn is_directory(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_dir())
}

/// Resolve a platform-appropriate initial working directory.
pub(crate) fn default_working_directory() -> PathBuf {
    let env_names: &[&str] = if cfg!(windows) {
        &["USERPROFILE", "HOME"]
    } else {
        &["HOME"]
    };
    env_names
        .iter()
        .filter_map(|name| std::env::var_os(name).map(PathBuf::from))
        .find(|path| is_directory(path))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Use a persisted working directory only when it still names a directory.
/// Otherwise fall back to the platform default before the shell is spawned.
pub(crate) fn session_working_directory(saved: Option<&Path>) -> PathBuf {
    if let Some(path) = saved {
        if is_directory(path) {
            return path.to_path_buf();
        }
        log::warn!(
            "session cwd {:?} is unavailable; using platform default",
            path
        );
    }
    default_working_directory()
}

impl TerminalSession {
    /// Map the config cursor shape to the terminal's shape type.
    pub(crate) fn map_cursor_shape(
        shape: CursorShape,
    ) -> alacritty_terminal::vte::ansi::CursorShape {
        match shape {
            CursorShape::Block => alacritty_terminal::vte::ansi::CursorShape::Block,
            CursorShape::Beam => alacritty_terminal::vte::ansi::CursorShape::Beam,
            CursorShape::Underline => alacritty_terminal::vte::ansi::CursorShape::Underline,
        }
    }

    /// Map the config blinking mode to a terminal blink policy.
    pub(crate) fn map_blink_policy(blinking: Blinking) -> BlinkPolicy {
        match blinking {
            Blinking::Off => BlinkPolicy::Off,
            Blinking::On => BlinkPolicy::On,
            Blinking::Terminal => BlinkPolicy::Terminal,
        }
    }

    /// Construct a new session: spawn a PTY, initialise the terminal,
    /// measure cell geometry, and wire the wgpu callback.
    ///
    /// `egui_ctx` is stored internally and cloned into the PTY reader
    /// thread as a wakeup callback (`ctx.request_repaint()`) so that
    /// incoming PTY data wakes the egui event loop from idle.
    pub(super) fn new(
        request: SessionRequest,
        gpu: SharedGpuContext,
        atlas: Arc<SharedGlyphAtlas>,
        callback: CallbackHandle,
        egui_ctx: egui::Context,
    ) -> Result<Self> {
        let SessionRequest {
            id,
            size,
            scheme,
            scrollback_lines,
            cursor,
            shell,
            cwd,
            save_to_clipboard,
            default_bg,
        } = request;
        // Create a wakeup callback that the PTY reader thread calls
        // after each successful read.  This is the core of the event-
        // driven architecture: instead of the main thread polling PTY
        // every frame, the reader thread notifies the event loop.
        let wakeup = {
            let ctx = egui_ctx.clone();
            Box::new(move || ctx.request_repaint())
        };
        let mut pty = match shell.as_deref() {
            Some(shell) => {
                zenterm_pty::PtySession::spawn_with_shell(size, Some(wakeup), Some(&cwd), shell)?
            }
            None => zenterm_pty::PtySession::spawn_with_cwd(size, Some(wakeup), Some(&cwd))?,
        };
        let mut terminal = Terminal::new_with_scrollback(
            size,
            scheme,
            CursorPrefs {
                shape: Self::map_cursor_shape(cursor.style.shape),
                blink: Self::map_blink_policy(cursor.style.blinking),
            },
            scrollback_lines,
        );

        let (cell_width, cell_height) = atlas.cell_size();
        let cell_w = cell_width.ceil() as u32;
        let cell_h = cell_height.ceil() as u32;
        terminal.cell_pixel_width = cell_w;
        terminal.cell_pixel_height = cell_h;

        // Compute total pixel dimensions from initial cell size * rows/cols.
        let px_w = (size.cols as f32 * cell_width).ceil() as u16;
        let px_h = (size.rows as f32 * cell_height).ceil() as u16;
        terminal.pixel_width = px_w as u32;
        terminal.pixel_height = px_h as u32;
        // Propagate pixel dimensions to the PTY so TIOCGWINSZ reports them.
        if let Err(e) = pty.resize(TermSize::new(size.rows, size.cols, px_w, px_h)) {
            log::error!("failed to resize PTY with pixel dims: {e}");
        }

        // Initialise `last_vp_size_px` so the first render picks up the
        // resize correctly.  Starting at [0, 0] is fine; the first
        // `update_cell_instances` call will overwrite it.
        let (notification_resp_tx, notification_resp_rx) = mpsc::channel();
        Ok(Self {
            id,
            runtime: SessionRuntime {
                title: detect_shell_name(shell.as_deref()),
                title_override: None,
                seen_terminal_title: false,
                shell,
                cwd: Some(cwd),
                progress: zenterm_core::Progress::None,
                latest_semantic_prompt: None,
                terminal,
                pty,
                terminal_dirty: true,
                last_resize_at: None,
                pty_exited: false,
                exit_effect_sent: false,
                highlight_cursor_line: false,
                batch_buf: Vec::new(),
                pending_pty_data: std::collections::VecDeque::new(),
                pending_title: None,
            },
            view: SessionViewState {
                gpu,
                atlas,
                callback,
                cell_width,
                cell_height,
                last_vp_size_px: [0.0, 0.0],
                last_vp_origin_px: [0.0, 0.0],
                dock_vp_origin_px: [0.0, 0.0],
                dock_vp_size_px: [0.0, 0.0],
                default_bg,
                cached_bg: Vec::new(),
                cached_glyph_per_atlas: Vec::new(),
                cached_deco: Vec::new(),
                cached_image_below: Vec::new(),
                cached_image_above: Vec::new(),
                image_sources: std::collections::HashSet::new(),
            },
            input: SessionInputState {
                selecting: false,
                blink_interval: cursor.blink_interval,
                blink_timeout: cursor.blink_timeout,
                cursor_thickness: cursor.thickness,
                unfocused_hollow: cursor.unfocused_hollow,
                window_focused: true,
                blink_epoch: std::time::Instant::now(),
                save_to_clipboard,
                clipboard: arboard::Clipboard::new().ok(),
                preedit_text: None,
                url_open: true,
                url_hover_underline: true,
                hover_cell: None,
                detected_links: Vec::new(),
                url_click_handled: false,
                scrollbar_dragging: false,
                scrollbar_drag_start_y: 0.0,
                scrollbar_drag_start_offset: 0,
                scroll_accumulator_y: 0.0,
                sgr_mouse_buttons: Vec::new(),
                last_sgr_motion_pos: None,
            },
            notifications: SessionNotificationState {
                notification: NotificationState::None,
                badge_format: None,
                notification_resp_tx,
                notification_resp_rx,
                tab_active: false,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::session_working_directory;
    use std::path::PathBuf;

    fn test_directory(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("zenterm-session-new-{}-{name}", std::process::id()))
    }

    #[test]
    fn existing_session_directory_is_preserved() {
        let path = test_directory("existing");
        std::fs::create_dir_all(&path).unwrap();

        assert_eq!(session_working_directory(Some(&path)), path);

        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn unavailable_session_directory_falls_back_to_existing_directory() {
        let path = test_directory("missing");
        let resolved = session_working_directory(Some(&path));

        assert!(resolved.is_dir());
    }
}
