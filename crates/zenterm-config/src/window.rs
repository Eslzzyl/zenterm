//! Window configuration parsed from the `[window]` section.
//!
//! # ⚠  Maintenance note
//!
//! If you modify any field, default value, or enum variant in this module,
//! update [`docs/usages/config.md`] to match.

use serde::{Deserialize, Serialize};

// ── WindowConfig ───────────────────────────────────────────────────────

/// The `[window]` section of the config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowConfig {
    /// Initial terminal size in cells (columns × lines).
    #[serde(default)]
    pub dimensions: WindowDimensions,

    /// Inner padding around the terminal grid (logical pixels).
    #[serde(default)]
    pub padding: WindowPadding,

    /// Window title.
    #[serde(default = "default_title")]
    pub title: String,

    /// Background opacity (0.0 = fully transparent, 1.0 = fully opaque).
    #[serde(default = "default_opacity")]
    pub opacity: f32,

    /// macOS-only: request background blur behind the window.
    #[serde(default)]
    pub blur: bool,

    /// Show window decorations (title bar + borders).
    #[serde(default = "default_decorations")]
    pub decorations: bool,

    /// Initial window state.
    #[serde(default)]
    pub startup_mode: StartupMode,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            dimensions: WindowDimensions::default(),
            padding: WindowPadding::default(),
            title: default_title(),
            opacity: default_opacity(),
            blur: false,
            decorations: default_decorations(),
            startup_mode: StartupMode::default(),
        }
    }
}

fn default_title() -> String {
    "Zenterm".into()
}

fn default_opacity() -> f32 {
    1.0
}

fn default_decorations() -> bool {
    true
}

// ── Sub-types ──────────────────────────────────────────────────────────

/// Terminal size in cells.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WindowDimensions {
    /// Number of columns (cells).
    #[serde(default = "default_columns")]
    pub columns: u16,
    /// Number of visible lines (rows).
    #[serde(default = "default_lines")]
    pub lines: u16,
}

impl Default for WindowDimensions {
    fn default() -> Self {
        Self {
            columns: default_columns(),
            lines: default_lines(),
        }
    }
}

fn default_columns() -> u16 {
    80
}
fn default_lines() -> u16 {
    24
}

/// Padding between the window edge and the terminal grid.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WindowPadding {
    /// Horizontal padding (logical pixels).
    #[serde(default)]
    pub x: f32,
    /// Vertical padding (logical pixels).
    #[serde(default)]
    pub y: f32,
}

impl Default for WindowPadding {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0 }
    }
}

/// Initial window state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum StartupMode {
    #[default]
    #[serde(rename = "Windowed")]
    Windowed,
    #[serde(rename = "Maximized")]
    Maximized,
    #[serde(rename = "Fullscreen")]
    Fullscreen,
}
