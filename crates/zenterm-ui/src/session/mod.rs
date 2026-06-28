//! A single terminal session: one PTY, one VT state machine, one
//! per-session render slice, one `CallbackHandle`.
//!
//! This struct used to live inline in [`crate::app::ZentermApp`].
//! When `config.ui.tabs_enabled = true`, multiple `TerminalSession`s
//! coexist — each in its own dock tab — and share the
//! [`SharedGpuContext`](crate::gpu::SharedGpuContext),
//! [`SharedGlyphAtlas`](crate::glyph_cache::SharedGlyphAtlas), and
//! [`SharedRenderState`](zenterm_render::callback::SharedRenderState).
//!
//! # Module layout
//!
//! * [`types`] — core type definitions (`SessionId`, `TerminalSession`, …)
//! * [`new`] — `TerminalSession::new()` constructor
//! * [`pty`] — PTY pumping, side-effect handling, SGR mouse
//! * [`reinit`] — viewport/dock helpers, DPI reinit, resize, config changes
//! * [`mouse`] — per-tab mouse interaction, scrollbar, context menu
//! * [`render`] — cell-instance generation (`update_cell_instances`)
//! * [`shaping`] — ligature run detection and text extraction
//! * [`osc7`] — OSC 7 working-directory URL parsing
//! * [`effects`] — `SessionEffect` enum
//!
//! # Rendering contract
//!
//! The render pipeline is unchanged from Phase 1:
//!
//! 1. [`TerminalSession::draw`] is called from the egui UI thread with
//!    the per-session `Ui`.  It builds a `Vec<CellInstance>` describing
//!    the visible cells in **clip space** (NDC, range -1..1).
//! 2. Each instance is positioned relative to the **dock viewport**,
//!    not the local session rect.  This is what allows the GPU to draw
//!    every tab in a single instanced call: a session that lives at
//!    dock pixel `(200, 0)` simply adds 200 to all of its cell
//!    `x_px` values before the clip-space conversion.
//! 3. After all sessions have been visited, the concatenated buffer is
//!    handed to the wgpu callback via the shared
//!    `SharedRenderState.instances`.  The callback draws everything
//!    with the existing instanced-quad pipeline — **no shader change
//!    is required**.
//!
//! # Side-effects
//!
//! OSC 7 (`\x1b]7;file://…\x07`) is parsed to update
//! [`TerminalSession::cwd`]; OSC 0/2 update the title used by the
//! dock tab and (legacy path) the window title.

mod effects;
mod mouse;
mod new;
mod osc7;
mod pty;
mod reinit;
mod render;
mod shaping;
mod types;

pub use effects::SessionEffect;
pub use types::{SessionId, TerminalSession};
