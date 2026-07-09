//! Terminal state machine.
//!
//! This crate is the bridge between the raw VT parser output and the
//! alacritty_terminal grid state. It uses `vte::ansi::Processor` to
//! convert byte streams into semantic `Handler` calls on `Term`.

pub mod image;

mod term;

pub use term::ColorScheme;
pub use term::CursorInfo;
pub use term::GridView;
pub use term::Terminal;
