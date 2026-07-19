//! The main eframe application for Zenterm.
//!
//! Wires together the PTY session(s), terminal state machine(s),
//! glyph atlas, GPU renderer, and input mapper into a single egui
//! application.  As of the multi-tab refactor (`Phase 2` per
//! `docs/roadmap.md`), this is a thin orchestrator over a
//! `HashMap<SessionId, TerminalSession>`; the heavy lifting lives
//! in [`crate::session`], [`crate::tab_viewer`], [`crate::sidebar`],
//! and [`crate::legacy`].
//!
//! Workspaces are managed by [`crate::workspace::WorkspaceManager`].

pub mod config;
pub mod dock;
pub mod keyboard;
pub mod persistence;
pub mod session_lifecycle;
pub mod settings;
pub mod theme;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::Context;

use zenterm_config::Config;
use zenterm_core::SubpixelLayout;
use zenterm_core::theme::{Theme, ThemePreference, THEME_DARK};
use zenterm_render::callback::{CallbackHandle, SharedRenderState, TerminalWgpuCallback};
use zenterm_term::ColorScheme;

use crate::glyph_cache::SharedGlyphAtlas;
use crate::gpu::SharedGpuContext;
use crate::layout_io::LayoutIo;
use crate::legacy::render_legacy_single;
use crate::session::{SessionId, TerminalSession};
use crate::settings::SettingsState;
use crate::workspace::WorkspaceManager;

// ── App-level state ────────────────────────────────────────────────────

/// The top-level eframe application state.
pub struct ZentermApp {
    // ── Shared GPU / atlas ─────────────────────────────────────────
    gpu: SharedGpuContext,
    pub atlas: std::sync::Arc<SharedGlyphAtlas>,
    pub callback: CallbackHandle,

    // ── Multi-session state ────────────────────────────────────────
    pub sessions: HashMap<SessionId, TerminalSession>,
    pub workspaces: WorkspaceManager,
    pub active_session_id: Option<SessionId>,

    // ── Layout persistence ─────────────────────────────────────────
    pub layout_io: LayoutIo,
    layout_dirty: bool,
    last_persist_at: Option<Instant>,

    // ── App-level state (not per-session) ──────────────────────────
    pub config: Config,
    settings_state: SettingsState,
    pub theme: Theme,
    pub theme_preference: ThemePreference,
    pub last_system_dark: bool,
    pub default_bg: egui::Color32,
    pub pixels_per_point: f32,
    pub error_toast: Option<String>,

    // ── Pending actions accumulated by the dock viewer ─────────────
    pending_close: Vec<SessionId>,
    pending_adds: u32,

    /// Last window title sent to the OS.  Used to deduplicate
    /// `ViewportCommand::Title` so we don't call `[NSWindow setTitle:]`
    /// with the same string on every shell prompt (which can cause
    /// unnecessary title-bar redraws on some platforms).
    current_window_title: Option<String>,

    // ── Config persistence ──────────────────────────────────────
    /// Whether the in-memory config has changed since the last disk
    /// write.  Set by both settings-panel changes and window-resize
    /// tracking; cleared after a successful [`Config::save`].
    config_dirty: bool,
    /// Timestamp of the most recent config change.  Used to debounce
    /// the disk write so rapid changes (e.g. slider drag or window
    /// resize) don't thrash the I/O.
    last_config_save_at: Option<Instant>,

    // ── Event-driven wakeup (idle CPU) ──────────────────────────
    /// Stored egui context, used to create cross-thread wakeup
    /// callbacks for PTY reader threads.  Clone is cheap (Arc bump).
    /// Set from the `CreationContext` during construction.
    pub(crate) egui_ctx: egui::Context,
}

// ── Construction ───────────────────────────────────────────────────────

impl ZentermApp {
    pub fn new_with_wgpu(
        egui_ctx: egui::Context,
        device: wgpu::Device,
        queue: wgpu::Queue,
        target_format: wgpu::TextureFormat,
        pixels_per_point: f32,
        config: Config,
    ) -> Self {
        let shared = std::sync::Arc::new(SharedRenderState::new(80 * 24));
        let gpu = SharedGpuContext::new(device, queue, target_format, shared.clone());

        // ── Glyph atlas (shared across all sessions) ───────────────
        let font_size = config.font.size * pixels_per_point;
        let font_family = std::borrow::Cow::Owned(config.font.normal.family.clone());
        let atlas = std::sync::Arc::new(SharedGlyphAtlas::new(
            font_size,
            font_family,
            pixels_per_point,
            SubpixelLayout::detect(),
            config.font.ligatures,
            config.font.hinting,
            config.font.render_mode,
            shared.clone(),
        ));
        atlas.seed_ascii();
        // Push the seeded atlas to the GPU channel so the very first
        // prepare() picks up glyphs instead of an all-zero texture.
        atlas.sync_to_gpu();

        // ── Wgpu callback (one per app; reads the shared instance
        //    buffer built by every session's `update_cell_instances`)
        let callback = TerminalWgpuCallback::new(
            (*gpu.device).clone(),
            (*gpu.queue).clone(),
            target_format,
            shared.clone(),
        );
        let callback = CallbackHandle::new(callback);

        // ── Theme + colour scheme (default = dark) ────────────────
        let theme = THEME_DARK.clone();
        let default_bg = theme_bg_to_color32(&theme);
        let scheme = ColorScheme::from_theme(&theme);

        // ── Layout persistence ────────────────────────────────────
        let config_path = Config::path();
        let layout_io = LayoutIo::from_config_path(&config_path);

        // ── First session (always present, even when tabs are off)
        let first_id = SessionId::new(0);
        let size = zenterm_core::size::TermSize::new(
            config.window.dimensions.lines,
            config.window.dimensions.columns,
            0,
            0,
        );
        let mut session = TerminalSession::new(
            first_id,
            size,
            scheme.clone(),
            config.cursor.blink_interval,
            default_bg,
            gpu.clone(),
            atlas.clone(),
            callback.clone(),
            egui_ctx.clone(),
        );
        session.title = "shell".into();
        let mut sessions = HashMap::new();
        sessions.insert(first_id, session);

        // ── Restore workspaces if config says so ──────────────────
        let mut workspaces = WorkspaceManager::new();
        let mut restored_session_ids: Vec<SessionId> = Vec::new();
        if config.ui.restore_layout_on_startup {
            if let Some(persisted) = layout_io.load_layout() {
                for pw in &persisted.workspaces {
                    for (_, tab) in pw.dock.iter_all_tabs() {
                        restored_session_ids.push(*tab);
                    }
                }
                let mut ws_states = Vec::new();
                for pw in &persisted.workspaces {
                    let ws_id = crate::workspace::WorkspaceId::new(pw.id);
                    ws_states.push(crate::workspace::WorkspaceState::from_dock(
                        ws_id,
                        pw.name.clone(),
                        pw.dock.clone(),
                    ));
                }
                if !ws_states.is_empty() {
                    let active_ws_id =
                        crate::workspace::WorkspaceId::new(persisted.active_workspace_id);
                    workspaces = WorkspaceManager::from_persisted(
                        ws_states,
                        active_ws_id,
                        persisted.next_session_id,
                        persisted.next_workspace_id,
                    );
                }
            }
        }

        // Create TerminalSessions for all persisted tab ids that
        // don't already exist (id 0 was created above).
        for sid in &restored_session_ids {
            if sessions.contains_key(sid) {
                continue;
            }
            let mut s = TerminalSession::new(
                *sid,
                size,
                scheme.clone(),
                config.cursor.blink_interval,
                default_bg,
                gpu.clone(),
                atlas.clone(),
                callback.clone(),
                egui_ctx.clone(),
            );
            s.title = "shell".into();
            sessions.insert(*sid, s);
        }

        // Ensure the first session is registered in the active workspace.
        if workspaces.active_workspace().all_tab_ids().is_empty() {
            workspaces.active_workspace_mut().new_tab(first_id);
        }

        // Hydrate session titles / cwd from sessions.json.
        let saved_meta = layout_io.load_sessions();
        for (id, meta) in saved_meta {
            if let Some(session) = sessions.get_mut(&SessionId(id)) {
                if !meta.title.is_empty() {
                    session.title = meta.title;
                }
                if let Some(cwd) = meta.cwd {
                    session.cwd = Some(cwd);
                }
            }
        }

        // Determine the active session from the active workspace.
        let active_session_id = workspaces
            .active_workspace()
            .all_tab_ids()
            .first()
            .copied()
            .or(Some(first_id));

        Self {
            gpu,
            atlas,
            callback,
            sessions,
            workspaces,
            active_session_id,
            layout_io,
            layout_dirty: false,
            last_persist_at: None,
            settings_state: SettingsState::new(&config),
            config,
            theme,
            theme_preference: ThemePreference::Dark,
            last_system_dark: true,
            default_bg,
            pixels_per_point,
            error_toast: None,
            pending_close: Vec::new(),
            pending_adds: 0,
            current_window_title: None,
            config_dirty: false,
            last_config_save_at: None,
            egui_ctx,
        }
    }

}

impl eframe::App for ZentermApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &Context, frame: &mut eframe::Frame) {
        // 0. Theme sync.
        self.sync_theme(ctx);

        // 0.5. DPI change.
        let current_ppp = ctx.pixels_per_point();
        if (current_ppp - self.pixels_per_point).abs() > 0.01 {
            for (_, session) in self.sessions.iter_mut() {
                session.reinit_for_dpi(current_ppp, self.config.font.ligatures);
            }
            self.pixels_per_point = current_ppp;
        }

        // 1. Pump PTY for every session.
        self.pump_pty_active_sessions();

        // 1.5. Side-effects.
        self.handle_side_effects(ctx);

        // 1.6. Close the application if ALL sessions have exited AND
        //      there is no further user input expected (currently:
        //      only when there are no sessions at all).
        if self.sessions.is_empty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return; // Don't render an empty frame — avoids a black flash.
        }

        // 2. Keyboard shortcuts (copy/paste/reload/settings).
        if self.handle_shortcuts(ctx) {
            // skip forwarding (the shortcut consumed the event)
        } else if !self.settings_state.open {
            // Don't forward keyboard when the settings panel is open.
            self.feed_keyboard_to_active(ctx);
        }

        // 3. Cursor blink — schedule timer-based ticks.
        //
        // Uses `request_repaint_after` (an OS-level timer, zero CPU while
        // waiting) instead of incrementing a `frame_count` every frame.
        // The blink phase is computed in `update_cell_instances` using
        // elapsed time since `blink_epoch`, so no per-frame state is
        // needed — the timer merely ensures we wake up to re-render when
        // the phase toggles.
        for (_, session) in self.sessions.iter_mut() {
            let blinking = session.terminal.cursor().style.blinking
                && !matches!(
                    session.terminal.cursor().style.shape,
                    alacritty_terminal::vte::ansi::CursorShape::Block
                );
            if blinking {
                session.terminal_dirty = true;
                ctx.request_repaint_after(Duration::from_millis(session.blink_interval));
            }
        }

        // 4. Render the main UI (terminal, tabs, sidebar) — only when
        //    something has actually changed.
        //
        // In the event-driven architecture we avoid rendering on every
        // frame.  Instead, we check for pending work:
        //   - `terminal_dirty`: at least one session has new terminal
        //     content (PTY data, cursor blink, input, config change)
        //   - `layout_dirty`: the dock/tab layout changed (session
        //     created, closed, or workspace switched)
        //
        // If nothing is dirty we skip the CentralPanel entirely, which
        // means:
        //   - `clear_instances` / `clear_atlas_ranges` are NOT called
        //   - `bump_instance_gen` is NOT called → GPU instance gen
        //     stays unchanged → GPU `prepare()` skips buffer upload
        //   - egui still paints whatever its immediate-mode UI produces
        //     (the empty CentralPanel draws nothing, but other panels
        //     like the settings window continue to work)
        let needs_render = self.layout_dirty
            || self.sessions.values().any(|s| s.terminal_dirty);
        if needs_render {
            #[allow(deprecated)]
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
                self.ui(ui, frame);
            });
        }

        // 4.5. Enable IME when the terminal has keyboard focus (no egui
        //      widget focused).  This tells egui-winit → winit to call
        //      `set_ime_allowed(true)`, which on macOS enables
        //      `interpretKeyEvents:` in keyDown so the IME (Chinese /
        //      Japanese / Korean input methods) receives key events.
        //      On Windows/Linux this enables the platform IME as well.
        //
        //      NOTE: egui-winit uses `ime.rect` (not `ime.cursor_rect`)
        //      for `set_ime_cursor_area`.  We therefore set the rect to
        //      a small area around the cursor position so the IME
        //      candidate window appears at the cursor, not at the
        //      viewport origin.
        if ctx.memory(|m| m.focused().is_none()) && !self.settings_state.open {
            if let Some(id) = self.active_session_id {
                if let Some(session) = self.sessions.get(&id) {
                    let ppp = ctx.pixels_per_point();
                    let ox = session.last_vp_origin_px[0] / ppp;
                    let oy = session.last_vp_origin_px[1] / ppp;
                    let cursor = session.terminal.cursor();

                    // Position the IME candidate window at the cursor.
                    // We use a cursor-sized rect so the IME window
                    // appears anchored to the cursor position.
                    let cursor_x = ox + cursor.pos.column as f32 * session.cell_width / ppp;
                    let cursor_y = oy + cursor.pos.line as f32 * session.cell_height / ppp;
                    let cursor_w = session.cell_width / ppp;
                    let cursor_h = session.cell_height / ppp;

                    let cursor_rect = egui::Rect::from_min_size(
                        egui::pos2(cursor_x, cursor_y),
                        egui::vec2(cursor_w, cursor_h),
                    );

                    ctx.output_mut(|o| {
                        o.ime = Some(egui::output::IMEOutput {
                            rect: cursor_rect,
                            cursor_rect,
                        });
                    });
                }
            }
        }

        // 5. Render the settings panel (separate native window).
        if self.settings_state.open {
            self.render_settings_viewport(ctx);
        }

        // 6. Track terminal grid dimensions and persist with debounce.
        self.track_and_persist_window_size(ctx);

        // 7. Debounced config write (settings panel + window size).
        self.maybe_save_config();

        // 8. Layout persistence (debounced).
        if self.layout_dirty {
            self.maybe_persist_layout();
        }

        // 9. Do NOT call ctx.request_repaint() here.
        //
        // In the event-driven architecture, the egui event loop enters
        // idle (zero CPU) when there are no pending repaint requests.
        // The following events trigger repaint externally:
        //   - PTY data arrives → reader thread calls ctx.request_repaint()
        //     via the wakeup callback
        //   - Cursor blink → request_repaint_after() fires
        //   - User input (mouse, keyboard) → egui repaints automatically
        //   - Window resize → egui repaints automatically
        //   - Settings panel → egui repaints while it's open
        //   - Config/theme reload → explicit request_repaint() in handler
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // ── Config error toast (top banner) ──────────────────────────
        if let Some(msg) = self.error_toast.clone() {
            egui::Panel::top("config_error").resizable(false).show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(egui::Color32::RED, "⚠ Config error");
                    ui.label(msg);
                    if ui.button("×").clicked() {
                        self.error_toast = None;
                    }
                });
            });
        }

        // Clear the shared instance buffer and atlas ranges at the
        // start of every frame.  Each session appends its own instances
        // and ranges; the final `bump_instance_gen` is called once
        // after the dock finishes.
        self.gpu.clear_instances();
        self.gpu.clear_atlas_ranges();

        if self.config.ui.tabs_enabled {
            self.render_tabs_with_dock(ui);
        } else {
            // Legacy single-terminal path (no dock, no sidebar).
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show_inside(ui, |ui| {
                    render_legacy_single(ui, &mut self.sessions);
                });
        }

        // Push the concatenated instance buffer to the GPU side.
        self.gpu.bump_instance_gen();
    }

    fn on_exit(&mut self) {
        self.persist_layout_now();
        // Save any pending config changes (window size, settings, etc.)
        // immediately so the next session starts with the correct state.
        if self.config_dirty {
            if let Err(e) = self.config.save() {
                log::error!("failed to save config on exit: {e}");
            }
            self.config_dirty = false;
        }
    }
}
// ── Colour helpers ─────────────────────────────────────────────────────

/// Convert a [`Theme`] background colour to `egui::Color32`.
fn theme_bg_to_color32(theme: &Theme) -> egui::Color32 {
    let b = theme.background;
    egui::Color32::from_rgba_premultiplied(
        (b.r() * 255.0).round().clamp(0.0, 255.0) as u8,
        (b.g() * 255.0).round().clamp(0.0, 255.0) as u8,
        (b.b() * 255.0).round().clamp(0.0, 255.0) as u8,
        (b.a() * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

// Mark unused-imports hints.  These are kept to preserve the
// signature of the previous single-terminal code paths.
#[allow(dead_code)]
fn _unused_path(_p: PathBuf) {}
