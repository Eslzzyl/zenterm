//! Selection configuration parsed from the `[selection]` section.
//!
//! # ⚠  Maintenance note
//!
//! If you modify any field, default value, or enum variant in this module,
//! update [`docs/usages/config.md`] to match.

use serde::{Deserialize, Serialize};

/// The `[selection]` section of the config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionConfig {
    /// Automatically copy selected text to the system clipboard.
    #[serde(default)]
    pub save_to_clipboard: bool,
}

impl Default for SelectionConfig {
    fn default() -> Self {
        Self {
            save_to_clipboard: false,
        }
    }
}
