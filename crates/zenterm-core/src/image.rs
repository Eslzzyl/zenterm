use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Playback state for a Kitty animated image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnimationPlayback {
    #[default]
    Stopped,
    Running {
        /// `true` means reaching the last frame waits for a later frame.
        loading: bool,
        /// `None` means infinite looping; `Some(n)` is the number of
        /// remaining loop-backs after the current pass.
        loops_remaining: Option<u32>,
    },
}

impl AnimationPlayback {
    pub const fn running(loading: bool, loops_remaining: Option<u32>) -> Self {
        Self::Running {
            loading,
            loops_remaining,
        }
    }
}

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
        /// Shared so the render upload can borrow the decoded pixels without
        /// making a second full-size staging copy.  Frame edits use
        /// copy-on-write before mutating the buffer.
        data: Arc<Vec<u8>>,
        width: u32,
        height: u32,
        hash: [u8; 32],
    },
    /// Animated RGBA sequence.
    AnimRgba8 {
        width: u32,
        height: u32,
        frames: Vec<Arc<Vec<u8>>>,
        /// Gap from this frame to the next frame, in signed milliseconds:
        /// - positive: wait before advancing;
        /// - zero: no automatic timing (the root frame defaults to zero);
        /// - negative: gapless frame, skipped immediately during playback.
        durations: Vec<i32>,
        hashes: Vec<[u8; 32]>,
        current_frame: usize,
        playback: AnimationPlayback,
        next_frame_at: Option<Instant>,
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

    /// Advance a terminal-driven animation and report whether its visible
    /// frame changed.
    pub fn advance_animation(&self, now: Instant) -> bool {
        self.inner
            .lock()
            .expect("ImageData lock")
            .advance_animation(now)
    }

    /// Return the next wake-up deadline for a running animation.
    pub fn animation_deadline(&self) -> Option<Instant> {
        self.inner
            .lock()
            .expect("ImageData lock")
            .animation_deadline()
    }

    /// Return hashes for all pixel buffers that may have a GPU texture.
    pub fn frame_hashes(&self) -> Vec<[u8; 32]> {
        self.inner.lock().expect("ImageData lock").frame_hashes()
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

fn compute_frames_hash(frames: &[Arc<Vec<u8>>]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update((frames.len() as u64).to_le_bytes());
    for frame in frames {
        hasher.update(compute_hash(frame.as_slice()));
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
            data: Arc::new(data),
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
        let durations = durations
            .into_iter()
            .map(|duration| i32::try_from(duration.as_millis()).unwrap_or(i32::MAX))
            .collect();
        Self::new_anim_rgba8_with_gaps(frames, durations, width, height)
    }

    /// Construct an animated image using Kitty's signed frame gaps.
    pub fn new_anim_rgba8_with_gaps(
        frames: Vec<Vec<u8>>,
        durations: Vec<i32>,
        width: u32,
        height: u32,
    ) -> Self {
        let frames: Vec<Arc<Vec<u8>>> = frames.into_iter().map(Arc::new).collect();
        let hashes: Vec<[u8; 32]> = frames.iter().map(|f| compute_hash(f.as_slice())).collect();
        Self::AnimRgba8 {
            width,
            height,
            frames,
            durations,
            hashes,
            current_frame: 0,
            playback: AnimationPlayback::Stopped,
            next_frame_at: None,
        }
    }

    /// Convert a regular image into a one-frame animation.
    pub fn promote_to_animation(&mut self) {
        let Self::Rgba8 {
            data,
            width,
            height,
            ..
        } = self
        else {
            return;
        };
        let data = std::mem::take(data);
        let width = *width;
        let height = *height;
        *self = Self::AnimRgba8 {
            width,
            height,
            hashes: vec![compute_hash(data.as_slice())],
            frames: vec![data],
            durations: vec![0],
            current_frame: 0,
            playback: AnimationPlayback::Stopped,
            next_frame_at: None,
        };
    }

    /// Return the hashes of all frame pixel buffers.
    pub fn frame_hashes(&self) -> Vec<[u8; 32]> {
        match self {
            Self::Rgba8 { data, .. } => vec![compute_hash(data.as_slice())],
            Self::AnimRgba8 { frames, .. } => frames
                .iter()
                .map(|frame| compute_hash(frame.as_slice()))
                .collect(),
        }
    }

    pub fn current_frame(&self) -> usize {
        match self {
            Self::Rgba8 { .. } => 0,
            Self::AnimRgba8 { current_frame, .. } => *current_frame,
        }
    }

    pub fn animation_deadline(&self) -> Option<Instant> {
        match self {
            Self::AnimRgba8 {
                playback: AnimationPlayback::Running { .. },
                next_frame_at,
                ..
            } => *next_frame_at,
            _ => None,
        }
    }

    pub fn stop_animation(&mut self) {
        if let Self::AnimRgba8 {
            playback,
            next_frame_at,
            ..
        } = self
        {
            *playback = AnimationPlayback::Stopped;
            *next_frame_at = None;
        }
    }

    pub fn set_animation_current_frame(
        &mut self,
        frame_number: usize,
        now: Instant,
    ) -> Result<(), String> {
        let Self::AnimRgba8 {
            current_frame,
            frames,
            durations,
            playback,
            next_frame_at,
            ..
        } = self
        else {
            return Err("image is not animated".into());
        };
        if frame_number == 0 || frame_number > frames.len() {
            return Err(format!(
                "frame {frame_number} out of range (1..={})",
                frames.len()
            ));
        }
        *current_frame = frame_number - 1;
        *next_frame_at = if matches!(playback, AnimationPlayback::Running { .. }) {
            Self::schedule_frame(*current_frame, durations, now)
        } else {
            None
        };
        Ok(())
    }

    pub fn set_animation_gap(
        &mut self,
        frame_number: usize,
        gap_ms: i32,
        now: Instant,
    ) -> Result<(), String> {
        let Self::AnimRgba8 {
            durations,
            frames,
            current_frame,
            playback,
            next_frame_at,
            ..
        } = self
        else {
            return Err("image is not animated".into());
        };
        if frame_number == 0 || frame_number > frames.len() {
            return Err(format!(
                "frame {frame_number} out of range (1..={})",
                frames.len()
            ));
        }
        durations[frame_number - 1] = gap_ms;
        if frame_number - 1 == *current_frame {
            *next_frame_at = if matches!(playback, AnimationPlayback::Running { .. }) {
                Self::schedule_frame(*current_frame, durations, now)
            } else {
                None
            };
        }
        Ok(())
    }

    pub fn start_animation(
        &mut self,
        loading: bool,
        loops: Option<u32>,
        now: Instant,
    ) -> Result<(), String> {
        let Self::AnimRgba8 {
            frames,
            current_frame,
            durations,
            playback,
            next_frame_at,
            ..
        } = self
        else {
            return Err("image is not animated".into());
        };
        if frames.is_empty() {
            return Err("animation has no frames".into());
        }
        *playback = AnimationPlayback::running(loading, loops);
        *next_frame_at = Self::schedule_frame(*current_frame, durations, now);
        Ok(())
    }

    pub fn advance_animation(&mut self, now: Instant) -> bool {
        let Self::AnimRgba8 {
            frames,
            durations,
            current_frame,
            playback,
            next_frame_at,
            ..
        } = self
        else {
            return false;
        };
        let Some(deadline) = *next_frame_at else {
            return false;
        };
        if deadline > now || frames.is_empty() {
            return false;
        }

        let mut changed = false;
        let max_steps = frames.len().saturating_mul(2).max(1);
        for _ in 0..max_steps {
            let old_frame = *current_frame;
            let next = old_frame + 1;
            if next >= frames.len() {
                match playback {
                    AnimationPlayback::Running { loading: true, .. } => {
                        *next_frame_at = None;
                        break;
                    }
                    AnimationPlayback::Running {
                        loops_remaining: Some(0),
                        ..
                    } => {
                        *playback = AnimationPlayback::Stopped;
                        *next_frame_at = None;
                        break;
                    }
                    AnimationPlayback::Running {
                        loops_remaining: Some(remaining),
                        ..
                    } => {
                        *remaining = remaining.saturating_sub(1);
                        *current_frame = 0;
                    }
                    AnimationPlayback::Running {
                        loops_remaining: None,
                        ..
                    } => {
                        *current_frame = 0;
                    }
                    AnimationPlayback::Stopped => {
                        *next_frame_at = None;
                        break;
                    }
                }
            } else {
                *current_frame = next;
            }
            changed |= *current_frame != old_frame;

            let gap = durations.get(*current_frame).copied().unwrap_or(0);
            *next_frame_at = Some(match gap {
                gap if gap < 0 => now,
                gap => now + Duration::from_millis(gap as u64),
            });
            if gap >= 0 {
                break;
            }
        }
        changed
    }

    /// Resume a run-wait animation after a new frame was appended.
    pub fn resume_loading_animation(&mut self, now: Instant) {
        let Self::AnimRgba8 {
            frames,
            current_frame,
            durations,
            playback,
            next_frame_at,
            ..
        } = self
        else {
            return;
        };
        if matches!(playback, AnimationPlayback::Running { loading: true, .. })
            && next_frame_at.is_none()
            && *current_frame + 1 < frames.len()
        {
            *next_frame_at = Self::schedule_frame(*current_frame, durations, now);
        }
    }

    fn schedule_frame(current_frame: usize, durations: &[i32], now: Instant) -> Option<Instant> {
        match durations.get(current_frame).copied().unwrap_or(0) {
            gap if gap < 0 => Some(now),
            gap => Some(now + Duration::from_millis(gap as u64)),
        }
    }

    pub fn hash(&self) -> [u8; 32] {
        match self {
            Self::Rgba8 { data, .. } => compute_hash(data.as_slice()),
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
    use super::{AnimationPlayback, ImageData, ImageDataType, hash_bytes};
    use std::time::{Duration, Instant};

    #[test]
    fn hash_tracks_in_place_single_frame_updates() {
        let image = ImageData::new(ImageDataType::new_rgba8(vec![1, 2, 3, 4], 1, 1));
        let original = image.hash();

        if let ImageDataType::Rgba8 { data, .. } = &mut *image.data() {
            std::sync::Arc::make_mut(data)[0] = 9;
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
            std::sync::Arc::make_mut(&mut frames[0])[0] = 9;
        }

        assert_ne!(image.hash(), original);
    }

    #[test]
    fn animation_advances_with_signed_gaps_and_loops() {
        let image = ImageData::new(ImageDataType::new_anim_rgba8_with_gaps(
            vec![vec![1, 2, 3, 4], vec![5, 6, 7, 8]],
            vec![10, 20],
            1,
            1,
        ));
        let start = Instant::now();
        image
            .data()
            .start_animation(false, Some(1), start)
            .expect("start animation");
        assert_eq!(image.data().current_frame(), 0);
        assert!(!image.advance_animation(start + Duration::from_millis(5)));
        assert_eq!(image.data().current_frame(), 0);
        assert!(image.advance_animation(start + Duration::from_millis(10)));
        assert_eq!(image.data().current_frame(), 1);
        assert!(image.advance_animation(start + Duration::from_millis(30)));
        assert_eq!(image.data().current_frame(), 0);
        assert!(image.advance_animation(start + Duration::from_millis(40)));
        assert_eq!(image.data().current_frame(), 1);
        assert!(!image.advance_animation(start + Duration::from_millis(60)));
        assert!(matches!(
            &*image.data(),
            ImageDataType::AnimRgba8 {
                playback: AnimationPlayback::Stopped,
                ..
            }
        ));
    }

    #[test]
    fn run_wait_resumes_when_a_frame_is_appended() {
        let image = ImageData::new(ImageDataType::new_anim_rgba8_with_gaps(
            vec![vec![1, 2, 3, 4], vec![5, 6, 7, 8]],
            vec![1, 1],
            1,
            1,
        ));
        let start = Instant::now();
        image
            .data()
            .start_animation(true, None, start)
            .expect("start animation");
        image.advance_animation(start + Duration::from_millis(1));
        image.advance_animation(start + Duration::from_millis(2));
        assert!(image.animation_deadline().is_none());

        {
            let mut data = image.data();
            if let ImageDataType::AnimRgba8 {
                frames,
                durations,
                hashes,
                ..
            } = &mut *data
            {
                let frame = std::sync::Arc::new(vec![9, 10, 11, 12]);
                frames.push(frame.clone());
                durations.push(1);
                hashes.push(hash_bytes(frame.as_slice()));
                data.resume_loading_animation(start + Duration::from_millis(2));
            }
        }
        assert!(image.animation_deadline().is_some());
        assert!(image.advance_animation(start + Duration::from_millis(3)));
        assert_eq!(image.data().current_frame(), 2);
    }
}
