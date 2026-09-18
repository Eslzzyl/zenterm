//! Session construction boundary.
//!
//! [`SessionFactory`] owns the process-wide handles needed to create a
//! session.  [`SessionRequest`] is a point-in-time snapshot of user-visible
//! launch settings, so a config reload affects the next session without
//! mutating the factory's shared resources.

use std::path::PathBuf;
use std::sync::Arc;

use zenterm_config::Config;
use zenterm_core::Result;
use zenterm_core::size::TermSize;
use zenterm_core::theme::Theme;
use zenterm_render::callback::CallbackHandle;
use zenterm_term::ColorScheme;

use super::types::{SessionId, TerminalSession};
use crate::glyph_cache::SharedGlyphAtlas;
use crate::gpu::SharedGpuContext;

/// Immutable resources shared by every session created by one application.
#[derive(Clone)]
pub(crate) struct SessionFactory {
    gpu: SharedGpuContext,
    atlas: Arc<SharedGlyphAtlas>,
    callback: CallbackHandle,
    egui_ctx: egui::Context,
}

/// All configuration-derived inputs for one terminal launch.
///
/// This intentionally owns its values: callers must build a fresh request
/// after configuration or theme changes instead of relying on cached launch
/// settings.
pub(crate) struct SessionRequest {
    pub(super) id: SessionId,
    pub(super) size: TermSize,
    pub(super) scheme: ColorScheme,
    pub(super) scrollback_lines: usize,
    pub(super) cursor: zenterm_config::cursor::CursorConfig,
    pub(super) cwd: PathBuf,
    pub(super) save_to_clipboard: bool,
    pub(super) default_bg: egui::Color32,
}

impl SessionFactory {
    pub(crate) fn new(
        gpu: SharedGpuContext,
        atlas: Arc<SharedGlyphAtlas>,
        callback: CallbackHandle,
        egui_ctx: egui::Context,
    ) -> Self {
        Self {
            gpu,
            atlas,
            callback,
            egui_ctx,
        }
    }

    pub(crate) fn create(&self, request: SessionRequest) -> Result<TerminalSession> {
        TerminalSession::new(
            request,
            self.gpu.clone(),
            self.atlas.clone(),
            self.callback.clone(),
            self.egui_ctx.clone(),
        )
    }
}

impl SessionRequest {
    pub(crate) fn from_config(
        id: SessionId,
        cwd: PathBuf,
        config: &Config,
        theme: &Theme,
        default_bg: egui::Color32,
    ) -> Self {
        Self {
            id,
            size: TermSize::new(
                config.window.dimensions.lines,
                config.window.dimensions.columns,
                0,
                0,
            ),
            scheme: ColorScheme::from_theme(theme),
            scrollback_lines: config.terminal.scrollback_lines,
            cursor: config.cursor.clone(),
            cwd,
            save_to_clipboard: config.selection.save_to_clipboard,
            default_bg,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use zenterm_config::Config;

    use super::{SessionId, SessionRequest};

    #[test]
    fn request_snapshots_current_launch_configuration() {
        let mut config = Config::default();
        config.window.dimensions.lines = 42;
        config.window.dimensions.columns = 137;
        config.terminal.scrollback_lines = 4_096;
        config.selection.save_to_clipboard = true;
        config.cursor.blink_interval = 750;
        let theme = config.colors.to_theme(false);
        let cwd = PathBuf::from("session-request-cwd");

        let request = SessionRequest::from_config(
            SessionId::new(9),
            cwd.clone(),
            &config,
            &theme,
            egui::Color32::from_rgb(1, 2, 3),
        );

        assert_eq!(request.id, SessionId::new(9));
        assert_eq!(request.size.rows, 42);
        assert_eq!(request.size.cols, 137);
        assert_eq!(request.scrollback_lines, 4_096);
        assert!(request.save_to_clipboard);
        assert_eq!(request.cursor.blink_interval, 750);
        assert_eq!(request.cwd, cwd);
        assert_eq!(request.default_bg, egui::Color32::from_rgb(1, 2, 3));
    }
}
