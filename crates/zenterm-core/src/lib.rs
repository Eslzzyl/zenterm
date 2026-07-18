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

// ── iTerm2 proprietary escape codes (OSC 1337) ──────────────────────────

/// iTerm2 proprietary escape code (OSC 1337) sub-commands.
///
/// Parsed by `scan_oscs` → dispatch in `Terminal::feed`.  Some variants are
/// handled immediately (ClearScrollback, Copy, CurrentDir, etc.); others are
/// stored as pending and consumed by the UI layer.
///
/// Spec: <https://iterm2.com/documentation-escape-codes.html>
/// Reference: wezterm/wezterm-escape-parser/src/osc.rs (`ITermProprietary`)
#[derive(Debug, Clone, PartialEq)]
pub enum ITermProprietary {
    /// Record a navigation mark at the current cursor position.
    /// `ESC ] 1337 ; SetMark ST`
    SetMark,
    /// Bring the terminal window to the foreground.
    /// `ESC ] 1337 ; StealFocus ST`
    StealFocus,
    /// Erase the scrollback history.
    /// `ESC ] 1337 ; ClearScrollback ST`
    ClearScrollback,
    /// Inform the terminal of the current working directory.
    /// `ESC ] 1337 ; CurrentDir=/path ST`
    CurrentDir(String),
    /// Change the session's profile on the fly.
    /// `ESC ] 1337 ; SetProfile=name ST`
    SetProfile(String),
    /// Show or hide the cursor guide (highlight cursor line).
    /// `ESC ] 1337 ; HighlightCursorLine=yes ST`
    HighlightCursorLine(bool),
    /// Query cell pixel dimensions (sent by the application).
    /// The application sends the keyword `ReportCellSize` without args,
    /// which we interpret as a request.
    /// `ESC ] 1337 ; ReportCellSize ST`
    RequestCellSize,
    /// Response to RequestCellSize: cell pixel dimensions.
    /// `ESC ] 1337 ; ReportCellSize=<h>;<w>[;<scale>] ST`
    ReportCellSize {
        /// Cell height in pixels.
        height_pixels: f32,
        /// Cell width in pixels.
        width_pixels: f32,
        /// Optional DPI-based scale factor (macOS 72 DPI base).
        scale: Option<f32>,
    },
    /// Place base64-decoded text on the system pasteboard.
    /// `ESC ] 1337 ; Copy=;base64data ST`
    Copy(String),
    /// Request the value of a session variable.
    /// `ESC ] 1337 ; ReportVariable=base64name ST`
    ReportVariable(String),
    /// Set a user-defined variable.
    /// `ESC ] 1337 ; SetUserVar=name=base64value ST`
    SetUserVar {
        /// Variable name (plain text).
        name: String,
        /// Variable value (base64-decoded).
        value: String,
    },
    /// Set the badge format string.
    /// `ESC ] 1337 ; SetBadgeFormat=base64 ST`
    SetBadgeFormat(String),
    /// File transfer / inline image (the iTerm2 Inline Images Protocol).
    /// `ESC ] 1337 ; File=key=val:base64data ST`
    File(ITermFileData),
    /// Configure Unicode version.
    /// `ESC ] 1337 ; UnicodeVersion=N ST`
    /// `ESC ] 1337 ; UnicodeVersion=push [label] ST`
    /// `ESC ] 1337 ; UnicodeVersion=pop [label] ST`
    UnicodeVersion(ITermUnicodeVersionOp),
}

/// Unicode version stack operations for `ITermProprietary::UnicodeVersion`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ITermUnicodeVersionOp {
    /// Set the Unicode version to a specific value.
    Set(u8),
    /// Push the current version onto a stack with an optional label.
    Push(Option<String>),
    /// Pop the version stack.  If a label is provided, pop until that label.
    Pop(Option<String>),
}

/// Dimension specification for iTerm2 inline images.
///
/// Used by `ITermFileData` to specify desired width/height of the
/// rendered image.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ITermDimension {
    /// Compute automatically from image pixel size ÷ cell pixel size.
    Automatic,
    /// Fixed number of terminal cells.
    Cells(isize),
    /// Fixed number of pixels.
    Pixels(isize),
    /// Percentage of the terminal width/height.
    Percent(isize),
}

impl Default for ITermDimension {
    fn default() -> Self {
        Self::Automatic
    }
}

/// Parsed iTerm2 file transfer / inline image data (OSC 1337 ; File=…).
///
/// Spec: <https://iterm2.com/documentation-images.html>
#[derive(Debug, Clone, PartialEq)]
pub struct ITermFileData {
    /// File name (base64-encoded in the OSC, decoded here).
    pub name: Option<String>,
    /// Total file size in bytes (for progress display).
    pub size: Option<usize>,
    /// Desired render width.
    pub width: ITermDimension,
    /// Desired render height.
    pub height: ITermDimension,
    /// Whether to preserve aspect ratio when fitting to width/height.
    pub preserve_aspect_ratio: bool,
    /// Whether to display the image inline in the terminal.
    /// If false, the file should be downloaded instead.
    pub inline: bool,
    /// Whether to leave the cursor at its current position after display.
    pub do_not_move_cursor: bool,
    /// The raw file/ image data (base64-decoded from the OSC payload).
    pub data: Vec<u8>,
}

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

/// Urgency level for Kitty OSC 99 desktop notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KittyUrgency {
    /// Low urgency (`u=0`).
    Low,
    /// Normal urgency (`u=1`, default).
    #[default]
    Normal,
    /// Critical urgency (`u=2`).
    Critical,
}

/// When to show a Kitty OSC 99 notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KittyOccasion {
    /// Always show the notification (default).
    #[default]
    Always,
    /// Only show when the terminal window does not have keyboard focus.
    Unfocused,
    /// Only show when the terminal window is not visible to the user
    /// (e.g. in an inactive tab or background OS window).
    Invisible,
}

/// A fully assembled Kitty desktop notification (OSC 99).
///
/// Produced by the parser once a notification is complete (d=1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KittyNotification {
    /// The notification identifier (`i=`), if any.
    pub id: Option<String>,
    /// Notification title (`p=title` payloads, concatenated).
    pub title: String,
    /// Notification body (`p=body` payloads, concatenated).
    pub body: String,
    /// Application name (`f=`, base64-decoded), if set.
    pub app_name: Option<String>,
    /// Urgency level (`u=`).
    pub urgency: KittyUrgency,
    /// When to show the notification (`o=`).
    pub occasion: KittyOccasion,
    /// System sound name (`s=`, base64-decoded), if set.
    pub sound: Option<String>,
    /// Icon names (`n=`, base64-decoded), in order.
    pub icon_names: Vec<String>,
    /// Raw icon data from `p=icon` (when `e=1`).
    pub icon_data: Vec<u8>,
    /// Icon cache key (`g=`), for reusing transmitted icon data.
    pub icon_cache_key: Option<String>,
    /// Notification types (`t=`, base64-decoded).  Multiple values allowed.
    pub notification_types: Vec<String>,
    /// Button labels from `p=buttons`, split by U+2028.
    pub buttons: Vec<String>,
    /// Auto-close timeout in milliseconds from `w=`:
    ///   -1 → system default
    ///    0 → never expire
    ///   >0 → close after N ms
    pub timeout_ms: i32,
    /// If true, send escape code when notification is clicked (`a=report`).
    pub report_click: bool,
    /// If true, send escape code when notification is closed (`c=1`).
    pub close_report: bool,
}

/// The kind of a prompt in the FinalTerm semantic prompt protocol (OSC 133 P).
///
/// Corresponds to the `k` parameter in `ESC ] 133 ; P ; k=X ST`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticPromptKind {
    /// Normal left-side primary prompt (`k=i`).
    Initial,
    /// Right-aligned prompt (`k=r`).
    RightSide,
    /// Continuation prompt for editable input (`k=c`).
    Continuation,
    /// Continuation prompt where the input cannot be edited (`k=s`).
    Secondary,
}

impl Default for SemanticPromptKind {
    fn default() -> Self {
        Self::Initial
    }
}

/// Click behaviour for semantic prompt regions (OSC 133 `cl` parameter).
///
/// Controls how cursor keys navigate within a multi-line prompt or input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticClick {
    /// Allow motion only within the single input line.
    Line,
    /// Allow moving between multiple lines of input.
    MultipleLine,
    /// Allow left/right and conservative up/down motion.
    ConservativeVertical,
    /// Allow full arrow-key motion with smart horizontal placement.
    SmartVertical,
}

/// A parsed FinalTerm semantic prompt (OSC 133).
///
/// See <https://gitlab.freedesktop.org/Per_Bothner/specifications/blob/master/proposals/semantic-prompts.md>
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticPrompt {
    /// `L` — Do a fresh line (cursor to column 0, new line if not already there).
    FreshLine,
    /// `A` — Fresh line and then start prompt mode.
    FreshLineAndStartPrompt {
        /// Optional action identifier for response matching.
        aid: Option<String>,
        /// Optional click-behaviour hint.
        cl: Option<SemanticClick>,
    },
    /// `N` — End of command output, fresh line, then start prompt mode.
    MarkEndOfCommandWithFreshLine {
        /// Optional action identifier.
        aid: Option<String>,
        /// Optional click-behaviour hint.
        cl: Option<SemanticClick>,
    },
    /// `P` — Start a prompt of the given kind.
    StartPrompt(SemanticPromptKind),
    /// `B` — End of prompt, start of user input (until next marker).
    MarkEndOfPromptAndStartOfInputUntilNextMarker,
    /// `I` — End of prompt, start of user input (until end of line).
    MarkEndOfPromptAndStartOfInputUntilEndOfLine,
    /// `C` — End of input, start of command output.
    MarkEndOfInputAndStartOfOutput {
        /// Optional action identifier.
        aid: Option<String>,
    },
    /// `D` — Command finished with the given exit status.
    CommandStatus {
        /// Exit status (0 = success, non-zero = failure).
        status: i32,
        /// Optional action identifier.
        aid: Option<String>,
    },
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
