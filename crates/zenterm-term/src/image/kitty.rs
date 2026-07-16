//! Kitty terminal graphics protocol parser (`\x1b_G…\x1b\\`).
//!
//! Ported from wezterm's `wezterm-escape-parser/src/apc.rs`.
//! Implements the protocol described at
//! <https://github.com/kovidgoyal/kitty/blob/master/docs/graphics-protocol.rst>

use std::collections::BTreeMap;
use std::sync::Arc;

use image::GenericImageView;
use image::{load_from_memory, RgbImage};

use zenterm_core::image::{ImageData, ImageDataType};

use crate::image::ImageCache;

// ── helpers ────────────────────────────────────────────────────────────

fn get<'a>(keys: &BTreeMap<&'a str, &'a str>, k: &str) -> Option<&'a str> {
    keys.get(k).map(|&s| s)
}

fn geti<T: std::str::FromStr>(keys: &BTreeMap<&str, &str>, k: &str) -> Option<T> {
    get(keys, k).and_then(|s| s.parse().ok())
}

// ── data source ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KittyImageData {
    Direct(String),
    DirectBin(Vec<u8>),
    File { path: String, data_size: Option<u32>, data_offset: Option<u32> },
    TemporaryFile { path: String, data_size: Option<u32>, data_offset: Option<u32> },
    SharedMem { name: String, data_size: Option<u32>, data_offset: Option<u32> },
}

impl KittyImageData {
    fn from_keys(keys: &BTreeMap<&str, &str>, payload: &[u8]) -> Option<Self> {
        match get(keys, "t").unwrap_or("d") {
            "d" => Some(Self::Direct(String::from_utf8(payload.to_vec()).ok()?)),
            "f" => Some(Self::File {
                path: String::from_utf8(decode_base64(payload).ok()?).ok()?,
                data_size: geti(keys, "S"),
                data_offset: geti(keys, "O"),
            }),
            "t" => Some(Self::TemporaryFile {
                path: String::from_utf8(decode_base64(payload).ok()?).ok()?,
                data_size: geti(keys, "S"),
                data_offset: geti(keys, "O"),
            }),
            "s" => Some(Self::SharedMem {
                name: String::from_utf8(decode_base64(payload).ok()?).ok()?,
                data_size: geti(keys, "S"),
                data_offset: geti(keys, "O"),
            }),
            _ => None,
        }
    }

    pub fn load_data(self) -> Result<Vec<u8>, String> {
        match self {
            Self::Direct(data) => decode_base64(data.as_bytes()),
            Self::DirectBin(bin) => Ok(bin),
            Self::File { path, data_offset, data_size } => {
                read_file_data(&path, data_offset, data_size)
            }
            Self::TemporaryFile { path, data_offset, data_size } => {
                let data = read_file_data(&path, data_offset, data_size)?;
                // Only remove if the path is in a known temp directory.
                if path.starts_with("/tmp/")
                    || path.starts_with("/var/tmp/")
                    || path.starts_with("/dev/shm/")
                    || std::env::var("TMPDIR").map_or(false, |t| path.starts_with(&t))
                {
                    let _ = std::fs::remove_file(&path);
                }
                Ok(data)
            }
            Self::SharedMem { .. } => Err("shared memory not supported in this build".into()),
        }
    }
}

fn read_file_data(path: &str, offset: Option<u32>, size: Option<u32>) -> Result<Vec<u8>, String> {
    use std::io::{Read, Seek};
    let mut f = std::fs::File::open(path).map_err(|e| format!("open {path}: {e}"))?;
    if let Some(o) = offset {
        f.seek(std::io::SeekFrom::Start(o as u64))
            .map_err(|e| format!("seek {path}: {e}"))?;
    }
    if let Some(len) = size {
        let mut buf = vec![0u8; len as usize];
        f.read_exact(&mut buf).map_err(|e| format!("read {path}: {e}"))?;
        Ok(buf)
    } else {
        let mut buf = vec![];
        f.read_to_end(&mut buf).map_err(|e| format!("read {path}: {e}"))?;
        Ok(buf)
    }
}

fn decode_base64(data: &[u8]) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| format!("base64 decode: {e}"))
}

#[allow(dead_code)]
fn encode_base64(data: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(data)
}

// ── format / compression ───────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyImageFormat {
    Rgb,   // f=24
    Rgba,  // f=32
    Png,   // f=100
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyImageCompression {
    None,
    Deflate, // o=z
}

// ── transmit struct ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KittyImageTransmit {
    pub format: Option<KittyImageFormat>,
    pub data: KittyImageData,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub image_id: Option<u32>,
    pub image_number: Option<u32>,
    pub compression: KittyImageCompression,
    pub more_data_follows: bool,
}

// ── placement struct ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KittyImagePlacement {
    pub x: Option<u32>,
    pub y: Option<u32>,
    pub w: Option<u32>,
    pub h: Option<u32>,
    pub x_offset: Option<u32>,
    pub y_offset: Option<u32>,
    pub columns: Option<u32>,
    pub rows: Option<u32>,
    pub do_not_move_cursor: bool,
    pub placement_id: Option<u32>,
    pub z_index: Option<i32>,
}

// ── delete ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KittyImageDelete {
    All { delete: bool },
    ByImageId { image_id: u32, placement_id: Option<u32>, delete: bool },
    ByImageNumber { image_number: u32, placement_id: Option<u32>, delete: bool },
    AtCursorPosition { delete: bool },
    DeleteAt { x: u32, y: u32, delete: bool },
    DeleteColumn { x: u32, delete: bool },
    DeleteRow { y: u32, delete: bool },
    DeleteZ { z: i32, delete: bool },
}

// ── verbosity ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KittyImageVerbosity {
    #[default]
    Verbose,
    OnlyErrors,
    Quiet,
}

// ── frame (stub) ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyFrameCompositionMode {
    AlphaBlending,
    Overwrite,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KittyImageFrame {
    pub x: Option<u32>,
    pub y: Option<u32>,
    pub duration_ms: Option<u32>,
    pub frame_number: Option<u32>,
    pub base_frame: Option<u32>,
    pub composition_mode: KittyFrameCompositionMode,
    pub background_pixel: Option<u32>,
}

// ── top-level command ──────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KittyImageFrameCompose {
    pub image_id: Option<u32>,
    pub image_number: Option<u32>,
    pub target_frame: Option<u32>,
    pub source_frame: Option<u32>,
    pub x: Option<u32>,
    pub y: Option<u32>,
    pub w: Option<u32>,
    pub h: Option<u32>,
    pub src_x: Option<u32>,
    pub src_y: Option<u32>,
    pub composition_mode: KittyFrameCompositionMode,
}

impl KittyImageFrameCompose {
    fn from_keys(keys: &BTreeMap<&str, &str>) -> Option<Self> {
        Some(Self {
            image_id: geti(keys, "i"),
            image_number: geti(keys, "I"),
            x: geti(keys, "x"),
            y: geti(keys, "y"),
            src_x: geti(keys, "X"),
            src_y: geti(keys, "Y"),
            w: geti(keys, "w"),
            h: geti(keys, "h"),
            target_frame: match geti(keys, "c") {
                None | Some(0) => None,
                n => n,
            },
            source_frame: match geti(keys, "r") {
                None | Some(0) => None,
                n => n,
            },
            composition_mode: match get(keys, "C") {
                None | Some("0") => KittyFrameCompositionMode::AlphaBlending,
                Some("1") => KittyFrameCompositionMode::Overwrite,
                _ => return None,
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KittyImage {
    TransmitData {
        transmit: KittyImageTransmit,
        verbosity: KittyImageVerbosity,
    },
    TransmitDataAndDisplay {
        transmit: KittyImageTransmit,
        placement: KittyImagePlacement,
        verbosity: KittyImageVerbosity,
    },
    Display {
        image_id: Option<u32>,
        image_number: Option<u32>,
        placement: KittyImagePlacement,
        verbosity: KittyImageVerbosity,
    },
    Delete {
        what: KittyImageDelete,
        verbosity: KittyImageVerbosity,
    },
    Query {
        transmit: KittyImageTransmit,
    },
    TransmitFrame {
        transmit: KittyImageTransmit,
        frame: KittyImageFrame,
        verbosity: KittyImageVerbosity,
    },
    ComposeFrame {
        frame: KittyImageFrameCompose,
        verbosity: KittyImageVerbosity,
    },
}

impl KittyImage {
    /// Parse an APC payload (bytes after `\x1b_G` and before `\x1b\\`).
    pub fn parse_apc(data: &[u8]) -> Option<Self> {
        if data.is_empty() || data[0] != b'G' {
            return None;
        }
        let mut iter = data[1..].splitn(2, |&d| d == b';');
        let keys_raw = iter.next()?;
        let payload = iter.next().unwrap_or(b"");

        let key_str = std::str::from_utf8(keys_raw).ok()?;
        let mut keys: BTreeMap<&str, &str> = BTreeMap::new();
        for kv in key_str.split(',') {
            let mut parts = kv.splitn(2, '=');
            let k = parts.next()?;
            let v = parts.next()?;
            keys.insert(k, v);
        }

        let action = get(&keys, "a").unwrap_or("t");
        let verbosity = KittyImageVerbosity::from_keys(&keys)?;

        match action {
            "t" => Some(Self::TransmitData {
                transmit: KittyImageTransmit::from_keys(&keys, payload)?,
                verbosity,
            }),
            "q" => Some(Self::Query {
                transmit: KittyImageTransmit::from_keys(&keys, payload)?,
            }),
            "T" => Some(Self::TransmitDataAndDisplay {
                transmit: KittyImageTransmit::from_keys(&keys, payload)?,
                placement: KittyImagePlacement::from_keys(&keys)?,
                verbosity,
            }),
            "p" => Some(Self::Display {
                placement: KittyImagePlacement::from_keys(&keys)?,
                image_id: geti(&keys, "i"),
                image_number: geti(&keys, "I"),
                verbosity,
            }),
            "d" => Some(Self::Delete {
                what: KittyImageDelete::from_keys(&keys)?,
                verbosity,
            }),
            "f" => Some(Self::TransmitFrame {
                transmit: KittyImageTransmit::from_keys(&keys, payload)?,
                frame: KittyImageFrame::from_keys(&keys)?,
                verbosity,
            }),
            "c" => Some(Self::ComposeFrame {
                frame: KittyImageFrameCompose::from_keys(&keys)?,
                verbosity,
            }),
            _ => None,
        }
    }
}

// ── `from_keys` implementations ────────────────────────────────────────

impl KittyImageVerbosity {
    fn from_keys(keys: &BTreeMap<&str, &str>) -> Option<Self> {
        match get(keys, "q") {
            None | Some("0") => Some(Self::Verbose),
            Some("1") => Some(Self::OnlyErrors),
            Some("2") => Some(Self::Quiet),
            _ => None,
        }
    }
}

impl KittyImageFormat {
    fn from_keys(keys: &BTreeMap<&str, &str>) -> Option<Option<Self>> {
        match get(keys, "f") {
            None => Some(None),
            Some("24") => Some(Some(Self::Rgb)),
            Some("32") => Some(Some(Self::Rgba)),
            Some("100") => Some(Some(Self::Png)),
            _ => None,
        }
    }
}

impl KittyImageCompression {
    fn from_keys(keys: &BTreeMap<&str, &str>) -> Option<Self> {
        match get(keys, "o") {
            None => Some(Self::None),
            Some("z") => Some(Self::Deflate),
            _ => None,
        }
    }
}

impl KittyImageTransmit {
    fn from_keys(keys: &BTreeMap<&str, &str>, payload: &[u8]) -> Option<Self> {
        Some(Self {
            format: KittyImageFormat::from_keys(keys)?,
            data: KittyImageData::from_keys(keys, payload)?,
            compression: KittyImageCompression::from_keys(keys)?,
            width: geti(keys, "s"),
            height: geti(keys, "v"),
            image_id: geti(keys, "i"),
            image_number: geti(keys, "I"),
            more_data_follows: match get(keys, "m") {
                None | Some("0") => false,
                Some("1") => true,
                _ => return None,
            },
        })
    }
}

impl KittyImagePlacement {
    fn from_keys(keys: &BTreeMap<&str, &str>) -> Option<Self> {
        Some(Self {
            x: geti(keys, "x"),
            y: geti(keys, "y"),
            w: geti(keys, "w"),
            h: geti(keys, "h"),
            x_offset: geti(keys, "X"),
            y_offset: geti(keys, "Y"),
            columns: geti(keys, "c"),
            rows: geti(keys, "r"),
            placement_id: geti(keys, "p"),
            do_not_move_cursor: match get(keys, "C") {
                None | Some("0") => false,
                Some("1") => true,
                _ => return None,
            },
            z_index: geti(keys, "z"),
        })
    }
}

impl KittyImageDelete {
    fn from_keys(keys: &BTreeMap<&str, &str>) -> Option<Self> {
        let d = get(keys, "d").unwrap_or("a");
        let d = d.chars().next()?;
        let delete = d.is_ascii_uppercase();
        match d.to_ascii_lowercase() {
            'a' => Some(Self::All { delete }),
            'i' => Some(Self::ByImageId {
                image_id: geti(keys, "i")?,
                placement_id: geti(keys, "p"),
                delete,
            }),
            'n' => Some(Self::ByImageNumber {
                image_number: geti(keys, "I")?,
                placement_id: geti(keys, "p"),
                delete,
            }),
            'c' => Some(Self::AtCursorPosition { delete }),
            'p' => Some(Self::DeleteAt {
                x: geti(keys, "x")?,
                y: geti(keys, "y")?,
                delete,
            }),
            'x' => Some(Self::DeleteColumn {
                x: geti(keys, "x")?,
                delete,
            }),
            'y' => Some(Self::DeleteRow {
                y: geti(keys, "y")?,
                delete,
            }),
            'z' => Some(Self::DeleteZ {
                z: geti(keys, "z")?,
                delete,
            }),
            _ => None,
        }
    }
}

impl KittyImageFrame {
    fn from_keys(keys: &BTreeMap<&str, &str>) -> Option<Self> {
        Some(Self {
            x: geti(keys, "x"),
            y: geti(keys, "y"),
            duration_ms: match geti(keys, "Z") {
                None | Some(0) => None,
                n => n,
            },
            frame_number: match geti(keys, "r") {
                None | Some(0) => None,
                n => n,
            },
            base_frame: match geti(keys, "c") {
                None | Some(0) => None,
                n => n,
            },
            composition_mode: match get(keys, "X") {
                None | Some("0") => KittyFrameCompositionMode::AlphaBlending,
                Some("1") => KittyFrameCompositionMode::Overwrite,
                _ => return None,
            },
            background_pixel: geti(keys, "Y"),
        })
    }
}

// ── response encoding ───────────────────────────────────────────────────

/// Build a Kitty protocol response string.
pub fn kitty_response(image_id: Option<u32>, image_number: Option<u32>, message: &str) -> String {
    let mut s = "\x1b_G".to_string();
    let mut first = true;
    let mut push = |k: &str, v: &str| {
        if !first { s.push(','); }
        s.push_str(k);
        s.push('=');
        s.push_str(v);
        first = false;
    };
    if let Some(id) = image_id {
        push("i", &id.to_string());
    }
    if let Some(no) = image_number {
        push("I", &no.to_string());
    }
    s.push(';');
    s.push_str(message);
    s.push_str("\x1b\\");
    s
}

// ── processing ─────────────────────────────────────────────────────────

/// Decode image data from a Kitty transmit command.
pub fn decode_image_data(
    transmit: KittyImageTransmit,
    image_cache: &mut ImageCache,
) -> Result<u32, String> {
    let raw = transmit.data.load_data()?;
    let raw = match transmit.compression {
        KittyImageCompression::None => raw,
        KittyImageCompression::Deflate => {
            miniz_oxide::inflate::decompress_to_vec_zlib(&raw)
                .map_err(|e| format!("deflate decompress: {e:?}"))?
        }
    };

    let img = match transmit.format {
        None | Some(KittyImageFormat::Rgba) | Some(KittyImageFormat::Rgb) => {
            let (w, h) = match (transmit.width, transmit.height) {
                (Some(w), Some(h)) => (w, h),
                _ => return Err("missing width/height for kitty rgb/rgba data".into()),
            };
            let rgba = match transmit.format {
                Some(KittyImageFormat::Rgb) => {
                    let rgb = RgbImage::from_vec(w, h, raw)
                        .ok_or_else(|| "invalid rgb data".to_string())?;
                    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
                    for pixel in rgb.pixels() {
                        rgba.extend_from_slice(&pixel.0);
                        rgba.push(255);
                    }
                    rgba
                }
                _ => raw,
            };
            if rgba.len() as u32 != w * h * 4 {
                return Err("rgba data length mismatch".into());
            }
            ImageDataType::new_rgba8(rgba, w, h)
        }
        Some(KittyImageFormat::Png) => {
            let decoded = load_from_memory(&raw).map_err(|e| format!("png decode: {e}"))?;
            let (w, h) = decoded.dimensions();
            let rgba = decoded.into_rgba8().into_vec();
            ImageDataType::new_rgba8(rgba, w, h)
        }
    };

    let data = Arc::new(ImageData::new(img));
    let image_id = image_cache.assign_id(transmit.image_id, transmit.image_number);
    image_cache.insert(image_id, data);
    Ok(image_id)
}

// ── frame transmit (a=f) ───────────────────────────────────────────────

/// Handle a "TransmitFrame" (a=f) command: edit an existing image's frame
/// or create a new animation frame.
pub fn decode_image_frame(
    transmit: KittyImageTransmit,
    frame: KittyImageFrame,
    image_cache: &mut ImageCache,
) -> Result<(), String> {
    let image_id = match (transmit.image_id, transmit.image_number) {
        (Some(id), _) => id,
        (None, Some(no)) => {
            // Look up the image_number mapping.
            // We assign via `image_cache.assign_id` which tracks number_to_id.
            let id = image_cache.assign_id(None, Some(no));
            id
        }
        (None, None) => {
            // Use image id 0 (anonymous).
            0
        }
    };

    let raw = transmit.data.load_data()?;
    let raw = match transmit.compression {
        KittyImageCompression::None => raw,
        KittyImageCompression::Deflate => {
            miniz_oxide::inflate::decompress_to_vec_zlib(&raw)
                .map_err(|e| format!("deflate decompress: {e:?}"))?
        }
    };

    let (frame_data, frame_w, frame_h) = match transmit.format {
        None | Some(KittyImageFormat::Rgba) => {
            let w = transmit.width.ok_or("missing width")?;
            let h = transmit.height.ok_or("missing height")?;
            if raw.len() as u32 != w * h * 4 {
                return Err("rgba data length mismatch".into());
            }
            (raw, w, h)
        }
        Some(KittyImageFormat::Rgb) => {
            let w = transmit.width.ok_or("missing width")?;
            let h = transmit.height.ok_or("missing height")?;
            let rgb = RgbImage::from_vec(w, h, raw)
                .ok_or("invalid rgb data")?;
            let mut rgba = Vec::with_capacity((w * h * 4) as usize);
            for pixel in rgb.pixels() {
                rgba.extend_from_slice(&pixel.0);
                rgba.push(255);
            }
            (rgba, w, h)
        }
        Some(KittyImageFormat::Png) => {
            let decoded = load_from_memory(&raw).map_err(|e| format!("png decode: {e}"))?;
            let (w, h) = decoded.dimensions();
            let rgba = decoded.into_rgba8().into_vec();
            (rgba, w, h)
        }
    };

    let x = frame.x.unwrap_or(0);
    let y = frame.y.unwrap_or(0);
    let composition_mode = frame.composition_mode; // 0=overlay, 1=replace
    let background_pixel = frame.background_pixel.unwrap_or(0);
    let bg = image::Rgba([
        ((background_pixel >> 24) & 0xff) as u8,
        ((background_pixel >> 16) & 0xff) as u8,
        ((background_pixel >> 8) & 0xff) as u8,
        (background_pixel & 0xff) as u8,
    ]);

    let existing = image_cache.get(image_id).ok_or("image_id not found for frame transmit")?;
    let mut guard = existing.data();

    match &mut *guard {
        ImageDataType::Rgba8 { data, width, height, hash } => {
            let frame_no = frame.frame_number.unwrap_or(1);
            if frame_no == 1 {
                // Edit in place: blit the new data onto the existing frame.
                let mut dest = image::RgbaImage::from_raw(*width, *height, data.clone())
                    .ok_or("invalid existing rgba data")?;
                let src = image::RgbaImage::from_raw(frame_w, frame_h, frame_data)
                    .ok_or("invalid frame data")?;
                apply_blit(&mut dest, &src, x, y, composition_mode);
                *data = dest.into_vec();
                *hash = ImageDataType::new_rgba8(data.clone(), *width, *height).hash();
            } else {
                // Create a second frame: convert to AnimRgba8.
                let bg_duration = std::time::Duration::from_millis(frame.duration_ms.unwrap_or(40) as u64);
                let base = if frame.base_frame.unwrap_or(0) == 1 {
                    data.clone()
                } else {
                    vec![bg.0[0], bg.0[1], bg.0[2], bg.0[3]].repeat((*width * *height) as usize)
                };
                let mut new_frame = image::RgbaImage::from_raw(*width, *height, base)
                    .ok_or("invalid base frame")?;
                let src = image::RgbaImage::from_raw(frame_w, frame_h, frame_data)
                    .ok_or("invalid frame data")?;
                apply_blit(&mut new_frame, &src, x, y, composition_mode);

                let old_data = std::mem::take(data);
                *guard = ImageDataType::AnimRgba8 {
                    width: *width,
                    height: *height,
                    frames: vec![old_data, new_frame.into_vec()],
                    durations: vec![std::time::Duration::from_secs(0), bg_duration],
                    hashes: Vec::new(),
                };
                // Recompute hashes.
                if let ImageDataType::AnimRgba8 { ref frames, ref mut hashes, .. } = *guard {
                    *hashes = frames.iter().map(|f| compute_hash_trait(f)).collect();
                }
            }
        }
        ImageDataType::AnimRgba8 { width, height, frames, durations, hashes } => {
            let frame_no = frame.frame_number.unwrap_or(frames.len() as u32 + 1);
            if frame_no <= frames.len() as u32 {
                // Edit existing frame in place.
                let mut dest = image::RgbaImage::from_raw(
                    *width, *height, frames[frame_no as usize - 1].clone(),
                ).ok_or("invalid anim frame data")?;
                let src = image::RgbaImage::from_raw(frame_w, frame_h, frame_data)
                    .ok_or("invalid frame data")?;
                apply_blit(&mut dest, &src, x, y, composition_mode);
                frames[frame_no as usize - 1] = dest.into_vec();
                hashes[frame_no as usize - 1] = compute_hash_trait(&frames[frame_no as usize - 1]);
            } else {
                // Append a new frame.
                let bg_duration = std::time::Duration::from_millis(frame.duration_ms.unwrap_or(40) as u64);
                let base = match frame.base_frame {
                    Some(n) if n > 0 && n as usize <= frames.len() => {
                        frames[n as usize - 1].clone()
                    }
                    _ => {
                        vec![bg.0[0], bg.0[1], bg.0[2], bg.0[3]].repeat((*width * *height) as usize)
                    }
                };
                let mut new_frame = image::RgbaImage::from_raw(*width, *height, base)
                    .ok_or("invalid base frame")?;
                let src = image::RgbaImage::from_raw(frame_w, frame_h, frame_data)
                    .ok_or("invalid frame data")?;
                apply_blit(&mut new_frame, &src, x, y, composition_mode);
                frames.push(new_frame.into_vec());
                durations.push(bg_duration);
                hashes.push(compute_hash_trait(frames.last().unwrap()));
            }
        }
    }
    drop(guard);

    // Recompute the overall hash.
    let _new_hash = {
        let g = existing.data();
        g.hash()
    };
    // We can't directly modify `hash` on ImageData because it's computed.
    // For now, the hash is recomputed on access.
    Ok(())
}

// ── compose frame (a=c) ────────────────────────────────────────────────

/// Handle a "ComposeFrame" (a=c) command: copy a source region between
/// frames of an animated image.
pub fn handle_compose_frame(
    frame: KittyImageFrameCompose,
    image_cache: &mut ImageCache,
) -> Result<(), String> {
    let image_id = match frame.image_id {
        Some(id) => id,
        None => {
            let no = frame.image_number.ok_or("no image_id or image_number")?;
            // Assign to look up or create mapping.
            image_cache.assign_id(None, Some(no))
        }
    };

    let existing = image_cache.get(image_id).ok_or("image_id not found for compose")?;
    let mut guard = existing.data();
    let src_frame_idx = frame.source_frame.ok_or("missing source_frame")? as usize;
    let dst_frame_idx = match frame.target_frame {
        Some(n) => n as usize,
        None => return Err("missing target_frame".into()),
    };

    match &mut *guard {
        ImageDataType::Rgba8 { data, width, height, .. } => {
            if src_frame_idx != 1 || dst_frame_idx != 1 {
                return Err("compose: only frame 1 available".into());
            }
            let (src_data, src_w, src_h) =
                clip_view(*width, *height, data, frame.src_x, frame.src_y, frame.w, frame.h)?;
            let mut dest = image::RgbaImage::from_raw(*width, *height, data.clone())
                .ok_or("invalid rgba data")?;
            let src_img = image::RgbaImage::from_raw(src_w, src_h, src_data)
                .ok_or("invalid clip")?;
            apply_blit(&mut dest, &src_img, frame.x.unwrap_or(0), frame.y.unwrap_or(0), frame.composition_mode);
            *data = dest.into_vec();
        }
        ImageDataType::AnimRgba8 { width, height, frames, .. } => {
            let src_ok = src_frame_idx > 0 && src_frame_idx <= frames.len();
            let dst_ok = dst_frame_idx > 0 && dst_frame_idx <= frames.len();
            if !src_ok || !dst_ok {
                return Err("compose: frame index out of range".into());
            }
            let (src_data, src_w, src_h) = clip_view(*width, *height, &frames[src_frame_idx - 1],
                frame.src_x, frame.src_y, frame.w, frame.h)?;
            let mut dest = image::RgbaImage::from_raw(*width, *height, frames[dst_frame_idx - 1].clone())
                .ok_or("invalid anim frame")?;
            let src_img = image::RgbaImage::from_raw(src_w, src_h, src_data)
                .ok_or("invalid clip")?;
            apply_blit(&mut dest, &src_img, frame.x.unwrap_or(0), frame.y.unwrap_or(0), frame.composition_mode);
            frames[dst_frame_idx - 1] = dest.into_vec();
        }
    }
    Ok(())
}

// ── helpers ────────────────────────────────────────────────────────────

fn compute_hash_trait(data: &[u8]) -> [u8; 32] {
    use std::hash::Hasher as _;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write(data);
    let h = hasher.finish();
    let mut hash = [0u8; 32];
    hash[..8].copy_from_slice(&h.to_le_bytes());
    hash
}

/// Copy a sub-rectangle from image data. Returns `(rgba_data, width, height)`.
fn clip_view(
    width: u32,
    height: u32,
    data: &[u8],
    src_x: Option<u32>,
    src_y: Option<u32>,
    view_w: Option<u32>,
    view_h: Option<u32>,
) -> Result<(Vec<u8>, u32, u32), String> {
    let src_x = src_x.unwrap_or(0);
    let src_y = src_y.unwrap_or(0);
    let vw = view_w.unwrap_or(width.saturating_sub(src_x)).min(width.saturating_sub(src_x));
    let vh = view_h.unwrap_or(height.saturating_sub(src_y)).min(height.saturating_sub(src_y));
    let mut out = vec![0u8; (vw * vh * 4) as usize];
    for y in 0..vh {
        for x in 0..vw {
            let si = (((src_y + y) * width + (src_x + x)) * 4) as usize;
            let di = ((y * vw + x) * 4) as usize;
            if si + 3 < data.len() && di + 3 < out.len() {
                out[di..di + 4].copy_from_slice(&data[si..si + 4]);
            }
        }
    }
    Ok((out, vw, vh))
}

/// Apply a blit operation (overwrite or alpha-blend).
fn apply_blit(
    dest: &mut image::RgbaImage,
    src: &image::RgbaImage,
    x: u32,
    y: u32,
    mode: KittyFrameCompositionMode,
) {
    match mode {
        KittyFrameCompositionMode::AlphaBlending => {
            image::imageops::overlay(dest, src, x.into(), y.into());
        }
        KittyFrameCompositionMode::Overwrite => {
            image::imageops::replace(dest, src, x.into(), y.into());
        }
    }
}

// ── chunk accumulation ─────────────────────────────────────────────────

/// Accumulator for multi-chunk image transmissions.
#[derive(Debug, Default)]
pub struct KittyAccumulator {
    chunks: Vec<KittyImageData>,
    transmit: Option<KittyImageTransmit>,
    placement: Option<KittyImagePlacement>,
    verbosity: KittyImageVerbosity,
}

impl KittyAccumulator {
    /// Feed a new chunk.  Returns `Ok(Some(assembled_command))` when
    /// the final chunk (`m=0` or `m` absent) arrives.
    pub fn feed(&mut self, img: KittyImage) -> Result<Option<KittyImage>, String> {
        let more = match &img {
            KittyImage::TransmitData { transmit, .. }
            | KittyImage::TransmitDataAndDisplay { transmit, .. } => transmit.more_data_follows,
            _ => return Ok(Some(img)),
        };
        let is_first = self.transmit.is_none();

        if is_first {
            let (tx, pl, verb) = match img {
                KittyImage::TransmitData { transmit, verbosity } => {
                    (transmit, None, verbosity)
                }
                KittyImage::TransmitDataAndDisplay { transmit, placement, verbosity } => {
                    (transmit, Some(placement), verbosity)
                }
                _ => unreachable!(),
            };
            self.transmit = Some(KittyImageTransmit {
                format: tx.format,
                data: KittyImageData::DirectBin(vec![]),
                width: tx.width,
                height: tx.height,
                image_id: tx.image_id,
                image_number: tx.image_number,
                compression: tx.compression,
                more_data_follows: false,
            });
            self.placement = pl;
            self.verbosity = verb;
            self.chunks.push(tx.data);
        } else {
            match img {
                KittyImage::TransmitData { transmit, .. }
                | KittyImage::TransmitDataAndDisplay { transmit, .. } => {
                    self.chunks.push(transmit.data);
                }
                _ => unreachable!(),
            }
        }

                if !more {
                    let mut all_data = vec![];
                    for chunk in self.chunks.drain(..) {
                        let bytes = chunk.load_data()?;
                        all_data.extend(bytes);
                    }
                    if let Some(tx) = self.transmit.take() {
                        let assembled = KittyImageTransmit {
                            data: KittyImageData::DirectBin(all_data),
                            ..tx
                        };
                        let placement = self.placement.take();
                        return Ok(Some(match placement {
                            Some(pl) => KittyImage::TransmitDataAndDisplay {
                                transmit: assembled,
                                placement: pl,
                                verbosity: self.verbosity,
                            },
                            None => KittyImage::TransmitData {
                                transmit: assembled,
                                verbosity: self.verbosity,
                            },
                        }));
                    }
                }
                Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_transmit() {
        let img = KittyImage::parse_apc(b"Gf=24,s=10,v=20;aGVsbG8=").unwrap();
        assert!(matches!(img, KittyImage::TransmitData { .. }));
    }

    #[test]
    fn test_parse_delete() {
        let img = KittyImage::parse_apc(b"Ga=d,q=2").unwrap();
        assert!(matches!(img, KittyImage::Delete { .. }));
    }

    #[test]
    fn test_parse_display() {
        let img = KittyImage::parse_apc(b"Ga=p,i=1,c=2,r=3").unwrap();
        assert!(matches!(img, KittyImage::Display { .. }));
    }
}
