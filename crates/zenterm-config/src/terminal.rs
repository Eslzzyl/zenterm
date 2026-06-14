//! Terminal behaviour configuration parsed from the `[terminal]` section.
//!
//! # ⚠  Maintenance note
//!
//! If you modify any field, default value, or enum variant in this module,
//! update [`docs/usages/config.md`] to match.

use serde::{Deserialize, Serialize};

/// The `[terminal]` section of the config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalConfig {
    /// OSC 52 clipboard escape sequence support.
    #[serde(default)]
    pub osc52: Osc52Mode,

    /// Shell to spawn.  When absent the system login shell is used.
    pub shell: Option<ShellConfig>,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            osc52: Osc52Mode::default(),
            shell: None,
        }
    }
}

/// OSC 52 clipboard permission level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Osc52Mode {
    /// Disallow all OSC 52 clipboard access.
    #[serde(rename = "Disabled")]
    Disabled,
    /// Only allow pasting from the system clipboard into the terminal.
    #[serde(rename = "OnlyPaste")]
    OnlyPaste,
    /// Only allow copying from the terminal to the system clipboard.
    #[serde(rename = "OnlyCopy")]
    OnlyCopy,
    /// Allow full clipboard read/write via OSC 52.
    #[default]
    #[serde(rename = "CopyPaste")]
    CopyPaste,
}

/// A program to launch as the shell.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellConfig {
    /// Path to the executable.
    pub program: String,
    /// Command-line arguments.
    #[serde(default)]
    pub args: Vec<String>,
}
