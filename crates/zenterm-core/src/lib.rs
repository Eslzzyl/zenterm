//! Core types shared across all Zenterm crates.
//!
//! This crate intentionally has minimal dependencies. It defines the protocol
//! types that the PTY, terminal state, glyph atlas, renderer, input mapper,
//! and UI all share.

pub mod cell;
pub mod color;
pub mod damage;
pub mod error;
pub mod image;
pub mod position;
pub mod size;
pub mod theme;

pub use cell::{Cell, UnderlineStyle};
pub use color::Rgba;
pub use damage::DamageSet;
pub use error::Error;
pub use position::TermPos;
pub use size::TermSize;
pub use theme::{Theme, ThemePreference, THEME_DARK, THEME_LIGHT};

/// Terminal-side progress state reported via the ConEmu OSC 9;4 protocol.
///
/// Applications emit `ESC ] 9 ; 4 ; <state> ; <pct> ST` to convey task
/// progress to the terminal.  Terminals that support this protocol render
/// it as a tab-bar or taskbar indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Progress {
    /// No progress (state 0).
    #[default]
    None,
    /// Normal progress percentage (state 1), 0–100.
    Percentage(u8),
    /// Error-state progress (state 2), 0–100.
    Error(u8),
    /// Indeterminate / busy (state 3).
    Indeterminate,
}

/// Convenience alias for [`Error`] results.
pub type Result<T> = std::result::Result<T, Error>;

/// The physical subpixel layout of the primary display.
///
/// LCD monitors arrange the red, green, and blue subpixels in one of two
/// horizontal orders: **RGB** (red–green–blue, the most common) or **BGR**
/// (blue–green–red, used by many Dell and Samsung panels, and most TVs).
///
/// Using the wrong layout causes visible colour fringing at text edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubpixelLayout {
    /// Standard RGB layout (subpixel order: R·G·B).
    Rgb,
    /// Inverted BGR layout (subpixel order: B·G·R).
    Bgr,
}

impl core::fmt::Display for SubpixelLayout {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Rgb => write!(f, "RGB"),
            Self::Bgr => write!(f, "BGR"),
        }
    }
}

/// Hinting mode for font rasterization.
///
/// Controls whether glyph outlines are snapped to the pixel grid (hinted)
/// for sharper rendering.  Hinting is most beneficial at low DPI / small
/// font sizes where the pixel grid is visible; at high DPI it can introduce
/// undesirable shape distortion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HintingMode {
    /// Never apply hinting.
    None,
    /// Apply hinting at low DPI, disable at high DPI (above ~1.04×).
    Auto,
    /// Always apply hinting.
    Full,
}

impl Default for HintingMode {
    fn default() -> Self {
        Self::Auto
    }
}

/// LCD subpixel or grayscale anti-aliasing mode.
///
/// Subpixel rendering gives sharper text on LCD displays but can produce
/// colour fringing on OLED or high-DPI screens.  Grayscale mode is safer
/// for non-LCD panels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderMode {
    /// LCD subpixel anti-aliasing (R/G/B per-channel coverage).
    Subpixel,
    /// Standard grayscale anti-aliasing (single alpha channel).
    Grayscale,
}

impl Default for RenderMode {
    fn default() -> Self {
        Self::Subpixel
    }
}

impl SubpixelLayout {
    /// Auto-detect the subpixel layout of the primary display.
    ///
    /// On Windows this queries the OS via `SystemParametersInfoW` (the same
    /// API that Chrome uses).  On other platforms it returns `Rgb` (macOS
    /// has not used subpixel rendering since 2018, and Linux GUI toolkits
    /// configure this via fontconfig).
    pub fn detect() -> Self {
        #[cfg(windows)]
        {
            Self::detect_windows()
        }
        #[cfg(not(windows))]
        {
            Self::Rgb
        }
    }

    /// Windows implementation: call `SystemParametersInfoW` to read the
    /// ClearType subpixel orientation from the OS.
    #[cfg(windows)]
    fn detect_windows() -> Self {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SystemParametersInfoW, SPI_GETFONTSMOOTHINGORIENTATION,
            FE_FONTSMOOTHINGORIENTATIONBGR,
        };

        let mut orientation: u32 = 0;
        // SAFETY: SystemParametersInfoW is a well-known Win32 API.
        // We pass a valid pointer to a `u32` whose lifetime is bounded by
        // this function, and interpret the result only on success (non-zero
        // return value).
        let ok = unsafe {
            SystemParametersInfoW(
                SPI_GETFONTSMOOTHINGORIENTATION,
                0,
                &mut orientation as *mut u32 as *mut _,
                0,
            )
        };

        if ok != 0 && orientation == FE_FONTSMOOTHINGORIENTATIONBGR {
            Self::Bgr
        } else {
            // Default to RGB: this is what most monitors use, and the OS
            // returns RGB even when ClearType is disabled or the monitor
            // has no subpixel structure (e.g. OLED).
            Self::Rgb
        }
    }
}
