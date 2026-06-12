//! Unified error type for Zenterm.

/// Result alias using [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

/// The error type for all Zenterm operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An I/O error (PTY read/write, etc.).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// PTY-specific error.
    #[error("PTY error: {0}")]
    Pty(String),

    /// Terminal initialisation error.
    #[error("Terminal error: {0}")]
    Terminal(String),

    /// Font / glyph error.
    #[error("Glyph error: {0}")]
    Glyph(String),

    /// GPU / rendering error.
    #[error("Render error: {0}")]
    Render(String),
}
