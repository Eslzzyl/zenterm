use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// Texture coordinate in normalized UV space `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextureCoordinate {
    pub x: f32,
    pub y: f32,
}

impl TextureCoordinate {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// Raw pixel data for a single decoded image.
#[derive(Debug, Clone)]
pub enum ImageDataType {
    /// Single decoded RGBA frame.
    Rgba8 {
        data: Vec<u8>,
        width: u32,
        height: u32,
        hash: [u8; 32],
    },
    /// Animated RGBA sequence.
    AnimRgba8 {
        width: u32,
        height: u32,
        frames: Vec<Vec<u8>>,
        durations: Vec<Duration>,
        hashes: Vec<[u8; 32]>,
    },
}

/// Thread-safe, ARC-wrapped image data deduplicated by content hash.
///
/// Multiple [`ImageCell`]s spanning different cells of the same image
/// share a single `Arc<ImageData>`.
#[derive(Debug, Clone)]
pub struct ImageData {
    inner: Arc<Mutex<ImageDataType>>,
}

impl ImageData {
    pub fn new(data: ImageDataType) -> Self {
        Self {
            inner: Arc::new(Mutex::new(data)),
        }
    }

    pub fn data(&self) -> MutexGuard<'_, ImageDataType> {
        self.inner.lock().expect("ImageData lock")
    }

    /// Return the current content hash.
    ///
    /// Image frames may be updated in place by the Kitty protocol, so this
    /// must be derived from the current payload rather than cached at the
    /// time the `Arc<ImageData>` is created.
    pub fn hash(&self) -> [u8; 32] {
        self.data().hash()
    }

    pub fn len(&self) -> usize {
        let guard = self.inner.lock().expect("ImageData lock");
        match &*guard {
            ImageDataType::Rgba8 { data, .. } => data.len(),
            ImageDataType::AnimRgba8 { frames, .. } => frames.iter().map(|f| f.len()).sum(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Per-cell slice of a multi-cell image placement.
///
/// When an image spans multiple terminal cells, each cell holds one
/// `ImageCell` with the UV sub-rectangle that maps to that cell's portion
/// of the full image.
#[derive(Debug, Clone)]
pub struct ImageCell {
    /// UV top-left of this cell's slice in the full image.
    pub top_left: TextureCoordinate,
    /// UV bottom-right of this cell's slice.
    pub bottom_right: TextureCoordinate,
    /// Reference to the shared image data.
    pub data: Arc<ImageData>,
    /// Compositing layer: negative = behind text, >= 0 = above text.
    pub z_index: i32,
    /// Pixel padding from the left edge of this cell (Kitty protocol).
    pub padding_left: u16,
    /// Pixel padding from the top edge.
    pub padding_top: u16,
    /// Pixel padding from the right edge.
    pub padding_right: u16,
    /// Pixel padding from the bottom edge.
    pub padding_bottom: u16,
    /// Kitty protocol image identifier.
    pub image_id: Option<u32>,
    /// Kitty protocol placement identifier.
    pub placement_id: Option<u32>,
}

impl ImageCell {
    pub fn new(
        top_left: TextureCoordinate,
        bottom_right: TextureCoordinate,
        data: Arc<ImageData>,
    ) -> Self {
        Self {
            top_left,
            bottom_right,
            data,
            z_index: 0,
            padding_left: 0,
            padding_top: 0,
            padding_right: 0,
            padding_bottom: 0,
            image_id: None,
            placement_id: None,
        }
    }
}

// ── helpers ──────────────────────────────────────────────────────────

fn compute_hash(data: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(data).into()
}

fn compute_frames_hash(frames: &[Vec<u8>]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update((frames.len() as u64).to_le_bytes());
    for frame in frames {
        hasher.update(compute_hash(frame));
    }
    hasher.finalize().into()
}

/// Hash raw image bytes for callers that update animation frames in place.
pub fn hash_bytes(data: &[u8]) -> [u8; 32] {
    compute_hash(data)
}

impl ImageDataType {
    /// Construct a single RGBA frame, computing its content hash.
    pub fn new_rgba8(data: Vec<u8>, width: u32, height: u32) -> Self {
        let hash = compute_hash(&data);
        Self::Rgba8 {
            data,
            width,
            height,
            hash,
        }
    }

    /// Construct an animated image from existing RGBA frames.
    /// Each frame must have the same `width` and `height`.
    /// `durations[i]` is the display duration of frame `i`.
    pub fn new_anim_rgba8(
        frames: Vec<Vec<u8>>,
        durations: Vec<std::time::Duration>,
        width: u32,
        height: u32,
    ) -> Self {
        let hashes: Vec<[u8; 32]> = frames.iter().map(|f| compute_hash(f)).collect();
        Self::AnimRgba8 {
            width,
            height,
            frames,
            durations,
            hashes,
        }
    }

    pub fn hash(&self) -> [u8; 32] {
        match self {
            Self::Rgba8 { data, .. } => compute_hash(data),
            Self::AnimRgba8 { frames, .. } => compute_frames_hash(frames),
        }
    }

    pub fn width(&self) -> u32 {
        match self {
            Self::Rgba8 { width, .. } | Self::AnimRgba8 { width, .. } => *width,
        }
    }

    pub fn height(&self) -> u32 {
        match self {
            Self::Rgba8 { height, .. } | Self::AnimRgba8 { height, .. } => *height,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ImageData, ImageDataType};

    #[test]
    fn hash_tracks_in_place_single_frame_updates() {
        let image = ImageData::new(ImageDataType::new_rgba8(vec![1, 2, 3, 4], 1, 1));
        let original = image.hash();

        if let ImageDataType::Rgba8 { data, .. } = &mut *image.data() {
            data[0] = 9;
        }

        assert_ne!(image.hash(), original);
    }

    #[test]
    fn hash_tracks_in_place_animation_frame_updates() {
        let image = ImageData::new(ImageDataType::new_anim_rgba8(
            vec![vec![1, 2, 3, 4]],
            vec![],
            1,
            1,
        ));
        let original = image.hash();

        if let ImageDataType::AnimRgba8 { frames, .. } = &mut *image.data() {
            frames[0][0] = 9;
        }

        assert_ne!(image.hash(), original);
    }
}
