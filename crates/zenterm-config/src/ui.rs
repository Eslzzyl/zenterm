//! UI chrome configuration parsed from the `[ui]` section.
//!
//! Controls the optional **tab bar** (multi-terminal workspace) and
//! **workspace sidebar** (cmux-style vertical tab list).
//!
//! # Behaviour
//!
//! Both [`UiConfig::tabs_enabled`] and [`UiConfig::sidebar_enabled`]
//! default to `false`.  This preserves Phase 1 behaviour — a single
//! terminal fills the window — for users who have not opted in.
//!
//! When `tabs_enabled = false`:
//!
//! * The application renders exactly one `TerminalSession` (the one
//!   stored at `SessionId(0)`).
//! * The `egui_dock` `DockArea` is **not** allocated — there is no
//!   tab bar, no tab close buttons, no drag-to-reorder.
//! * The sidebar (if also enabled) is still rendered and shows the
//!   single active session, but switching it does nothing (no other
//!   sessions exist).
//!
//! When `tabs_enabled = true`:
//!
//! * Multiple `TerminalSession`s can coexist; each owns its own
//!   `PtySession` and `Terminal`.
//! * The `DockArea` draws a tab bar at the top of the workspace area.
//! * The sidebar (if enabled) lists every session and lets the user
//!   switch focus with a single click.
//!
//! # Layout persistence
//!
//! See `docs/usages/config.md` and `crates/zenterm-ui/src/layout_io.rs`
//! for the `dock.json` / `sessions.json` mechanism.  This struct only
//! controls whether persistence is **allowed**; the actual I/O is
//! performed by `LayoutIo` in the `zenterm-ui` crate.
//!
//! # ⚠  Maintenance note
//!
//! If you modify any field, default value, or enum variant in this
//! module, update [`docs/usages/config.md`] to match.

use serde::{Deserialize, Serialize};

// ── UiConfig ───────────────────────────────────────────────────────────

/// The `[ui]` section of the config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    /// Render the multi-tab `egui_dock::DockArea`.  Default `false`.
    ///
    /// When `false`, the application runs as a single-terminal
    /// emulator with no tab bar (Phase 1 behaviour).
    #[serde(default)]
    pub tabs_enabled: bool,

    /// Render the cmux-style workspace sidebar on the left (or
    /// right) edge of the window.  Default `false`.
    ///
    /// Has no effect when [`Self::tabs_enabled`] is `false`.
    #[serde(default)]
    pub sidebar_enabled: bool,

    /// Which edge of the window the sidebar docks to.
    #[serde(default)]
    pub sidebar_position: SidebarPosition,

    /// Default sidebar width in logical pixels (1× DPI).
    #[serde(default = "default_sidebar_width")]
    pub sidebar_width: f32,

    /// Minimum sidebar width (user-resize clamp).
    #[serde(default = "default_sidebar_min_width")]
    pub sidebar_min_width: f32,

    /// Maximum sidebar width (user-resize clamp).
    #[serde(default = "default_sidebar_max_width")]
    pub sidebar_max_width: f32,

    /// Show the `+` button on the tab bar to spawn a new shell.
    #[serde(default = "default_true")]
    pub show_add_tab_button: bool,

    /// Show a `×` close button on each tab.
    #[serde(default = "default_true")]
    pub show_close_tab_button: bool,

    /// Allow middle-click on a tab title to close that tab.
    #[serde(default = "default_true")]
    pub tab_close_on_middle_click: bool,

    /// Restore tab layout from `~/.config/zenterm/dock.json` on
    /// startup when present.
    #[serde(default = "default_true")]
    pub restore_layout_on_startup: bool,

    /// Persist tab layout to `dock.json` and session metadata to
    /// `sessions.json` as the user mutates them.
    #[serde(default = "default_true")]
    pub persist_layout: bool,

    /// Debounce window (milliseconds) between a layout mutation and
    /// the disk write.  Smaller values reduce data loss on crash;
    /// larger values reduce wear / write amplification.
    #[serde(default = "default_layout_debounce_ms")]
    pub layout_debounce_ms: u64,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            tabs_enabled: false,
            sidebar_enabled: false,
            sidebar_position: SidebarPosition::default(),
            sidebar_width: default_sidebar_width(),
            sidebar_min_width: default_sidebar_min_width(),
            sidebar_max_width: default_sidebar_max_width(),
            show_add_tab_button: default_true(),
            show_close_tab_button: default_true(),
            tab_close_on_middle_click: default_true(),
            restore_layout_on_startup: default_true(),
            persist_layout: default_true(),
            layout_debounce_ms: default_layout_debounce_ms(),
        }
    }
}

fn default_true() -> bool { true }
fn default_sidebar_width() -> f32 { 220.0 }
fn default_sidebar_min_width() -> f32 { 160.0 }
fn default_sidebar_max_width() -> f32 { 480.0 }
fn default_layout_debounce_ms() -> u64 { 500 }

// ── Sub-types ──────────────────────────────────────────────────────────

/// Which edge of the window the sidebar attaches to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SidebarPosition {
    #[default]
    #[serde(rename = "Left")]
    Left,
    #[serde(rename = "Right")]
    Right,
}
