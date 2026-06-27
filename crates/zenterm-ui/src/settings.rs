//! Settings panel — editing all [`Config`](zenterm_config::Config) fields
//! in a native OS window.
//!
//! # Behaviour
//!
//! - Every field change is **applied immediately** to the running program.
//! - Settings that require a restart are marked inline with a hint.
//! - "Reset All" in the nav sidebar resets to defaults, applies, and saves.
//! - No explicit Apply/Save buttons — direct manipulation.

use zenterm_config::colors::{AnsiColors, ColorsConfig, CursorColors, PrimaryColors, SelectionColors, ThemePreference};
use zenterm_config::cursor::{Blinking, CursorConfig, CursorShape};
use zenterm_config::font::{FontConfig, FontDescription};
use zenterm_config::keyboard::KeyboardConfig;
use zenterm_config::mouse::MouseConfig;
use zenterm_config::selection::SelectionConfig;
use zenterm_config::terminal::{Osc52Mode, ShellConfig, TerminalConfig};
use zenterm_config::ui::{SidebarPosition, UiConfig};
use zenterm_config::window::{StartupMode, WindowConfig};
use zenterm_config::Config;

use crate::settings_widgets;

// ── Section selector ─────────────────────────────────────────────────────

/// Top-level config sections that the user can navigate to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSection {
    Window,
    Font,
    Colors,
    Cursor,
    Selection,
    Mouse,
    Terminal,
    Keyboard,
    Ui,
}

impl SettingsSection {
    /// All sections, in display order.
    pub const ALL: &'static [Self] = &[
        Self::Window,
        Self::Font,
        Self::Colors,
        Self::Cursor,
        Self::Selection,
        Self::Mouse,
        Self::Terminal,
        Self::Keyboard,
        Self::Ui,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Window => "Window",
            Self::Font => "Font",
            Self::Colors => "Colors",
            Self::Cursor => "Cursor",
            Self::Selection => "Selection",
            Self::Mouse => "Mouse",
            Self::Terminal => "Terminal",
            Self::Keyboard => "Keyboard",
            Self::Ui => "UI",
        }
    }
}

// ── Render result ───────────────────────────────────────────────────────

/// What happened inside the settings panel this frame.
#[derive(Debug, Default)]
pub struct SettingsOutput {
    /// `true` when the user confirmed "Reset All to defaults".
    /// The caller should reset config, apply it, and save to disk.
    pub reset_all_confirmed: bool,
}

// ── SettingsState ────────────────────────────────────────────────────────

/// Mutable state for the settings panel.
pub struct SettingsState {
    /// Whether the panel is currently visible.
    pub open: bool,
    /// The config being edited in the panel.
    pub working_config: Config,
    /// Which section is selected in the navigation sidebar.
    pub selected_section: SettingsSection,
    /// When `true`, a confirmation dialog for "Reset All" is shown.
    pub pending_reset_confirm: bool,
}

impl SettingsState {
    /// Create a new settings state from the app's current config.
    pub fn new(config: &Config) -> Self {
        Self {
            open: false,
            working_config: config.clone(),
            selected_section: SettingsSection::Window,
            pending_reset_confirm: false,
        }
    }

    /// Reset the working config to match a fresh config.
    pub fn reset_to(&mut self, config: &Config) {
        self.working_config = config.clone();
    }

    /// Returns `true` if the working config differs from `other`.
    pub fn is_dirty(&self, other: &Config) -> bool {
        self.working_config != *other
    }
}

// ── Render entry point (native viewport mode) ─────────────────────────

/// Render the settings panel inside a **native OS window**.
///
/// Returns [`SettingsOutput`] describing actions the caller should take.
pub fn render_settings_viewport(
    ctx: &egui::Context,
    state: &mut SettingsState,
    current_config: &Config,
) -> SettingsOutput {
    let mut output = SettingsOutput::default();
    #[allow(deprecated)]
    egui::CentralPanel::default().show(ctx, |ui| {
        render_settings_content(ui, state, current_config, &mut output);
    });
    output
}

// ── Shared content rendering ──────────────────────────────────────────

/// Draw the settings form inside an existing [`egui::Ui`].
fn render_settings_content(
    ui: &mut egui::Ui,
    state: &mut SettingsState,
    current_config: &Config,
    output: &mut SettingsOutput,
) {
    ui.horizontal_top(|ui| {
        // ── Left: navigation sidebar ───────────────────────────
        egui::Panel::left("settings_nav")
            .resizable(false)
            .default_size(160.0)
            .min_size(120.0)
            .show_inside(ui, |ui| {
                ui.vertical(|ui| {
                    ui.add_space(4.0);

                    for sec in SettingsSection::ALL {
                        let selected = *sec == state.selected_section;
                        let label = sec.label();
                        if ui
                            .selectable_label(selected, label)
                            .clicked()
                        {
                            state.selected_section = *sec;
                        }
                    }

                    // ── Push "Reset All" to the bottom ────────
                    ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                        ui.add_space(8.0);
                        if ui
                            .button(egui::RichText::new("↺ Reset All").color(egui::Color32::LIGHT_RED))
                            .clicked()
                        {
                            state.pending_reset_confirm = true;
                        }
                    });
                });
            });

        // ── Right: content area (scrollable) ───────────────────
        egui::CentralPanel::default()
            .show_inside(ui, |ui| {
                // Show a restart-required banner at top of content
                // if any changed field needs a restart.
                let changes = current_config.diff_to(&state.working_config);
                if changes.needs_restart {
                    ui.label(
                        egui::RichText::new("⚠ Some changes require an application restart to take full effect")
                            .color(egui::Color32::YELLOW),
                    );
                    ui.add_space(4.0);
                }

                egui::ScrollArea::vertical().show(ui, |ui| {
                    render_section(
                        ui,
                        state.selected_section,
                        &mut state.working_config,
                    );
                });
            });
    });

    // ── Reset All confirmation dialog ──────────────────────────
    if state.pending_reset_confirm {
        egui::Window::new("Reset All Settings")
            .id(egui::Id::new("reset_all_confirm"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                ui.label("Reset all settings to their default values?");
                ui.label("This will save and apply immediately.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        state.pending_reset_confirm = false;
                    }
                    if ui.button("Reset && Save").clicked() {
                        state.pending_reset_confirm = false;
                        output.reset_all_confirmed = true;
                    }
                });
            });
    }
}

// ── Per-section render dispatcher ────────────────────────────────────────

fn render_section(ui: &mut egui::Ui, section: SettingsSection, cfg: &mut Config) {
    match section {
        SettingsSection::Window => render_window_section(ui, &mut cfg.window),
        SettingsSection::Font => render_font_section(ui, &mut cfg.font),
        SettingsSection::Colors => render_colors_section(ui, &mut cfg.colors),
        SettingsSection::Cursor => render_cursor_section(ui, &mut cfg.cursor),
        SettingsSection::Selection => render_selection_section(ui, &mut cfg.selection),
        SettingsSection::Mouse => render_mouse_section(ui, &mut cfg.mouse),
        SettingsSection::Terminal => render_terminal_section(ui, &mut cfg.terminal),
        SettingsSection::Keyboard => render_keyboard_section(ui, &mut cfg.keyboard),
        SettingsSection::Ui => render_ui_section(ui, &mut cfg.ui),
    }
}

// ── Window section ───────────────────────────────────────────────────────

fn render_window_section(ui: &mut egui::Ui, w: &mut WindowConfig) {
    settings_widgets::section_header(ui, "Window", "Window appearance, size, and initial state.");
    settings_widgets::drag_u16(ui, "Columns", &mut w.dimensions.columns, 1.0,
        "Number of character columns (requires restart)");
    settings_widgets::drag_u16(ui, "Lines", &mut w.dimensions.lines, 1.0,
        "Number of visible rows (requires restart)");
    settings_widgets::drag_f32(ui, "Padding X", &mut w.padding.x, 0.5,
        "Horizontal padding in logical pixels");
    settings_widgets::drag_f32(ui, "Padding Y", &mut w.padding.y, 0.5,
        "Vertical padding in logical pixels");
    settings_widgets::text_setting(ui, "Title", &mut w.title, "Window title");
    settings_widgets::slider_setting(ui, "Opacity", &mut w.opacity, 0.0..=1.0,
        "Background opacity (0=transparent, 1=opaque)");
    settings_widgets::bool_setting(ui, "Blur", &mut w.blur,
        "macOS only: request background blur");
    settings_widgets::bool_setting(ui, "Decorations", &mut w.decorations,
        "Show window title bar and borders (requires restart)");
    settings_widgets::combo_setting(ui, "Startup Mode", &mut w.startup_mode, &[
        (StartupMode::Windowed, "Windowed"),
        (StartupMode::Maximized, "Maximized"),
        (StartupMode::Fullscreen, "Fullscreen"),
    ], "Initial window state (requires restart)");
}

// ── Font section ─────────────────────────────────────────────────────────

fn render_font_section(ui: &mut egui::Ui, f: &mut FontConfig) {
    settings_widgets::section_header(ui, "Font", "Terminal typeface and spacing.");
    settings_widgets::drag_f32(ui, "Size", &mut f.size, 0.5,
        "Font size in logical pixels at 1× DPI");
    render_font_description(ui, "Normal", &mut f.normal);
    render_opt_font_description(ui, "Bold", &mut f.bold);
    render_opt_font_description(ui, "Italic", &mut f.italic);
    render_opt_font_description(ui, "Bold Italic", &mut f.bold_italic);

    ui.add_space(8.0);
    settings_widgets::section_header(ui, "Spacing", "");
    settings_widgets::drag_f32(ui, "Offset X", &mut f.offset.x, 0.25,
        "Extra horizontal spacing per character");
    settings_widgets::drag_f32(ui, "Offset Y", &mut f.offset.y, 0.25,
        "Extra vertical spacing per character");
    settings_widgets::drag_f32(ui, "Glyph Offset X", &mut f.glyph_offset.x, 0.25,
        "Per-glyph horizontal offset");
    settings_widgets::drag_f32(ui, "Glyph Offset Y", &mut f.glyph_offset.y, 0.25,
        "Per-glyph vertical offset");

    ui.add_space(8.0);
    settings_widgets::section_header(ui, "Features", "");
    settings_widgets::bool_setting(ui, "Built-in Box Drawing", &mut f.builtin_box_drawing,
        "Use software-rendered box-drawing / block characters");
    settings_widgets::bool_setting(ui, "Ligatures", &mut f.ligatures,
        "Enable OpenType ligatures (requires font support)");
}

fn render_font_description(ui: &mut egui::Ui, label: &str, fd: &mut FontDescription) {
    settings_widgets::text_setting(ui, &format!("{label} Family"), &mut fd.family,
        "Font family name (e.g. \"JetBrains Mono\")");
}

fn render_opt_font_description(ui: &mut egui::Ui, label: &str, opt: &mut Option<FontDescription>) {
    let mut enabled = opt.is_some();
    settings_widgets::bool_setting(ui, &format!("{label} – Enable"), &mut enabled,
        &format!("Override the {label} font face"));
    if enabled {
        let fd = opt.get_or_insert_with(|| FontDescription {
            family: "Menlo".into(),
            style: None,
        });
        render_font_description(ui, label, fd);
    } else {
        *opt = None;
    }
}

// ── Colors section ───────────────────────────────────────────────────────

fn render_colors_section(ui: &mut egui::Ui, c: &mut ColorsConfig) {
    settings_widgets::section_header(ui, "Theme", "Built-in colour scheme.");
    settings_widgets::combo_setting(ui, "Theme Preference", &mut c.theme, &[
        (ThemePreference::System, "System"),
        (ThemePreference::Dark, "Dark"),
        (ThemePreference::Light, "Light"),
    ], "Automatically follow system preference, or force Dark/Light");

    ui.add_space(8.0);
    settings_widgets::section_header(ui, "Primary", "Default text and background.");
    render_primary_colors(ui, &mut c.primary);

    ui.add_space(8.0);
    settings_widgets::section_header(ui, "Cursor", "");
    render_cursor_colors(ui, &mut c.cursor);

    ui.add_space(8.0);
    settings_widgets::section_header(ui, "Selection", "");
    render_selection_colors(ui, &mut c.selection);

    ui.add_space(8.0);
    settings_widgets::section_header(ui, "Normal ANSI", "The 8 dark ANSI colours.");
    render_ansi_colors(ui, &mut c.normal);

    ui.add_space(8.0);
    settings_widgets::section_header(ui, "Bright ANSI", "The 8 bright ANSI colours.");
    render_ansi_colors(ui, &mut c.bright);

    ui.add_space(8.0);
    settings_widgets::section_header(ui, "Dim ANSI", "Optional dim variant (auto-calculated when absent).");
    render_opt_ansi_colors(ui, &mut c.dim);
}

fn render_primary_colors(ui: &mut egui::Ui, p: &mut PrimaryColors) {
    settings_widgets::color_hex_setting(ui, "Foreground", &mut p.foreground, "Default text colour");
    settings_widgets::color_hex_setting(ui, "Background", &mut p.background, "Default background colour");
    settings_widgets::color_hex_setting(ui, "Dim Foreground", &mut p.dim_foreground, "Half-intensity text colour");
    settings_widgets::color_hex_setting(ui, "Bright Foreground", &mut p.bright_foreground,
        "Text colour when bold + bright-colours is enabled");
}

fn render_cursor_colors(ui: &mut egui::Ui, c: &mut CursorColors) {
    settings_widgets::color_hex_setting(ui, "Text", &mut c.text, "Colour for text under the cursor");
    settings_widgets::color_hex_setting(ui, "Cursor", &mut c.cursor, "Colour for the cursor cell");
}

fn render_selection_colors(ui: &mut egui::Ui, s: &mut SelectionColors) {
    settings_widgets::color_hex_setting(ui, "Foreground", &mut s.foreground, "Selected text colour");
    settings_widgets::color_hex_setting(ui, "Background", &mut s.background, "Selection background colour");
}

fn render_ansi_colors(ui: &mut egui::Ui, a: &mut AnsiColors) {
    settings_widgets::color_hex_setting(ui, "Black", &mut a.black, "");
    settings_widgets::color_hex_setting(ui, "Red", &mut a.red, "");
    settings_widgets::color_hex_setting(ui, "Green", &mut a.green, "");
    settings_widgets::color_hex_setting(ui, "Yellow", &mut a.yellow, "");
    settings_widgets::color_hex_setting(ui, "Blue", &mut a.blue, "");
    settings_widgets::color_hex_setting(ui, "Magenta", &mut a.magenta, "");
    settings_widgets::color_hex_setting(ui, "Cyan", &mut a.cyan, "");
    settings_widgets::color_hex_setting(ui, "White", &mut a.white, "");
}

fn render_opt_ansi_colors(ui: &mut egui::Ui, opt: &mut Option<AnsiColors>) {
    let mut enabled = opt.is_some();
    settings_widgets::bool_setting(ui, "Override Dim ANSI", &mut enabled, "");
    if enabled {
        let a = opt.get_or_insert_with(AnsiColors::default);
        render_ansi_colors(ui, a);
    } else {
        *opt = None;
    }
}

// ── Cursor section ───────────────────────────────────────────────────────

fn render_cursor_section(ui: &mut egui::Ui, c: &mut CursorConfig) {
    settings_widgets::section_header(ui, "Cursor", "Cursor appearance and behaviour.");
    settings_widgets::combo_setting(ui, "Shape", &mut c.style.shape, &[
        (CursorShape::Block, "Block"),
        (CursorShape::Beam, "Beam"),
        (CursorShape::Underline, "Underline"),
    ], "Visual cursor shape");
    settings_widgets::combo_setting(ui, "Blinking", &mut c.style.blinking, &[
        (Blinking::Off, "Off"),
        (Blinking::On, "On"),
        (Blinking::Terminal, "Terminal"),
    ], "Cursor blinking mode");

    settings_widgets::bool_setting(ui, "Unfocused Hollow", &mut c.unfocused_hollow,
        "Show a hollow cursor when the window loses focus");
    settings_widgets::slider_setting(ui, "Thickness", &mut c.thickness, 0.0..=1.0,
        "Thickness of Beam/Underline cursor (fraction of cell)");
    settings_widgets::drag_u64(ui, "Blink Interval", &mut c.blink_interval, 1.0,
        "Frames between blinks at 60 FPS (30 ≈ 500 ms)");
    settings_widgets::drag_u64(ui, "Blink Timeout", &mut c.blink_timeout, 1.0,
        "Seconds before blinking stops (0 = blink forever)");
}

// ── Selection section ────────────────────────────────────────────────────

fn render_selection_section(ui: &mut egui::Ui, s: &mut SelectionConfig) {
    settings_widgets::section_header(ui, "Selection", "Text selection behaviour.");
    settings_widgets::bool_setting(ui, "Save to Clipboard", &mut s.save_to_clipboard,
        "Automatically copy selected text to the system clipboard");
}

// ── Mouse section ────────────────────────────────────────────────────────

fn render_mouse_section(ui: &mut egui::Ui, m: &mut MouseConfig) {
    settings_widgets::section_header(ui, "Mouse", "Mouse interaction settings.");
    settings_widgets::bool_setting(ui, "Hide When Typing", &mut m.hide_when_typing,
        "Hide the mouse cursor while the user is typing");
}

// ── Terminal section ─────────────────────────────────────────────────────

fn render_terminal_section(ui: &mut egui::Ui, t: &mut TerminalConfig) {
    settings_widgets::section_header(ui, "Terminal", "Terminal emulation behaviour.");
    settings_widgets::combo_setting(ui, "OSC 52 Mode", &mut t.osc52, &[
        (Osc52Mode::Disabled, "Disabled"),
        (Osc52Mode::OnlyPaste, "Only Paste"),
        (Osc52Mode::OnlyCopy, "Only Copy"),
        (Osc52Mode::CopyPaste, "Copy && Paste"),
    ], "Clipboard access via OSC 52 escape sequences");

    ui.add_space(8.0);
    settings_widgets::section_header(ui, "Shell", "Override the default login shell.");
    let mut shell_enabled = t.shell.is_some();
    settings_widgets::bool_setting(ui, "Override Shell", &mut shell_enabled, "");
    if shell_enabled {
        let shell = t.shell.get_or_insert_with(|| ShellConfig {
            program: "bash".into(),
            args: Vec::new(),
        });
        settings_widgets::text_setting(ui, "Program", &mut shell.program,
            "Path to the shell executable");
        // Render args as a comma-separated string.
        let mut args_str = shell.args.join(", ");
        settings_widgets::text_setting(ui, "Arguments", &mut args_str,
            "Comma-separated command-line arguments");
        shell.args = args_str.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    } else {
        t.shell = None;
    }
}

// ── Keyboard section ─────────────────────────────────────────────────────

fn render_keyboard_section(ui: &mut egui::Ui, _k: &mut KeyboardConfig) {
    settings_widgets::section_header(ui, "Keyboard", "Custom key bindings.");
    ui.label("Custom key bindings are not yet implemented.");
    ui.label("This section is reserved for future use.");
}

// ── UI section ───────────────────────────────────────────────────────────

fn render_ui_section(ui: &mut egui::Ui, u: &mut UiConfig) {
    settings_widgets::section_header(ui, "Tabs", "Multi-terminal tab bar.");
    settings_widgets::bool_setting(ui, "Enable Tabs", &mut u.tabs_enabled,
        "Show tab bar for multiple terminal sessions");
    settings_widgets::bool_setting(ui, "Show Add Tab Button", &mut u.show_add_tab_button, "");
    settings_widgets::bool_setting(ui, "Show Close Tab Button", &mut u.show_close_tab_button, "");
    settings_widgets::bool_setting(ui, "Close on Middle Click", &mut u.tab_close_on_middle_click, "");

    ui.add_space(8.0);
    settings_widgets::section_header(ui, "Sidebar", "cmux-style workspace sidebar.");
    settings_widgets::bool_setting(ui, "Enable Sidebar", &mut u.sidebar_enabled,
        "Show the workspace sidebar (requires tabs)");
    settings_widgets::combo_setting(ui, "Sidebar Position", &mut u.sidebar_position, &[
        (SidebarPosition::Left, "Left"),
        (SidebarPosition::Right, "Right"),
    ], "");
    settings_widgets::slider_setting(ui, "Width", &mut u.sidebar_width, 100.0..=800.0,
        "Default sidebar width in logical pixels");
    settings_widgets::slider_setting(ui, "Min Width", &mut u.sidebar_min_width, 80.0..=600.0,
        "Minimum sidebar width");
    settings_widgets::slider_setting(ui, "Max Width", &mut u.sidebar_max_width, 200.0..=1200.0,
        "Maximum sidebar width");

    ui.add_space(8.0);
    settings_widgets::section_header(ui, "Layout Persistence", "");
    settings_widgets::bool_setting(ui, "Restore Layout on Startup", &mut u.restore_layout_on_startup,
        "Re-open previous tabs and workspaces on start");
    settings_widgets::bool_setting(ui, "Persist Layout", &mut u.persist_layout,
        "Save tab layout changes to disk automatically");
    settings_widgets::drag_u64(ui, "Debounce (ms)", &mut u.layout_debounce_ms, 10.0,
        "Milliseconds to wait before writing layout changes");
}
