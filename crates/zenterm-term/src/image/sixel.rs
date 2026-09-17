//! Sixel graphics parser (`DCS q … ST`).
//!
//! Implements the sixel graphics format defined in
//! <https://vt100.net/docs/vt3xx-gp/chapter14.html>
//!
//! Ported from wezterm's `wezterm-escape-parser/src/parser/sixel.rs`.

use std::sync::Arc;

use image::RgbaImage;

use zenterm_core::image::{ImageData, ImageDataType};

use super::kitty::{MAX_IMAGE_BYTES, MAX_IMAGE_PIXELS};

// ── sixel data types ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum SixelData {
    Data(u8),
    Repeat {
        repeat_count: u32,
        data: u8,
    },
    DefineColorMapRGB {
        color_number: u16,
        r: u8,
        g: u8,
        b: u8,
    },
    DefineColorMapHSL {
        color_number: u16,
        hue_angle: u16,
        lightness: u8,
        saturation: u8,
    },
    SelectColorMapEntry(u16),
    CarriageReturn,
    NewLine,
}

#[derive(Debug, Clone)]
pub struct Sixel {
    pub pan: i64,
    pub pad: i64,
    pub pixel_width: Option<u32>,
    pub pixel_height: Option<u32>,
    pub background_is_transparent: bool,
    pub data: Vec<SixelData>,
    truncated: bool,
    invalid_dimensions: bool,
}

// ── builder ────────────────────────────────────────────────────────────

const MAX_PARAMS: usize = 5;
const MAX_SIXEL_DATA_ITEMS: usize = 1_000_000;

pub struct SixelBuilder {
    pub sixel: Sixel,
    params: [i64; MAX_PARAMS],
    param_no: usize,
    current_command: u8,
}

impl SixelBuilder {
    pub fn new(params: &[i64]) -> Self {
        let pan = match params.first().copied().unwrap_or(0) {
            7..=9 => 1,
            0 | 1 | 5 | 6 => 2,
            3 | 4 => 3,
            2 => 5,
            _ => 2,
        };
        let background_is_transparent = params.get(1).copied().unwrap_or(0) == 1;

        Self {
            sixel: Sixel {
                pan,
                pad: 1,
                pixel_width: None,
                pixel_height: None,
                background_is_transparent,
                data: vec![],
                truncated: false,
                invalid_dimensions: false,
            },
            param_no: 0,
            params: [-1; MAX_PARAMS],
            current_command: 0,
        }
    }

    pub fn push(&mut self, data: u8) {
        match data {
            b'$' => {
                self.finish_command();
                self.push_data(SixelData::CarriageReturn);
            }
            b'-' => {
                self.finish_command();
                self.push_data(SixelData::NewLine);
            }
            0x3f..=0x7e if self.current_command == b'!' => {
                let repeat_count = match u32::try_from(self.params[0]) {
                    Ok(count) => count,
                    Err(_) => {
                        self.sixel.invalid_dimensions = true;
                        0
                    }
                };
                self.push_data(SixelData::Repeat {
                    repeat_count,
                    data: data - 0x3f,
                });
                self.finish_command();
            }
            0x3f..=0x7e => {
                self.finish_command();
                self.push_data(SixelData::Data(data - 0x3f));
            }
            b'#' | b'!' | b'"' => {
                self.finish_command();
                self.current_command = data;
            }
            b'0'..=b'9' if self.current_command != 0 => {
                let pos = self.param_no;
                if pos >= MAX_PARAMS {
                    return;
                }
                if self.params[pos] == -1 {
                    self.params[pos] = 0;
                }
                self.params[pos] = self.params[pos]
                    .saturating_mul(10)
                    .saturating_add((data - b'0') as i64);
            }
            b';' if self.current_command != 0 => {
                if self.param_no < MAX_PARAMS {
                    self.param_no += 1;
                }
            }
            _ => {
                self.finish_command();
            }
        }
    }

    fn push_data(&mut self, data: SixelData) {
        if self.sixel.data.len() < MAX_SIXEL_DATA_ITEMS {
            self.sixel.data.push(data);
        } else {
            self.sixel.truncated = true;
        }
    }

    fn finish_command(&mut self) {
        match self.current_command {
            b'#' if self.param_no >= 4 => {
                let color_number = self.params[0] as u16;
                let system = self.params[1] as u16;
                let a = self.params[2] as u16;
                let b = self.params[3] as u8;
                let c = self.params[4] as u8;
                if system == 1 {
                    self.push_data(SixelData::DefineColorMapHSL {
                        color_number,
                        hue_angle: a,
                        lightness: b,
                        saturation: c,
                    });
                } else {
                    let r = (a as f32 * 255.0 / 100.0) as u8;
                    let g = (b as f32 * 255.0 / 100.0) as u8;
                    let b = (c as f32 * 255.0 / 100.0) as u8;
                    self.push_data(SixelData::DefineColorMapRGB {
                        color_number,
                        r,
                        g,
                        b,
                    });
                }
            }
            b'#' => {
                let color_number = self.params[0] as u16;
                self.push_data(SixelData::SelectColorMapEntry(color_number));
            }
            b'"' => {
                let pan = if self.params[0] == -1 {
                    2
                } else {
                    self.params[0]
                };
                let pad = if self.params[1] == -1 {
                    1
                } else {
                    self.params[1]
                };
                let pixel_width = self.params[2];
                let pixel_height = self.params[3];
                self.sixel.pan = pan;
                self.sixel.pad = pad;
                if self.param_no >= 3 && pixel_width > 0 && pixel_height > 0 {
                    match (u32::try_from(pixel_width), u32::try_from(pixel_height)) {
                        (Ok(width), Ok(height)) => {
                            self.sixel.pixel_width = Some(width);
                            self.sixel.pixel_height = Some(height);
                        }
                        _ => self.sixel.invalid_dimensions = true,
                    }
                }
            }
            _ => {}
        }
        self.param_no = 0;
        self.params = [-1; MAX_PARAMS];
        self.current_command = 0;
    }

    pub fn finish(&mut self) {
        self.finish_command();
    }
}

// ── render sixel to RGBA ──────────────────────────────────────────────

/// Convert parsed sixel data into an [`ImageData`] (RGBA).
pub fn render_sixel(sixel: &Sixel) -> Result<Arc<ImageData>, String> {
    let (width, height) = sixel_dimensions(sixel)?;
    if width == 0 || height == 0 {
        return Err("sixel has zero dimensions".into());
    }

    let mut color_map: std::collections::HashMap<u16, (u8, u8, u8)> = {
        let mut m = std::collections::HashMap::new();
        m.insert(0, (0, 0, 0)); // default color 0 = black
        m
    };

    let mut image = if sixel.background_is_transparent {
        RgbaImage::new(width, height)
    } else {
        let bg = color_map.get(&0).copied().unwrap_or((0, 0, 0));
        RgbaImage::from_pixel(width, height, [bg.0, bg.1, bg.2, 255].into())
    };

    let mut x = 0u32;
    let mut y = 0u32;
    let mut fg = (0, 255, 0); // default green

    for d in &sixel.data {
        match d {
            SixelData::Data(d) => {
                emit_sixel_bitplane(&mut image, *d, &fg, x, y, width, height);
                x = x.saturating_add(1);
            }
            SixelData::Repeat { repeat_count, data } => {
                let count = (*repeat_count).min(width.saturating_sub(x));
                for _ in 0..count {
                    emit_sixel_bitplane(&mut image, *data, &fg, x, y, width, height);
                    x = x.saturating_add(1);
                }
            }
            SixelData::CarriageReturn => x = 0,
            SixelData::NewLine => {
                x = 0;
                y = y.saturating_add(6);
            }
            SixelData::DefineColorMapRGB {
                color_number,
                r,
                g,
                b,
            } => {
                color_map.insert(*color_number, (*r, *g, *b));
            }
            SixelData::DefineColorMapHSL {
                color_number,
                hue_angle,
                saturation,
                lightness,
            } => {
                let angle = (*hue_angle as f64) - 120.0;
                let angle = if angle < 0. { 360.0 + angle } else { angle };
                let c = csscolorparser::Color::from_hsla(
                    angle,
                    *saturation as f64 / 100.0,
                    *lightness as f64 / 100.0,
                    1.0,
                );
                let [r, g, b, _] = c.to_rgba8();
                color_map.insert(*color_number, (r, g, b));
            }
            SixelData::SelectColorMapEntry(n) => {
                fg = color_map.get(n).copied().unwrap_or((255, 255, 255));
            }
        }
    }

    let rgba = image.into_vec();
    let data_type = ImageDataType::new_rgba8(rgba, width, height);
    Ok(Arc::new(ImageData::new(data_type)))
}

fn sixel_dimensions(sixel: &Sixel) -> Result<(u32, u32), String> {
    if sixel.truncated {
        return Err("sixel command stream exceeds item limit".into());
    }
    if sixel.invalid_dimensions {
        return Err("sixel dimensions are out of range".into());
    }
    if let (Some(w), Some(h)) = (sixel.pixel_width, sixel.pixel_height) {
        validate_sixel_dimensions(w, h)?;
        return Ok((w, h));
    }
    // Compute dimensions from the data stream.
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut x = 0u32;
    let mut y = 0u32;
    for d in &sixel.data {
        match d {
            SixelData::Data(_) | SixelData::Repeat { .. } => {
                let count = match d {
                    SixelData::Data(_) => 1,
                    SixelData::Repeat { repeat_count, .. } => *repeat_count,
                    _ => unreachable!(),
                };
                x = x
                    .checked_add(count)
                    .ok_or_else(|| "sixel width overflows".to_string())?;
                max_x = max_x.max(x);
                max_y = max_y.max(
                    y.checked_add(6)
                        .ok_or_else(|| "sixel height overflows".to_string())?,
                );
            }
            SixelData::CarriageReturn => x = 0,
            SixelData::NewLine => {
                x = 0;
                y = y
                    .checked_add(6)
                    .ok_or_else(|| "sixel height overflows".to_string())?;
            }
            _ => {}
        }
    }
    validate_sixel_dimensions(max_x, max_y)?;
    Ok((max_x, max_y))
}

fn validate_sixel_dimensions(width: u32, height: u32) -> Result<(), String> {
    let pixels = (width as u64)
        .checked_mul(height as u64)
        .ok_or_else(|| format!("sixel dimensions overflow: {width}x{height}"))?;
    if pixels > MAX_IMAGE_PIXELS {
        return Err(format!(
            "sixel too large: {width}x{height} ({pixels} pixels) exceeds limit of {MAX_IMAGE_PIXELS}"
        ));
    }
    let bytes = pixels
        .checked_mul(4)
        .ok_or_else(|| format!("sixel buffer size overflows: {width}x{height}"))?;
    if bytes > MAX_IMAGE_BYTES as u64 {
        return Err(format!("sixel buffer exceeds {MAX_IMAGE_BYTES} byte limit"));
    }
    Ok(())
}

fn emit_sixel_bitplane(
    image: &mut RgbaImage,
    d: u8,
    fg: &(u8, u8, u8),
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) {
    if x >= width {
        return;
    }
    for bitno in 0..6 {
        let py = y.saturating_add(bitno);
        if py >= height {
            break;
        }
        if (d & (1 << bitno)) != 0 {
            image.put_pixel(x, py, image::Rgba([fg.0, fg.1, fg.2, 255]));
        }
    }
}

// ── DCS scanner ────────────────────────────────────────────────────────

/// Scan for a sixel DCS sequence (`\x1bP[params]q…\x1b\\`) and return
/// the params portion + payload portion separately.
pub fn scan_sixel_dcs(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let mut i = 0;
    while i + 2 < bytes.len() {
        // DCS introducer: ESC P (0x1B 0x50)
        if bytes[i] == 0x1B && bytes[i + 1] == b'P' {
            let param_start = i + 2;
            let mut j = param_start;
            // Skip optional numeric parameters separated by `;`
            while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b';') {
                j += 1;
            }
            // After parameters, expect `q` (sixel final byte)
            if j < bytes.len() && bytes[j] == b'q' {
                let payload_start = j + 1;
                let mut k = payload_start;
                while k + 1 < bytes.len() {
                    if bytes[k] == 0x1B && bytes[k + 1] == b'\\' {
                        let params_raw = &bytes[param_start..j];
                        let payload = &bytes[payload_start..k];
                        return Some((params_raw, payload));
                    }
                    k += 1;
                }
            }
        }
        i += 1;
    }
    None
}

/// Parse the DCS parameter string (e.g. `"1;2;3"` from `\x1bP1;2;3q`).
pub fn parse_dcs_params(bytes: &[u8]) -> Vec<i64> {
    let mut params = Vec::new();
    let mut current = 0i64;
    let mut i = 2; // skip ESC P
    while i < bytes.len() && bytes[i] != b'q' {
        if bytes[i] == b';' {
            params.push(current);
            current = 0;
        } else if bytes[i].is_ascii_digit() {
            current = current
                .saturating_mul(10)
                .saturating_add((bytes[i] - b'0') as i64);
        } else {
            break;
        }
        i += 1;
    }
    params.push(current);
    params
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sixel_parse() {
        let mut builder = SixelBuilder::new(&[0, 0, 0]);
        // "HI" example from wikipedia
        let data = b"#0;2;0;0;0#1;2;100;100;0#2;2;0;100;0\
            #1~~@@vv@@~~@@~~$\
            #2??}}GG}}??}}??-\
            #1!14@";
        for &b in data {
            builder.push(b);
        }
        builder.finish();
        assert!(!builder.sixel.data.is_empty());
        let img = render_sixel(&builder.sixel).unwrap();
        assert!(img.data().width() > 0);
        assert!(img.data().height() > 0);
    }

    #[test]
    fn oversized_explicit_dimensions_are_rejected_before_rendering() {
        let mut builder = SixelBuilder::new(&[]);
        for byte in b"\";1;1;5000000000;1" {
            builder.push(*byte);
        }
        builder.finish();

        assert!(render_sixel(&builder.sixel).is_err());
    }
}
