//! Zenterm UI — the eframe application that wires everything together.
//!
//! Module layout (Phase 2):
//!
//! - [`app`] — top-level eframe::App orchestrator
//! - [`gpu`] — shared wgpu::Device/Queue/SharedRenderState handle
//! - [`glyph_cache`] — shared glyph atlas across all sessions
//! - [`session`] — per-tab `TerminalSession`
//! - [`workspace`] — workspace management (WorkspaceManager, WorkspaceState)
//! - [`tab_viewer`] — `egui_dock::TabViewer` implementation
//! - [`sidebar`] — cmux-style vertical tab list
//! - [`layout_io`] — `dock.json` + `sessions.json` persistence
//! - [`legacy`] — single-terminal fallback (when `tabs_enabled = false`)

mod app;
mod gpu;
mod glyph_cache;
mod layout_io;
mod legacy;
mod session;
mod sidebar;
mod tab_viewer;
mod workspace;

pub use app::ZentermApp;
pub use session::{SessionEffect, SessionId, TerminalSession};
pub use workspace::{WorkspaceId, WorkspaceManager, WorkspaceState};
