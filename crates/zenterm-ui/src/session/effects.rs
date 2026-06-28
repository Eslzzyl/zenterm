//! Per-session effects emitted to the application layer.

/// Effects emitted by [`TerminalSession::handle_side_effects`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEffect {
    /// The session requested a new window title (OSC 0/2).
    WindowTitle(String),
    /// The session requested the application close (terminal escape).
    CloseWindow,
}
