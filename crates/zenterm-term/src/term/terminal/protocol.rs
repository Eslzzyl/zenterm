//! Scanning for terminal protocol sequences handled outside `vte`.

use std::time::{Duration, Instant};

use alacritty_terminal::grid::Dimensions;

use crate::image::kitty::KittyImage;
use crate::image::sixel;

use super::super::MAX_ESCAPE_SEQUENCE_BYTES;
use super::Terminal;

impl Terminal {
    /// Handle image protocols and terminal queries that are not dispatched by
    /// `vte` in the form required by the UI.
    pub(super) fn scan_special_sequences(
        &mut self,
        bytes: &[u8],
        replies: &mut Vec<u8>,
    ) -> Duration {
        let started = Instant::now();
        let esc_positions = memchr::memchr_iter(0x1b, bytes);
        let mut prev_end: Option<usize> = None;
        let mut apc_count = 0;

        for esc_pos in esc_positions {
            if prev_end.is_some_and(|end| esc_pos < end) {
                continue;
            }

            if esc_pos + 2 >= bytes.len() {
                self.buffer_apc_remainder(bytes, esc_pos);
                break;
            }

            if bytes[esc_pos + 1] == b'_' && bytes[esc_pos + 2] == b'G' {
                let payload_start = esc_pos + 2;
                let st_rel = find_st(&bytes[payload_start..]);
                if let Some(st_rel) = st_rel {
                    let payload = &bytes[payload_start..payload_start + st_rel];
                    if let Some(cmd) = KittyImage::parse_apc(payload) {
                        if let Some(reply) = self.handle_kitty_command(cmd) {
                            log::debug!("[img] Kitty query response: {} bytes", reply.len());
                            replies.extend_from_slice(reply.as_bytes());
                        }
                    } else {
                        log::warn!(
                            "[img] Kitty APC parse FAILED, first 80 bytes: {:?}",
                            String::from_utf8_lossy(&payload[..payload.len().min(80)]),
                        );
                    }
                    apc_count += 1;
                    prev_end = Some(payload_start + st_rel + 2);
                } else {
                    log::warn!(
                        "[img] APC ST not found at offset={} ({} bytes from ESC _ G), \
                         buffering for next feed()",
                        esc_pos,
                        bytes.len() - esc_pos,
                    );
                    self.buffer_apc_remainder(bytes, esc_pos);
                    break;
                }
            }

            if bytes[esc_pos + 1] == b'P' {
                let param_start = esc_pos + 2;
                let mut j = param_start;
                while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b';') {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b'q' {
                    let payload_start = j + 1;
                    if let Some(st_rel) = find_st(&bytes[payload_start..]) {
                        let params = sixel::parse_dcs_params(&bytes[param_start..j]);
                        let payload = &bytes[payload_start..payload_start + st_rel];
                        if payload.len() <= MAX_ESCAPE_SEQUENCE_BYTES {
                            self.handle_sixel(payload, &params);
                        } else {
                            log::warn!(
                                "[img] dropping oversized sixel payload ({} bytes)",
                                payload.len()
                            );
                        }
                        prev_end = Some(payload_start + st_rel + 2);
                    }
                }
            }

            if bytes[esc_pos + 1] == b'[' {
                if esc_pos + 3 < bytes.len()
                    && bytes[esc_pos + 2] == b'2'
                    && bytes[esc_pos + 3] == b'J'
                    && (!self.image_placements.is_empty() || !self.virtual_placements.is_empty())
                {
                    log::debug!(
                        "[img] CSI 2J (Erase Display): clearing {} image placements, {} virtual placements",
                        self.image_placements.len(),
                        self.virtual_placements.len(),
                    );
                    self.image_placements.clear();
                    self.virtual_placements.clear();
                }

                let mut j = esc_pos + 2;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j < bytes.len()
                    && bytes[j] == b't'
                    && j > esc_pos + 2
                    && let Ok(param_str) = std::str::from_utf8(&bytes[esc_pos + 2..j])
                    && param_str == "16"
                {
                    let cols = self.term.columns();
                    let rows = self.term.screen_lines();
                    let cell_w = if cols > 0 {
                        self.pixel_width / cols as u32
                    } else {
                        0
                    };
                    let cell_h = if rows > 0 {
                        self.pixel_height / rows as u32
                    } else {
                        0
                    };
                    let response = format!("\x1b[6;{};{}t", cell_h, cell_w);
                    log::info!(
                        "[img] CSI 16t response: cell_w={cell_w}, cell_h={cell_h}, pixel={}x{}, grid={}x{}",
                        self.pixel_width,
                        self.pixel_height,
                        cols,
                        rows,
                    );
                    replies.extend_from_slice(response.as_bytes());
                    prev_end = Some(j + 1);
                }
            }
        }

        let elapsed = started.elapsed();
        log::debug!(
            "[img] APC scan: batch_len={}, apc_count={}, elapsed={:?}",
            bytes.len(),
            apc_count,
            elapsed,
        );
        elapsed
    }

    fn buffer_apc_remainder(&mut self, bytes: &[u8], esc_pos: usize) {
        let remainder = &bytes[esc_pos..];
        self.apc_remainder.clear();
        if remainder.len() <= MAX_ESCAPE_SEQUENCE_BYTES {
            self.apc_remainder.extend_from_slice(remainder);
        } else {
            log::warn!(
                "[img] dropping oversized unterminated Kitty APC ({} bytes)",
                remainder.len()
            );
        }
    }
}

fn find_st(bytes: &[u8]) -> Option<usize> {
    memchr::memchr(0x1b, bytes)
        .and_then(|rel| (rel + 1 < bytes.len() && bytes[rel + 1] == b'\\').then_some(rel))
}
