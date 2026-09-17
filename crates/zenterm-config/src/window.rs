//! Window configuration parsed from the `[window]` section.
//!
//! # ⚠  Maintenance note
//!
//! If you modify any field, default value, or enum variant in this module,
//! update [`docs/usages/config.md`] to match.

use serde::{Deserialize, Serialize};

// ── WindowConfig ───────────────────────────────────────────────────────

/// The `[window]` section of the config file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

    /// Show window decorations (title bar + borders).
    #[serde(default = "default_decorations")]
    pub decorations: bool,

    /// macOS-only: treat the Option key as Alt.
    ///
    /// When `true`, Option+key behaves like Alt+key, sending `ESC` +
    /// the key's ASCII byte.  When `false` (the default), Option+key
    /// composes special Unicode characters per the macOS keyboard
    /// layout (e.g. Option+A → "å").
    ///
    /// Only `false` and `true` are supported because egui does not
    /// expose left/right Option side information.
    #[serde(default)]
    pub macos_option_as_alt: bool,

    /// Last known window inner size in logical points.
    ///
    /// Saved automatically when the terminal grid is resized, and
    /// restored on the next startup so the window opens at the exact
    /// same pixel dimensions.  `None` on first launch or after a
    /// config reset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_window_size: Option<[f32; 2]>,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            dimensions: WindowDimensions::default(),
            padding: WindowPadding::default(),
            title: default_title(),
            decorations: default_decorations(),
            macos_option_as_alt: false,
            last_window_size: None,
        }
    }
}

impl WindowConfig {
    /// Returns `true` if the restart-sensitive fields differ from `other`.
    ///
    /// Window title and decorations are baked into the
    /// `eframe::ViewportBuilder` passed at startup and cannot be
    /// changed at runtime.
    pub fn needs_restart_from(&self, other: &Self) -> bool {
        self.decorations != other.decorations || self.title != other.title
    }
}

fn default_title() -> String {
    "Zenterm".into()
}

fn default_decorations() -> bool {
    true
}

// ── Sub-types ──────────────────────────────────────────────────────────

/// Terminal size in cells.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
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
        Self { x: 12.0, y: 10.0 }
    }
}
