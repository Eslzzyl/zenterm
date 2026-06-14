//! Cursor configuration parsed from the `[cursor]` section.
//!
//! # ⚠  Maintenance note
//!
//! If you modify any field, default value, or enum variant in this module,
//! update [`docs/usages/config.md`] to match.

use serde::{Deserialize, Serialize};

// ── CursorConfig ─────────────────────────────────────��─────────────────

/// The `[cursor]` section of the config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorConfig {
    /// Cursor shape and blinking behaviour.
    #[serde(default)]
    pub style: CursorStyle,

    /// Show a hollow cursor when the window is unfocused.
    #[serde(default = "default_unfocused_hollow")]
    pub unfocused_hollow: bool,

    /// Thickness of the Underline / Beam cursor as a fraction of cell
    /// height (0.0–1.0).
    #[serde(default = "default_thickness")]
    pub thickness: f32,

    /// Blink interval in **frames** (at 60 FPS; 30 frames ≈ 500 ms).
    #[serde(default = "default_blink_interval")]
    pub blink_interval: u64,

    /// Timeout in seconds before blinking stops (0 = blink forever).
    #[serde(default = "default_blink_timeout")]
    pub blink_timeout: u64,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            style: CursorStyle::default(),
            unfocused_hollow: default_unfocused_hollow(),
            thickness: default_thickness(),
            blink_interval: default_blink_interval(),
            blink_timeout: default_blink_timeout(),
        }
    }
}

fn default_unfocused_hollow() -> bool {
    true
}
fn default_thickness() -> f32 {
    0.15
}
fn default_blink_interval() -> u64 {
    30
}
fn default_blink_timeout() -> u64 {
    5
}

// ── CursorStyle ────────────────────────────────────────────────────────

/// Cursor appearance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorStyle {
    /// Visual shape.
    #[serde(default)]
    pub shape: CursorShape,
    /// Blinking mode.
    #[serde(default)]
    pub blinking: Blinking,
}

impl Default for CursorStyle {
    fn default() -> Self {
        Self {
            shape: CursorShape::default(),
            blinking: Blinking::default(),
        }
    }
}

/// Cursor visual shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CursorShape {
    /// Solid rectangular block.
    #[default]
    #[serde(rename = "Block")]
    Block,
    /// Vertical bar at the left side of the cell.
    #[serde(rename = "Beam")]
    Beam,
    /// Horizontal bar at the bottom of the cell.
    #[serde(rename = "Underline")]
    Underline,
}

/// Cursor blinking mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Blinking {
    /// Never blink.
    #[default]
    #[serde(rename = "Off")]
    Off,
    /// Always blink (ignoring the terminal's own blink control).
    #[serde(rename = "On")]
    On,
    /// Follow the terminal's cursor-blinking escape sequence.
    #[serde(rename = "Terminal")]
    Terminal,
}
