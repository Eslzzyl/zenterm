//! Terminal buffer configuration parsed from the `[terminal]` section.
//!
//! # ⚠  Maintenance note
//!
//! If you modify any field or default value in this module, update
//! [`docs/usages/config.md`] to match.

use serde::{Deserialize, Serialize};

/// The `[terminal]` section of the config file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalConfig {
    /// Maximum number of lines retained in the main-screen scrollback.
    /// `0` disables scrollback history.
    #[serde(default = "default_scrollback_lines")]
    pub scrollback_lines: usize,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            scrollback_lines: default_scrollback_lines(),
        }
    }
}

fn default_scrollback_lines() -> usize {
    10_000
}
