//! Mouse configuration parsed from the `[mouse]` section.
//!
//! # ⚠  Maintenance note
//!
//! If you modify any field, default value, or enum variant in this module,
//! update [`docs/usages/config.md`] to match.

use serde::{Deserialize, Serialize};

/// The `[mouse]` section of the config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MouseConfig {
    /// Hide the mouse cursor while the user is typing.
    #[serde(default)]
    pub hide_when_typing: bool,
}

impl Default for MouseConfig {
    fn default() -> Self {
        Self {
            hide_when_typing: false,
        }
    }
}
