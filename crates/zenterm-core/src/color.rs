//! RGBA color type for terminal rendering.

/// An RGBA color with components in `[0.0, 1.0]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba(pub [f32; 4]);

impl Rgba {
    /// Transparent black.
    pub const TRANSPARENT: Self = Self([0.0; 4]);

    /// Opaque black.
    pub const BLACK: Self = Self([0.0, 0.0, 0.0, 1.0]);

    /// Opaque white.
    pub const WHITE: Self = Self([1.0, 1.0, 1.0, 1.0]);

    /// Create a new RGBA color from float components.
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self([r, g, b, a])
    }

    /// Create an opaque RGB color from float components.
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self([r, g, b, 1.0])
    }

    /// Create from 8-bit integer components (0-255).
    pub const fn from_u8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self([
            (r as f32) * (1.0 / 255.0),
            (g as f32) * (1.0 / 255.0),
            (b as f32) * (1.0 / 255.0),
            (a as f32) * (1.0 / 255.0),
        ])
    }

    /// Red component.
    pub fn r(&self) -> f32 {
        self.0[0]
    }

    /// Green component.
    pub fn g(&self) -> f32 {
        self.0[1]
    }

    /// Blue component.
    pub fn b(&self) -> f32 {
        self.0[2]
    }

    /// Alpha component.
    pub fn a(&self) -> f32 {
        self.0[3]
    }
}

impl From<[f32; 4]> for Rgba {
    fn from(arr: [f32; 4]) -> Self {
        Self(arr)
    }
}

impl From<(u8, u8, u8)> for Rgba {
    fn from((r, g, b): (u8, u8, u8)) -> Self {
        Self::from_u8(r, g, b, 255)
    }
}
