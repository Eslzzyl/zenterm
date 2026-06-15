//! Top-level configuration loading, saving, and error handling.
//!
//! # ⚠  Maintenance note
//!
//! If you add/remove a top-level section from [`Config`], update
//! [`docs/usages/config.md`] to match.

use std::path::PathBuf;
use std::{env, fs, io};

use serde::{Deserialize, Serialize};

use crate::colors::ColorsConfig;
use crate::cursor::CursorConfig;
use crate::font::FontConfig;
use crate::keyboard::KeyboardConfig;
use crate::mouse::MouseConfig;
use crate::selection::SelectionConfig;
use crate::terminal::TerminalConfig;
use crate::ui::UiConfig;
use crate::window::WindowConfig;

// ── Config ─────────────────────────────────────────────────────────────

/// Complete Zenterm configuration, deserialised from TOML.
///
/// Every section is optional in TOML — missing sections fall back to
/// [`Default`] values that mirror the original hardcoded behaviour.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub window: WindowConfig,

    #[serde(default)]
    pub font: FontConfig,

    #[serde(default)]
    pub colors: ColorsConfig,

    #[serde(default)]
    pub cursor: CursorConfig,

    #[serde(default)]
    pub selection: SelectionConfig,

    #[serde(default)]
    pub mouse: MouseConfig,

    #[serde(default)]
    pub terminal: TerminalConfig,

    #[serde(default)]
    pub keyboard: KeyboardConfig,

    /// UI chrome (tabs + sidebar).  Defaults to all-off.
    #[serde(default)]
    pub ui: UiConfig,
}

impl Config {
    // ── Path ──────────────────────────────────────────────────────────

    /// Return the path to the config file.
    ///
    /// # Design note — intentionally hardcoded
    ///
    /// We use a hardcoded `~/.config/zenterm/config.toml` on **all**
    /// platforms (including Windows) instead of relying on
    /// XDG Base Directory or a `directories` crate.
    ///
    /// Rationale:
    /// 1. **Zero extra dependencies** — no need for `directories` or
    ///    `xdg` crates.
    /// 2. **User expectation** — terminal emulators like Alacritty use
    ///    `~/.config/alacritty/` on both Linux and macOS; Windows users
    ///    who want a cross-platform dotfiles setup can use the same path.
    /// 3. **Portability** — copying `~/.config/zenterm/` between machines
    ///    migrates all configuration.
    /// 4. **Transparency** — the path is simple and obvious.
    ///
    /// The `ZENTERM_CONFIG` environment variable overrides this path.
    pub fn path() -> PathBuf {
        match env::var_os("ZENTERM_CONFIG") {
            Some(p) => PathBuf::from(p),
            None => {
                let home = env::var("HOME")
                    .or_else(|_| env::var("USERPROFILE"))
                    .expect("neither $HOME nor $USERPROFILE is set; cannot locate config file");
                PathBuf::from(home).join(".config/zenterm/config.toml")
            }
        }
    }

    // ── Load ──────────────────────────────────────────────────────────

    /// Load configuration from the standard path.
    ///
    /// * **File not found** → log at info level, return `Ok(Config::default())`.
    /// * **File found but invalid** → log at error level, **return the error**.
    ///   The caller decides whether to abort or fall back to defaults.
    pub fn load() -> Result<Self, ConfigError> {
        let path = Self::path();

        if !path.exists() {
            log::info!("config not found at {:?}, using defaults", path);
            return Ok(Config::default());
        }

        let content = fs::read_to_string(&path)
            .map_err(|e| ConfigError::Io {
                path: path.clone(),
                source: e,
            })?;

        let config: Config = toml::from_str(&content).map_err(|e| ConfigError::Parse {
            path: path.clone(),
            source: e,
        })?;

        log::info!("loaded config from {:?}", path);
        Ok(config)
    }

    // ── Reload ────────────────────────────────────────────────────────

    /// Re-read the config file from disk.
    ///
    /// Returns `Ok(Some(config))` on success, `Ok(None)` if the file doesn't
    /// exist (keep current config), and `Err(e)` if the file exists but is
    /// malformed.
    pub fn reload() -> Result<Option<Self>, ConfigError> {
        let path = Self::path();

        if !path.exists() {
            log::info!("config file removed, keeping current settings");
            return Ok(None);
        }

        Self::load().map(Some)
    }

    // ── Save ──────────────────────────────────────────────────────────

    /// Serialise this configuration to the standard path.
    ///
    /// Creates parent directories if they don't exist.
    pub fn save(&self) -> Result<(), ConfigError> {
        let path = Self::path();

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| ConfigError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }

        let content =
            toml::to_string_pretty(self).map_err(|e| ConfigError::Serialize(e))?;

        fs::write(&path, &content).map_err(|e| ConfigError::Io {
            path,
            source: e,
        })?;

        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            window: WindowConfig::default(),
            font: FontConfig::default(),
            colors: ColorsConfig::default(),
            cursor: CursorConfig::default(),
            selection: SelectionConfig::default(),
            mouse: MouseConfig::default(),
            terminal: TerminalConfig::default(),
            keyboard: KeyboardConfig::default(),
            ui: UiConfig::default(),
        }
    }
}

// ── ConfigError ────────────────────────────────────────────────────────

/// Errors that can occur during config file loading / saving.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// I/O error reading or writing the config file.
    #[error("config I/O error at {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// TOML parse error.
    #[error("config parse error in {path:?}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    /// TOML serialisation error (should not happen in practice).
    #[error("config serialisation error: {0}")]
    Serialize(#[source] toml::ser::Error),
}
