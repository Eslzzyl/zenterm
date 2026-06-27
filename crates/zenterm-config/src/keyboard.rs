//! Keyboard key binding configuration.
//!
//! This module defines the `[keyboard]` config section.
//! Custom key bindings are not yet implemented; this is a placeholder
//! for the data model so the TOML structure is forward-compatible.

use serde::{Deserialize, Serialize};

/// The `[keyboard]` section of the config file.
///
/// Currently a placeholder — no bindings are parsed.
/// The `keyboard` section is recognised so that a future version can
/// add bindings without breaking existing configs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyboardConfig {
    /// Custom key bindings (not yet implemented).
    #[serde(default)]
    pub bindings: Vec<KeyBinding>,
}

impl Default for KeyboardConfig {
    fn default() -> Self {
        Self {
            bindings: Vec::new(),
        }
    }
}

/// A single key binding (reserved for future use).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyBinding {
    /// Key identifier (e.g. `"V"`, `"F1"`, `"Return"`).
    pub key: String,
    /// Modifier keys separated by `|` (e.g. `"Control|Shift"`).
    #[serde(default)]
    pub mods: String,
    /// Action to perform (e.g. `"Paste"`, `"Copy"`).
    pub action: String,
}
