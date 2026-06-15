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

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Context, Id};
use egui_dock::{DockArea, Style};

use zenterm_config::Config;
use zenterm_core::SubpixelLayout;
use zenterm_core::theme::{Theme, ThemePreference, THEME_DARK};
use zenterm_render::callback::{CallbackHandle, SharedRenderState, TerminalWgpuCallback};
use zenterm_term::ColorScheme;

use crate::glyph_cache::SharedGlyphAtlas;
use crate::gpu::SharedGpuContext;
use crate::layout_io::{LayoutIo, PersistedLayout, PersistedWorkspace, SessionMeta, SCHEMA_VERSION};
use crate::legacy::render_legacy_single;
use crate::session::{SessionEffect, SessionId, TerminalSession};
use crate::tab_viewer::TabViewerContext;
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
    pub theme: Theme,
    pub theme_preference: ThemePreference,
    pub last_system_dark: bool,
    pub default_bg: egui::Color32,
    pub pixels_per_point: f32,
    pub error_toast: Option<String>,

    // ── Pending actions accumulated by the dock viewer ─────────────
    pending_close: Vec<SessionId>,
    pending_adds: u32,
}

// ── Construction ───────────────────────────────────────────────────────

impl ZentermApp {
    /// Create a new Zenterm application with the given wgpu resources.
    pub fn new_with_wgpu(
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
            config,
            theme,
            theme_preference: ThemePreference::Dark,
            last_system_dark: true,
            default_bg,
            pixels_per_point,
            error_toast: None,
            pending_close: Vec::new(),
            pending_adds: 0,
        }
    }


    // ── Theme sync (app-level) ─────────────────────────────────────

    /// Sync the active theme with the user's preference and the OS
    /// system theme.  Rebuilds each session's colour scheme when the
    /// theme changes.
    fn sync_theme(&mut self, egui_ctx: &Context) {
        let system_dark = egui_ctx.input(|i| match i.raw.system_theme {
            Some(egui::Theme::Dark) => true,
            Some(egui::Theme::Light) => false,
            None => true,
        });
        let new_theme = self.config.colors.to_theme(system_dark);
        let theme_changed = new_theme.background.r() != self.theme.background.r()
            || new_theme.background.g() != self.theme.background.g()
            || new_theme.background.b() != self.theme.background.b();
        if theme_changed || self.last_system_dark != system_dark {
            self.theme = new_theme.clone();
            self.last_system_dark = system_dark;
            self.default_bg = theme_bg_to_color32(&new_theme);
            let scheme = ColorScheme::from_theme(&new_theme);
            for (_, session) in self.sessions.iter_mut() {
                session.terminal.set_scheme(scheme.clone());
                session.default_bg = self.default_bg;
                session.terminal_dirty = true;
            }
        }
    }

    /// Generate a unique workspace name based on the current working
    /// directory.  Falls back to a numbered name if the cwd is
    /// unavailable or the name already exists.
    fn generate_workspace_name(workspaces: &WorkspaceManager) -> String {
        let cwd_name = std::env::current_dir()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));

        let base = cwd_name.unwrap_or_else(|| "workspace".into());

        // Ensure uniqueness by appending a suffix if needed.
        let existing: std::collections::HashSet<String> = workspaces
            .workspaces
            .iter()
            .map(|ws| ws.name.clone())
            .collect();
        if !existing.contains(&base) {
            return base;
        }
        for i in 2.. {
            let candidate = format!("{base}-{i}");
            if !existing.contains(&candidate) {
                return candidate;
            }
        }
        unreachable!()
    }

    // ── Session lifecycle ─────────────────────────────────────────

    /// Spawn a new session in the active workspace's currently focused
    /// dock leaf and return its id.
    pub fn spawn_session(&mut self) -> SessionId {
        let id = self.workspaces.new_session_id();
        let scheme = ColorScheme::from_theme(&self.theme);
        let size = zenterm_core::size::TermSize::new(
            self.config.window.dimensions.lines,
            self.config.window.dimensions.columns,
        );
        let session = TerminalSession::new(
            id,
            size,
            scheme,
            self.config.cursor.blink_interval,
            self.default_bg,
            self.gpu.clone(),
            self.atlas.clone(),
            self.callback.clone(),
        );
        self.sessions.insert(id, session);
        self.workspaces.active_workspace_mut().new_tab(id);
        self.active_session_id = Some(id);
        self.mark_layout_dirty();
        id
    }

    /// Close a session and remove its tab from whichever workspace
    /// owns it.
    pub fn close_session(&mut self, id: SessionId) {
        // Remove the tab from the workspace that owns it.
        self.workspaces.remove_tab_from_any_workspace(id);

        // Drop the session (its `Drop` kills the PTY).
        self.sessions.remove(&id);

        // Re-focus: pick the first tab in the active workspace.
        if self.active_session_id == Some(id) {
            self.active_session_id = self
                .workspaces
                .active_workspace()
                .all_tab_ids()
                .first()
                .copied();
        }
        self.mark_layout_dirty();
    }

    /// Switch the active tab to the given `(node, tab)` pair in the
    /// active workspace.
    pub fn focus_tab(&mut self, node: egui_dock::NodeIndex, tab: egui_dock::TabIndex) {
        let path = egui_dock::TabPath {
            surface: egui_dock::SurfaceIndex::main(),
            node,
            tab,
        };
        let ws = self.workspaces.active_workspace_mut();
        if ws.dock.set_active_tab(path).is_ok() {
            if let Some(tp) = ws.dock.iter_all_tabs().next() {
                self.active_session_id = Some(*tp.1);
            }
            ws.mark_changed();
        }
    }

    // ── Layout persistence ────────────────────────────────────────

    fn mark_layout_dirty(&mut self) {
        self.layout_dirty = true;
    }

    /// If the layout has been mutated longer than the configured
    /// debounce window ago, write it to disk.
    fn maybe_persist_layout(&mut self) {
        if !self.layout_dirty {
            return;
        }
        if !self.config.ui.persist_layout {
            self.layout_dirty = false;
            return;
        }
        let debounce = Duration::from_millis(self.config.ui.layout_debounce_ms);
        if let Some(at) = self.last_persist_at {
            if at.elapsed() < debounce {
                return;
            }
        }
        self.persist_layout_now();
    }

    /// Force a layout write.  Called from [`Self::on_exit`] and from
    /// [`Self::maybe_persist_layout`] after the debounce window.
    pub fn persist_layout_now(&mut self) {
        let persisted = PersistedLayout {
            version: SCHEMA_VERSION,
            active_workspace_id: self.workspaces.active_workspace_id.raw(),
            next_session_id: self.workspaces.next_session_id,
            next_workspace_id: self.workspaces.next_workspace_id,
            workspaces: self
                .workspaces
                .workspaces
                .iter()
                .map(|ws| PersistedWorkspace {
                    id: ws.id.raw(),
                    name: ws.name.clone(),
                    dock: ws.dock.clone(),
                })
                .collect(),
        };
        if let Err(e) = self.layout_io.save_layout(&persisted) {
            log::error!("persist_layout_now: save_layout failed: {e}");
        }
        let metas: Vec<SessionMeta> = self
            .sessions
            .iter()
            .map(|(id, s)| {
                let ws_id = self
                    .workspaces
                    .find_tab_workspace(*id)
                    .map(|ws| ws.id.raw());
                SessionMeta {
                    id: id.0,
                    title: s.title.clone(),
                    cwd: s.cwd.clone(),
                    shell: None,
                    workspace_id: ws_id,
                }
            })
            .collect();
        if let Err(e) = self.layout_io.save_sessions(&metas) {
            log::error!("persist_layout_now: save_sessions failed: {e}");
        }
        self.layout_dirty = false;
        self.last_persist_at = Some(Instant::now());
    }

    // ── Keyboard / input (per-active-session) ─────────────────────

    fn pump_pty_active_sessions(&mut self) {
        // Iterate a snapshot of ids to avoid borrow issues.
        let ids: Vec<SessionId> = self.sessions.keys().copied().collect();
        for id in ids {
            if let Some(session) = self.sessions.get_mut(&id) {
                session.pump_pty();
            }
        }
    }

    fn handle_side_effects(&mut self, ctx: &Context) {
        let ids: Vec<SessionId> = self.sessions.keys().copied().collect();
        let mut all_close = false;
        let mut window_title: Option<String> = None;
        for id in ids {
            let effects = if let Some(s) = self.sessions.get_mut(&id) {
                s.handle_side_effects(ctx)
            } else {
                Vec::new()
            };
            for effect in effects {
                match effect {
                    SessionEffect::WindowTitle(t) => {
                        window_title = Some(t);
                    }
                    SessionEffect::CloseWindow => {
                        all_close = true;
                    }
                }
            }
        }
        if let Some(t) = window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(t));
        }
        if all_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// Forward a single [`egui::Event`] to the active session's PTY.
    fn forward_event_to_active(&mut self, event: &egui::Event) {
        if let Some(id) = self.active_session_id {
            if let Some(session) = self.sessions.get_mut(&id) {
                if let Some(bytes) = zenterm_input::InputMapper::map(event) {
                    if let Err(e) = session.pty.write(&bytes) {
                        log::error!("PTY write error: {e}");
                    }
                }
            }
        }
    }

    fn feed_keyboard_to_active(&mut self, ctx: &Context) {
        ctx.input(|input| {
            for event in &input.events {
                if matches!(
                    event,
                    egui::Event::Key { pressed: true, .. }
                ) {
                    // Copy / paste / reload are handled at the app
                    // level in `handle_shortcuts`.  The remaining
                    // events are forwarded to the active session.
                }
            }
        });
        // Forward everything via InputMapper.
        ctx.input(|input| {
            for event in &input.events {
                self.forward_event_to_active(event);
            }
        });
    }

    /// Handle app-level keyboard shortcuts.
    ///
    /// Returns `true` if a shortcut was consumed (skip forwarding to
    /// the active session).
    fn handle_shortcuts(&mut self, ctx: &Context) -> bool {
        let (copy, paste, reload, ws_switch, ws_cycle) = ctx.input(|input| {
            let mut c = false;
            let mut p = false;
            let mut r = false;
            let mut ws_switch: Option<usize> = None;
            let mut ws_cycle: Option<isize> = None;
            for event in &input.events {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } = event
                {
                    // Ctrl+Shift+C / V / R
                    let shift_ctrl = modifiers.ctrl && modifiers.shift && !modifiers.alt;
                    if shift_ctrl {
                        match key {
                            egui::Key::C => c = true,
                            egui::Key::V => p = true,
                            egui::Key::R => r = true,
                            _ => {}
                        }
                    }
                    // Ctrl+1..9 → switch to workspace by index
                    if modifiers.ctrl && !modifiers.shift && !modifiers.alt {
                        match key {
                            egui::Key::Num1 => ws_switch = Some(0),
                            egui::Key::Num2 => ws_switch = Some(1),
                            egui::Key::Num3 => ws_switch = Some(2),
                            egui::Key::Num4 => ws_switch = Some(3),
                            egui::Key::Num5 => ws_switch = Some(4),
                            egui::Key::Num6 => ws_switch = Some(5),
                            egui::Key::Num7 => ws_switch = Some(6),
                            egui::Key::Num8 => ws_switch = Some(7),
                            egui::Key::Num9 => ws_switch = Some(8),
                            _ => {}
                        }
                    }
                    // Ctrl+Tab → next workspace, Ctrl+Shift+Tab → prev
                    if modifiers.ctrl && !modifiers.alt {
                        match key {
                            egui::Key::Tab if !modifiers.shift => ws_cycle = Some(1),
                            egui::Key::Tab if modifiers.shift => ws_cycle = Some(-1),
                            _ => {}
                        }
                    }
                }
            }
            (c, p, r, ws_switch, ws_cycle)
        });
        if reload {
            self.reload_config(ctx);
            return true;
        }
        // Workspace switching shortcuts.
        if let Some(idx) = ws_switch {
            if let Some(ws) = self.workspaces.workspaces.get(idx) {
                let ws_id = ws.id;
                self.workspaces.switch_to(ws_id);
                self.active_session_id = self
                    .workspaces
                    .active_workspace()
                    .all_tab_ids()
                    .first()
                    .copied();
                self.mark_layout_dirty();
                return true;
            }
        }
        if let Some(dir) = ws_cycle {
            let len = self.workspaces.workspaces.len();
            if len > 0 {
                let current_idx = self
                    .workspaces
                    .workspaces
                    .iter()
                    .position(|ws| ws.id == self.workspaces.active_workspace_id)
                    .unwrap_or(0);
                let new_idx = ((current_idx as isize + dir).rem_euclid(len as isize)) as usize;
                let ws_id = self.workspaces.workspaces[new_idx].id;
                self.workspaces.switch_to(ws_id);
                self.active_session_id = self
                    .workspaces
                    .active_workspace()
                    .all_tab_ids()
                    .first()
                    .copied();
                self.mark_layout_dirty();
                return true;
            }
        }
        if copy {
            if let Some(id) = self.active_session_id {
                if let Some(session) = self.sessions.get_mut(&id) {
                    if session.terminal.has_selection() {
                        if let Some(text) = session.terminal.selected_text() {
                            ctx.copy_text(text);
                            return true;
                        }
                    }
                }
            }
        }
        if paste {
            if let Ok(mut cb) = arboard::Clipboard::new() {
                if let Ok(text) = cb.get_text() {
                    if !text.is_empty() {
                        if let Some(id) = self.active_session_id {
                            if let Some(session) = self.sessions.get_mut(&id) {
                                if let Err(e) = session.pty.write(text.as_bytes()) {
                                    log::error!("PTY paste error: {e}");
                                }
                                return true;
                            }
                        }
                    }
                }
            }
        }
        false
    }

    // ── Config reload ─────────────────────────────────────────────

    fn reload_config(&mut self, egui_ctx: &Context) {
        match Config::reload() {
            Ok(Some(cfg)) => {
                log::info!("config reloaded, applying changes");
                let old_config = std::mem::replace(&mut self.config, cfg);

                // Re-resolve theme.
                let system_dark = egui_ctx.input(|i| match i.raw.system_theme {
                    Some(egui::Theme::Dark) => true,
                    Some(egui::Theme::Light) => false,
                    None => true,
                });
                self.theme = self.config.colors.to_theme(system_dark);
                self.last_system_dark = system_dark;
                self.default_bg = theme_bg_to_color32(&self.theme);
                let scheme = ColorScheme::from_theme(&self.theme);

                // Apply to every session.
                let font_size_changed =
                    (self.config.font.size - old_config.font.size).abs() > f32::EPSILON;
                for (_, session) in self.sessions.iter_mut() {
                    session.terminal.set_scheme(scheme.clone());
                    session.default_bg = self.default_bg;
                    session.terminal_dirty = true;
                    session.apply_config_change(self.config.font.size, self.config.cursor.blink_interval);
                }

                // Rebuild the glyph atlas if the logical font size changed.
                if font_size_changed {
                    let new_font_size = self.config.font.size * self.pixels_per_point;
                    let font_family =
                        std::borrow::Cow::Owned(self.config.font.normal.family.clone());
                    let (cw, ch) = self.atlas.reinit_for_dpi(
                        new_font_size,
                        font_family,
                        self.pixels_per_point,
                        SubpixelLayout::detect(),
                    );
                    self.atlas.seed_ascii();
                    self.atlas.sync_to_gpu();
                    for (_, session) in self.sessions.iter_mut() {
                        session.cell_width = cw;
                        session.cell_height = ch;
                    }
                }
                self.error_toast = None;
            }
            Ok(None) => {
                log::info!("config file removed, keeping current settings");
                self.error_toast = None;
            }
            Err(e) => {
                log::error!("config reload failed: {e}");
                self.error_toast = Some(format!("Config error — keeping old settings:\n{}", e));
            }
        }
    }
}

// ── eframe::App ────────────────────────────────────────────────────────

impl eframe::App for ZentermApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // 0. Theme sync.
        self.sync_theme(ctx);

        // 0.5. DPI change.
        let current_ppp = ctx.pixels_per_point();
        if (current_ppp - self.pixels_per_point).abs() > 0.01 {
            for (_, session) in self.sessions.iter_mut() {
                session.reinit_for_dpi(current_ppp);
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
        }

        // 2. Keyboard shortcuts (copy/paste/reload).
        if self.handle_shortcuts(ctx) {
            // skip forwarding (the shortcut consumed the event)
        } else {
            self.feed_keyboard_to_active(ctx);
        }

        // 3. Advance frame count and trigger cursor-blink rebuilds.
        for (_, session) in self.sessions.iter_mut() {
            session.frame_count = session.frame_count.wrapping_add(1);
            let blinking = session.terminal.cursor().style.blinking
                && !matches!(
                    session.terminal.cursor().style.shape,
                    alacritty_terminal::vte::ansi::CursorShape::Block
                );
            if blinking {
                session.terminal_dirty = true;
            }
        }

        // 4. Request continuous repaint.
        ctx.request_repaint();
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

        // Clear the shared instance buffer at the start of every
        // frame.  Each session appends its own instances; the final
        // `bump_instance_gen` is called once after the dock finishes.
        self.gpu.clear_instances();

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

        // Debounced layout persistence.
        self.maybe_persist_layout();
    }

    fn on_exit(&mut self) {
        self.persist_layout_now();
    }
}

// ── Dock rendering ─────────────────────────────────────────────────────

impl ZentermApp {
    fn render_tabs_with_dock(&mut self, ui: &mut egui::Ui) {
        // Clear pending queues collected during the previous frame.
        self.pending_close.clear();
        self.pending_adds = 0;

        // ── Optional sidebar ────────────────────────────────────────
        let show_sidebar = self.config.ui.sidebar_enabled;
        if show_sidebar {
            let pos = self.config.ui.sidebar_position;
            let width = self.config.ui.sidebar_width;
            let min_w = self.config.ui.sidebar_min_width;
            let max_w = self.config.ui.sidebar_max_width;
            let panel = match pos {
                zenterm_config::ui::SidebarPosition::Left => {
                    egui::SidePanel::left("zenterm_sidebar")
                }
                zenterm_config::ui::SidebarPosition::Right => {
                    egui::SidePanel::right("zenterm_sidebar")
                }
            };

            // Snapshot all workspaces and their tabs so the closure
            // doesn't need to borrow `self`.
            let ws_snapshot: Vec<(
                crate::workspace::WorkspaceId,
                String,
                bool,
                Vec<(egui_dock::NodeIndex, egui_dock::TabIndex, SessionId, String, Option<std::path::PathBuf>)>,
            )> = self
                .workspaces
                .workspaces
                .iter()
                .map(|ws| {
                    let tabs = ws
                        .dock
                        .iter_all_tabs()
                        .filter_map(|(path, tab)| {
                            let s = self.sessions.get(tab)?;
                            Some((path.node, path.tab, *tab, s.title.clone(), s.cwd.clone()))
                        })
                        .collect();
                    (
                        ws.id,
                        ws.name.clone(),
                        ws.id == self.workspaces.active_workspace_id,
                        tabs,
                    )
                })
                .collect();
            let active_session = self.active_session_id;

            panel
                .resizable(true)
                .default_width(width)
                .min_width(min_w)
                .max_width(max_w)
                .show_inside(ui, |ui| {
                    let mut queued_new_tab = false;
                    let mut queued_new_ws = false;
                    let mut queued_switch_ws: Option<crate::workspace::WorkspaceId> = None;
                    let mut queued_focus: Option<(egui_dock::NodeIndex, egui_dock::TabIndex)> = None;
                    let mut queued_rename_ws: Option<(crate::workspace::WorkspaceId, String)> =
                        None;
                    let mut queued_close_ws: Option<crate::workspace::WorkspaceId> = None;

                    ui.vertical(|ui| {
                        ui.add_space(6.0);
                        ui.heading("Workspaces");
                        ui.add_space(4.0);
                        ui.separator();
                        ui.horizontal(|ui| {
                            if ui.button("+  New shell").clicked() {
                                queued_new_tab = true;
                            }
                            if ui.button("+  New WS").clicked() {
                                queued_new_ws = true;
                            }
                        });
                        ui.add_space(2.0);
                        ui.separator();
                        egui::ScrollArea::vertical()
                            .auto_shrink([false; 2])
                            .show(ui, |ui| {
                                for (ws_id, ws_name, is_active_ws, tabs) in &ws_snapshot {
                                    // ── Workspace header ─────────
                                    let rename_id =
                                        egui::Id::new(("ws_rename", ws_id.0));
                                    let is_renaming = ui.memory(|m| {
                                        m.data
                                            .get_temp::<bool>(rename_id)
                                            .unwrap_or(false)
                                    });

                                    if is_renaming {
                                        // Inline rename mode.
                                        let mut buf = ws_name.clone();
                                        let resp = ui.text_edit_singleline(&mut buf);
                                        if resp.lost_focus()
                                            || ui.input(|i| i.key_pressed(egui::Key::Enter))
                                        {
                                            // Commit rename.
                                            ui.memory_mut(|m| {
                                                m.data.remove_temp::<bool>(rename_id);
                                            });
                                            if !buf.is_empty() && buf != *ws_name {
                                                queued_rename_ws =
                                                    Some((*ws_id, buf));
                                            }
                                        }
                                        // Also cancel on Escape.
                                        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                            ui.memory_mut(|m| {
                                                m.data.remove_temp::<bool>(rename_id);
                                            });
                                        }
                                        // Keep focus on the text edit.
                                        resp.request_focus();
                                    } else {
                                        // Normal display mode.
                                        let header_label = if *is_active_ws {
                                            egui::RichText::new(ws_name)
                                                .strong()
                                                .color(ui.visuals().strong_text_color())
                                        } else {
                                            egui::RichText::new(ws_name)
                                        };
                                        let header_resp =
                                            ui.selectable_label(*is_active_ws, header_label);
                                        if header_resp.clicked() {
                                            queued_switch_ws = Some(*ws_id);
                                        }
                                        // Double-click to rename.
                                        if header_resp.double_clicked() {
                                            ui.memory_mut(|m| {
                                                m.data.insert_temp::<bool>(
                                                    rename_id, true,
                                                );
                                            });
                                        }
                                        // Right-click context menu.
                                        header_resp.context_menu(|ui| {
                                            if ui.button("New Tab").clicked() {
                                                queued_new_tab = true;
                                                queued_switch_ws = Some(*ws_id);
                                                ui.close();
                                            }
                                            ui.separator();
                                            if ui.button("Rename...").clicked() {
                                                ui.memory_mut(|m| {
                                                    m.data.insert_temp::<bool>(
                                                        rename_id, true,
                                                    );
                                                });
                                                ui.close();
                                            }
                                            ui.separator();
                                            if ui.button("Close workspace").clicked() {
                                                queued_close_ws = Some(*ws_id);
                                                ui.close();
                                            }
                                        });
                                    }

                                    // ── Tabs under this workspace ─
                                    ui.indent(egui::Id::new(("ws_tabs", ws_id.0)), |ui| {
                                        if tabs.is_empty() {
                                            ui.weak("(no tabs)");
                                        }
                                        for (node, tab, id, title, cwd) in tabs {
                                            let is_active_tab = Some(*id) == active_session;
                                            let label = if is_active_tab {
                                                egui::RichText::new(title)
                                                    .strong()
                                                    .color(ui.visuals().strong_text_color())
                                            } else {
                                                egui::RichText::new(title)
                                            };
                                            let resp =
                                                ui.selectable_label(is_active_tab, label);
                                            if resp.clicked() {
                                                // Switch to the tab's workspace first, then
                                                // focus the tab.
                                                queued_switch_ws = Some(*ws_id);
                                                queued_focus = Some((*node, *tab));
                                            }
                                            if let Some(cwd) = cwd {
                                                ui.weak(cwd.display().to_string());
                                            }
                                        }
                                    });

                                    // Small gap between workspace sections.
                                    ui.add_space(4.0);
                                }
                            });
                    });

                    // ── Apply queued actions ──────────────────────
                    if queued_new_ws {
                        let ws_name = Self::generate_workspace_name(
                            &self.workspaces,
                        );
                        self.workspaces.create_workspace(ws_name);
                        // Also spawn a first tab in the new workspace.
                        let id = self.workspaces.new_session_id();
                        let scheme = ColorScheme::from_theme(&self.theme);
                        let size = zenterm_core::size::TermSize::new(
                            self.config.window.dimensions.lines,
                            self.config.window.dimensions.columns,
                        );
                        let session = TerminalSession::new(
                            id,
                            size,
                            scheme,
                            self.config.cursor.blink_interval,
                            self.default_bg,
                            self.gpu.clone(),
                            self.atlas.clone(),
                            self.callback.clone(),
                        );
                        self.sessions.insert(id, session);
                        self.workspaces.active_workspace_mut().new_tab(id);
                        self.active_session_id = Some(id);
                        self.mark_layout_dirty();
                    }
                    if let Some(ws_id) = queued_switch_ws {
                        self.workspaces.switch_to(ws_id);
                        self.mark_layout_dirty();
                    }
                    if queued_new_tab {
                        self.spawn_session();
                    }
                    if let Some((node, tab)) = queued_focus {
                        self.focus_tab(node, tab);
                    }
                    if let Some((ws_id, new_name)) = queued_rename_ws {
                        self.workspaces.rename_workspace(ws_id, new_name);
                        self.mark_layout_dirty();
                    }
                    if let Some(ws_id) = queued_close_ws {
                        // Collect sessions to close from the workspace.
                        let sessions_to_close: Vec<SessionId> = self
                            .workspaces
                            .find_workspace(ws_id)
                            .map(|ws| ws.all_tab_ids())
                            .unwrap_or_default();
                        self.workspaces.close_workspace(ws_id);
                        for id in sessions_to_close {
                            self.sessions.remove(&id);
                        }
                        // Re-focus on the now-active workspace.
                        self.active_session_id = self
                            .workspaces
                            .active_workspace()
                            .all_tab_ids()
                            .first()
                            .copied();
                        self.mark_layout_dirty();
                    }
                });
        }

        // ── Central dock area ──────────────────────────────────────
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show_inside(ui, |ui| {
                let mut viewer = TabViewerContext {
                    sessions: &mut self.sessions,
                    active_session_id: &mut self.active_session_id,
                    pending_close: &mut self.pending_close,
                    pending_adds: &mut self.pending_adds,
                };
                let style = Style::from_egui(ui.style().as_ref());
                let ws = self.workspaces.active_workspace_mut();
                let mut area = DockArea::new(&mut ws.dock)
                    .style(style)
                    .show_close_buttons(self.config.ui.show_close_tab_button)
                    .show_add_buttons(self.config.ui.show_add_tab_button);
                area = area.id(Id::new("zenterm_dock"));
                area.show_inside(ui, &mut viewer);
            });

        // ── Apply pending actions collected by the viewer ─────────
        let added = self.pending_adds;
        if added > 0 {
            for _ in 0..added {
                self.spawn_session();
            }
        }
        // Drain to a local first to avoid a borrow conflict with
        // `self.close_session` (which mutably borrows `self`).
        let to_close: Vec<SessionId> = std::mem::take(&mut self.pending_close);
        for id in to_close {
            self.close_session(id);
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
