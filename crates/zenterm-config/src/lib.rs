//! Zenterm configuration — TOML-based settings for the terminal emulator.
//!
//! # Overview
//!
//! The config file lives at `~/.config/zenterm/config.toml` on all platforms
//! (overridable via the `ZENTERM_CONFIG` environment variable).
//! Every section is optional — missing fields use sensible defaults that
//! mirror the original hardcoded behaviour.
//!
//! # Usage
//!
//! ```rust
//! let config = zenterm_config::Config::load().unwrap_or_default();
//! ```
//!
//! # ⚠  Maintenance note — docs must stay in sync with code
//!
//! The user-facing configuration reference is at
//! [`docs/usages/config.md`](https://github.com/.../blob/main/docs/usages/config.md).
//!
//! **If you add, remove, or rename any configuration field or value**
//! (in any of the sub-modules below), you **MUST** update that document
//! to match.  This includes:
//!
//! - New sections / subsections
//! - New keys or changes to existing key names
//! - Changes to default values
//! - New enum variants
//! - Changes to value type or format
//!
//! The documentation is the single source of truth for users;
//! the Rust types are the single source of truth for the implementation.
//! They must agree.

mod config;

pub mod colors;
pub mod cursor;
pub mod font;
pub mod keyboard;
pub mod mouse;
pub mod selection;
pub mod terminal;
pub mod window;

pub use config::{Config, ConfigError};
