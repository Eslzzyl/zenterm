//! Mouse configuration parsed from the `[mouse]` section.
//!
//! # ⚠  Maintenance note
//!
//! If you modify any field, default value, or enum variant in this module,
//! update [`docs/usages/config.md`] to match.

use serde::{Deserialize, Serialize};

/// The `[mouse]` section of the config file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MouseConfig {
    /// Hide the mouse cursor while the user is typing.
    #[serde(default)]
    pub hide_when_typing: bool,

    /// Whether to open URLs on Ctrl+Click.
    #[serde(default = "default_url_open")]
    pub url_open: bool,

    /// Whether to underline URLs on hover.
    #[serde(default = "default_url_hover_underline")]
    pub url_hover_underline: bool,
}

const fn default_url_open() -> bool {
    true
}

const fn default_url_hover_underline() -> bool {
    true
}

impl Default for MouseConfig {
    fn default() -> Self {
        Self {
            hide_when_typing: false,
            url_open: true,
            url_hover_underline: true,
        }
    }
}
