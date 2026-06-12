//! Core types shared across all Zenmux crates.
//!
//! This crate intentionally has minimal dependencies. It defines the protocol
//! types that the PTY, terminal state, glyph atlas, renderer, input mapper,
//! and UI all share.

pub mod cell;
pub mod color;
pub mod damage;
pub mod error;
pub mod position;
pub mod size;

pub use cell::Cell;
pub use color::Rgba;
pub use damage::DamageSet;
pub use error::Error;
pub use position::TermPos;
pub use size::TermSize;

/// Convenience alias for [`Error`] results.
pub type Result<T> = std::result::Result<T, Error>;
