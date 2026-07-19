use std::sync::Arc;

use alacritty_terminal::grid::Dimensions;

use zenterm_core::image::{ImageData, ImageDataType};
use zenterm_core::{ITermDimension, ITermFileData};

use crate::image::kitty::{self, KittyImage};
use crate::image::sixel::{self, SixelBuilder};
use crate::image::{
    assign_image_to_cells, PlacementParams, PlacementStyle,
};

use super::unicode::VirtualPlacement;
use super::Terminal;

// ── Kitty protocol handler ─────────────────────────────────────────────

impl Terminal {
    /// Handle a parsed Kitty image command.
    /// Returns `Some(response_bytes)` for `a=q` queries that must be
    /// written back to the PTY.
    pub(crate) fn handle_kitty_command(&mut self, cmd: KittyImage) -> Option<String> {
        // Feed through the accumulator to support multi-chunk transmissions.
        let assembled = match self.kitty_accumulator.feed(cmd) {
            Ok(Some(assembled)) => assembled,
            Ok(None) => return None, // waiting for more chunks
            Err(e) => {
                log::error!("[img] kitty accumulator error: {e}");
                return None;
            }
        };

        log::debug!(
            "[img] handle_kitty_command: variant={}, cache_images={}, placements={}",
            kitty_cmd_variant_name(&assembled),
            self.image_cache.all_hashes().len(),
            self.image_placements.len(),
        );

        match assembled {
            KittyImage::TransmitData { transmit, verbosity } => {
                log::debug!(
                    "[img] TransmitData: fmt={:?}, w={:?}, h={:?}, id={:?}, num={:?}",
                    transmit.format, transmit.width, transmit.height,
                    transmit.image_id, transmit.image_number,
                );
                // Implicit ID (i=0, I=0): do not respond.
                let implicit = transmit.image_id == Some(0) && transmit.image_number == Some(0);
                let resp_id = transmit.image_id;
                let resp_num = transmit.image_number;
                if verbosity != kitty::KittyImageVerbosity::Quiet {
                    match kitty::decode_image_data(transmit, &mut self.image_cache) {
                        Ok(id) => {
                            log::debug!("[img] TransmitData decode OK, image_id={id}");
                            if implicit { return None; }
                            return Some(kitty::kitty_response(
                                Some(id), None, "OK",
                            ));
                        }
                        Err(e) => {
                            log::error!("[img] TransmitData decode FAILED: {e}");
                            if implicit { return None; }
                            return Some(kitty::kitty_response(
                                resp_id, resp_num,
                                &format!("ERROR:{e}"),
                            ));
                        }
                    }
                } else {
                    let _ = kitty::decode_image_data(transmit, &mut self.image_cache);
                }
                None
            }
            KittyImage::TransmitDataAndDisplay { transmit, placement, .. } => {
                log::debug!(
                    "[img] TransmitDataAndDisplay: fmt={:?}, w={:?}, h={:?}, id={:?}, num={:?}, virtual={}",
                    transmit.format, transmit.width, transmit.height,
                    transmit.image_id, transmit.image_number,
                    placement.virtual_placement,
                );
                match kitty::decode_image_data(transmit, &mut self.image_cache) {
                    Ok(image_id) => {
                        log::debug!("[img] decode OK, image_id={image_id}, calling kitty_place_image");
                        self.kitty_place_image(Some(image_id), None, placement);
                    }
                    Err(e) => log::error!("[img] decode FAILED: {e}"),
                }
                None
            }
            KittyImage::Display { image_id, image_number, placement, .. } => {
                log::debug!(
                    "[img] Display: image_id={image_id:?}, num={image_number:?}, virtual={}",
                    placement.virtual_placement,
                );
                self.kitty_place_image(image_id, image_number, placement);
                None
            }
            KittyImage::Delete { what, .. } => {
                log::debug!("[img] Delete");
                self.handle_kitty_delete(what);
                None
            }
            KittyImage::Query { transmit } => {
                log::debug!(
                    "[img] Query: id={:?}, num={:?}",
                    transmit.image_id, transmit.image_number,
                );
                // EINVAL: image ID required for query.
                if transmit.image_id == Some(0) && transmit.image_number == Some(0) {
                    return Some(kitty::kitty_response(
                        transmit.image_id, transmit.image_number,
                        "EINVAL: image ID required",
                    ));
                }
                Some(kitty::kitty_response(
                    transmit.image_id,
                    transmit.image_number,
                    "OK",
                ))
            }
            KittyImage::TransmitFrame { transmit, frame, verbosity } => {
                log::debug!("[img] TransmitFrame");
                let result = kitty::decode_image_frame(transmit, frame, &mut self.image_cache);
                match &result {
                    Ok(()) => {
                        if verbosity != kitty::KittyImageVerbosity::Quiet {
                            // No image_id readily available from frame result; respond generically.
                            return Some(kitty::kitty_response(None, None, "OK"));
                        }
                    }
                    Err(e) => {
                        log::error!("[img] frame transmit FAILED: {e}");
                        if verbosity != kitty::KittyImageVerbosity::OnlyErrors {
                            return Some(kitty::kitty_response(None, None, &format!("ERROR:{e}")));
                        }
                    }
                }
                None
            }
            KittyImage::ComposeFrame { frame, verbosity } => {
                log::debug!("[img] ComposeFrame");
                let resp_id = frame.image_id;
                let resp_num = frame.image_number;
                let result = kitty::handle_compose_frame(frame, &mut self.image_cache);
                match &result {
                    Ok(()) => {
                        if verbosity != kitty::KittyImageVerbosity::Quiet {
                            return Some(kitty::kitty_response(
                                resp_id, resp_num, "OK",
                            ));
                        }
                    }
                    Err(e) => {
                        log::error!("[img] compose frame FAILED: {e}");
                        if verbosity != kitty::KittyImageVerbosity::OnlyErrors {
                            return Some(kitty::kitty_response(
                                resp_id, resp_num,
                                &format!("ERROR:{e}"),
                            ));
                        }
                    }
                }
                None
            }
            KittyImage::AnimationControl { control, verbosity } => {
                log::debug!(
                    "[img] AnimationControl: action={:?}, frame={:?}, gap={:?}",
                    control.action, control.frame, control.gap_ms,
                );
                // Animation playback control is not yet supported; return error.
                if verbosity != kitty::KittyImageVerbosity::OnlyErrors {
                    return Some(kitty::kitty_response(None, None, "ERROR: animation control not implemented"));
                }
                None
            }
        }
    }

    fn kitty_place_image(
        &mut self,
        image_id: Option<u32>,
        image_number: Option<u32>,
        placement: kitty::KittyImagePlacement,
    ) {
        let id = self.image_cache.assign_id(image_id, image_number);
        log::debug!(
            "[img] kitty_place_image: resolved_id={id}, image_id={image_id:?}, \
             num={image_number:?}, virtual={}, cell_pixel={}x{}, do_not_move={}",
            placement.virtual_placement,
            self.cell_pixel_width, self.cell_pixel_height,
            placement.do_not_move_cursor,
        );

        // U=1 — virtual placement: store metadata for later rendering via
        // Unicode placeholder characters.  No direct image is placed.
        if placement.virtual_placement {
            // EINVAL: virtual placement cannot refer to a parent.
            if placement.parent_id.is_some_and(|p| p > 0) {
                log::error!(
                    "[img] EINVAL: virtual placement cannot refer to a parent (parent_id={})",
                    placement.parent_id.unwrap(),
                );
                return;
            }
            let vp = VirtualPlacement {
                image_id: id,
                placement_id: placement.placement_id,
                columns: placement.columns.unwrap_or(0),
                rows: placement.rows.unwrap_or(0),
                source_x: placement.x,
                source_y: placement.y,
                source_w: placement.w,
                source_h: placement.h,
                x_offset: placement.x_offset,
                y_offset: placement.y_offset,
                z_index: placement.z_index.unwrap_or(0),
            };
            log::debug!(
                "[img] virtual placement stored: id={}, p={:?}, grid={}x{}",
                vp.image_id, vp.placement_id, vp.columns, vp.rows,
            );
            self.virtual_placements.insert(
                (vp.image_id, vp.placement_id),
                vp,
            );
            // Virtual placements do not move the cursor.
            return;
        }

        // Direct placement path (U=0 or absent).
        let data = match self.image_cache.get(id) {
            Some(d) => d.clone(),
            None => {
                log::error!("[img] kitty place: image id {id} not found in cache");
                return;
            }
        };

        let img_w = data.data().width();
        let img_h = data.data().height();

        if self.cell_pixel_width == 0 || self.cell_pixel_height == 0 {
            log::warn!(
                "[img] kitty_place_image: cell_pixel is 0 ({}x{}), SKIPPING placement",
                self.cell_pixel_width, self.cell_pixel_height,
            );
            return;
        }

        let cursor = self.cursor();
        let cols = self.term.columns();
        let rows = self.term.screen_lines();

        // X/Y (unsigned) are the primary cell padding offsets.
        // H/V (signed) are for relative placements (P/Q parent);
        // since parent placement is not yet supported, H/V are stored
        // but do not affect the placement coordinates.
        let params = PlacementParams {
            columns: placement.columns.map(|c| c as usize),
            rows: placement.rows.map(|r| r as usize),
            source_x: placement.x,
            source_y: placement.y,
            source_w: placement.w,
            source_h: placement.h,
            cell_padding_left: placement.x_offset.unwrap_or(0) as u16,
            cell_padding_top: placement.y_offset.unwrap_or(0) as u16,
            z_index: placement.z_index.unwrap_or(0),
            do_not_move_cursor: placement.do_not_move_cursor,
            image_id: Some(id),
            placement_id: placement.placement_id,
            style: PlacementStyle::Kitty,
        };

        let result = assign_image_to_cells(
            data,
            img_w,
            img_h,
            &params,
            self.cell_pixel_width,
            self.cell_pixel_height,
            cursor.pos.column,
            cursor.pos.line.min(rows.saturating_sub(1)),
            cols,
            rows,
        );

        // Store placements keyed by grid-relative line so they follow
        // content when the viewport scrolls.
        let display_offset = self.term.grid().display_offset() as i32;
        for (col, viewport_row, cell) in &result.cells {
            // viewport_row is in [0, screen_lines).  Convert to grid line.
            let grid_line = *viewport_row as i32 - display_offset;
            self.image_placements.insert((grid_line, *col), cell.clone());
        }

        let new_cursor = if result.move_cursor {
            let new_col = (cursor.pos.column + result.width_in_cells).min(cols.saturating_sub(1));
            let new_row = (cursor.pos.line + result.height_in_cells)
                .saturating_sub(1)
                .min(rows.saturating_sub(1));
            (new_col, new_row)
        } else {
            (cursor.pos.column, cursor.pos.line)
        };
        log::debug!(
            "[img] placed {} cells ({}x{}), total_placements={}, \
             img={}x{}px, cursor ({},{})→({},{})",
            result.cells.len(), result.width_in_cells, result.height_in_cells,
            self.image_placements.len(),
            img_w, img_h,
            cursor.pos.column, cursor.pos.line,
            new_cursor.0, new_cursor.1,
        );

        if result.move_cursor {
            // Kitty moves cursor to after the bottom-right of the image.
            self.term.grid_mut().cursor.point.column = alacritty_terminal::index::Column(new_cursor.0);
            self.term.grid_mut().cursor.point.line = alacritty_terminal::index::Line(new_cursor.1 as i32);
        }

        self.damage.mark_all();
    }

    /// Look up a built-in iTerm2 session variable by name.
    ///
    /// Returns `Some(value)` for recognised variables, `None` otherwise.
    /// The caller falls back to `user_vars` if this returns `None`.
    pub(crate) fn iterm_builtin_var(&self, name: &str) -> Option<String> {
        match name {
            "session.terminalName" => Some("Zenterm".into()),
            "session.name" => {
                // Use the tab title if available, otherwise "zenterm".
                Some(
                    self.pending_title
                        .as_ref()
                        .cloned()
                        .unwrap_or_else(|| "zenterm".into()),
                )
            }
            "session.hostname" => {
                // Try environment variables, fall back to hostname command.
                let from_env = std::env::var("HOSTNAME")
                    .or_else(|_| std::env::var("HOST"))
                    .ok();
                if let Some(host) = from_env {
                    Some(host)
                } else {
                    std::process::Command::new("hostname")
                        .output()
                        .ok()
                        .and_then(|o| {
                            if o.status.success() {
                                String::from_utf8(o.stdout)
                                    .ok()
                                    .map(|s| s.trim().to_string())
                            } else {
                                None
                            }
                        })
                }
            }
            "session.path" => {
                // Current working directory from OSC 7 if available.
                self.pending_current_directory.clone()
            }
            "session.tty" => {
                // Return the terminal device if we have it.
                None // Not tracked in current architecture.
            }
            _ => None,
        }
    }

    /// Handle iTerm2 inline image (`OSC 1337;File=…` with `inline=1`).
    ///
    /// Decodes the image data, stores it in the image cache, and places it
    /// on the terminal grid at the current cursor position.
    pub(crate) fn handle_iterm_inline_image(&mut self, file: ITermFileData) {
        log::debug!(
            "[iterm-img] inline image: name={:?}, size={:?}, data_len={}, \
             cell_pixel={}x{}, cursor={:?}",
            file.name,
            file.size,
            file.data.len(),
            self.cell_pixel_width,
            self.cell_pixel_height,
            self.cursor(),
        );

        if self.cell_pixel_width == 0 || self.cell_pixel_height == 0 {
            log::warn!(
                "[iterm-img] cell_pixel is 0 ({}x{}), SKIPPING placement",
                self.cell_pixel_width,
                self.cell_pixel_height,
            );
            return;
        }

        // Decode the image data using the `image` crate (PNG, JPEG, GIF, …).
        let decoded = match image::load_from_memory(&file.data) {
            Ok(img) => img.into_rgba8(),
            Err(e) => {
                log::error!("[iterm-img] failed to decode image: {e}");
                return;
            }
        };
        let (img_w, img_h) = decoded.dimensions();
        let rgba = decoded.into_vec();

        // Store in image cache with a unique id.
        let image_data = Arc::new(ImageData::new(ImageDataType::new_rgba8(
            rgba, img_w, img_h,
        )));
        // Use a unique auto-incrementing number so each image gets its own
        // cache slot, even when the application sends multiple `File=` sequences.
        let number = self.next_iterm_image_number;
        self.next_iterm_image_number += 1;
        let image_id = self.image_cache.assign_id(None, Some(number));
        self.image_cache.insert(image_id, image_data.clone());

        // Convert iTerm2 dimensions to columns/rows for PlacementParams.
        let cols = self.term.columns();
        let rows = self.term.screen_lines();

        let (columns, rows_opt) = self.iterm_dimensions_to_grid(
            file.width,
            file.height,
            img_w,
            img_h,
            cols,
            rows,
        );

        let cursor = self.cursor();
        let cursor_col = cursor.pos.column;
        let cursor_row = cursor.pos.line.min(rows.saturating_sub(1));

        let params = PlacementParams {
            columns: Some(columns),
            rows: rows_opt,
            source_x: None,
            source_y: None,
            source_w: None,
            source_h: None,
            cell_padding_left: 0,
            cell_padding_top: 0,
            z_index: 0,
            do_not_move_cursor: file.do_not_move_cursor,
            image_id: Some(image_id),
            placement_id: None,
            style: PlacementStyle::Iterm,
        };

        let result = assign_image_to_cells(
            image_data,
            img_w,
            img_h,
            &params,
            self.cell_pixel_width,
            self.cell_pixel_height,
            cursor_col,
            cursor_row,
            cols,
            rows,
        );

        // Store placements.
        let display_offset = self.term.grid().display_offset() as i32;
        for (col, viewport_row, cell) in &result.cells {
            let grid_line = *viewport_row as i32 - display_offset;
            self.image_placements.insert((grid_line, *col), cell.clone());
        }

        // Move cursor if needed.
        if result.move_cursor {
            let new_col = (cursor_col + result.width_in_cells).min(cols.saturating_sub(1));
            let new_row = (cursor_row + result.height_in_cells)
                .saturating_sub(1)
                .min(rows.saturating_sub(1));
            self.term.grid_mut().cursor.point.column =
                alacritty_terminal::index::Column(new_col);
            self.term.grid_mut().cursor.point.line =
                alacritty_terminal::index::Line(new_row as i32);
        }

        log::debug!(
            "[iterm-img] placed {} cells ({}x{}), img={}x{}px",
            result.cells.len(),
            result.width_in_cells,
            result.height_in_cells,
            img_w,
            img_h,
        );

        self.damage.mark_all();
    }

    /// Convert iTerm2 `ITermDimension` width/height to grid columns/rows.
    fn iterm_dimensions_to_grid(
        &self,
        width: ITermDimension,
        height: ITermDimension,
        img_w: u32,
        img_h: u32,
        max_cols: usize,
        max_rows: usize,
    ) -> (usize, Option<usize>) {
        let cell_w = self.cell_pixel_width.max(1);
        let cell_h = self.cell_pixel_height.max(1);

        let calc_cols = |dim: ITermDimension| -> Option<usize> {
            match dim {
                ITermDimension::Automatic => None,
                ITermDimension::Cells(n) => Some(n.max(1) as usize),
                ITermDimension::Pixels(n) => {
                    Some((n.max(1) as u32 / cell_w).max(1) as usize)
                }
                ITermDimension::Percent(n) => {
                    let pct = n.max(1).min(100) as usize;
                    Some((max_cols * pct / 100).max(1))
                }
            }
        };
        let calc_rows = |dim: ITermDimension| -> Option<usize> {
            match dim {
                ITermDimension::Automatic => None,
                ITermDimension::Cells(n) => Some(n.max(1) as usize),
                ITermDimension::Pixels(n) => {
                    Some((n.max(1) as u32 / cell_h).max(1) as usize)
                }
                ITermDimension::Percent(n) => {
                    let pct = n.max(1).min(100) as usize;
                    Some((max_rows * pct / 100).max(1))
                }
            }
        };

        let columns = calc_cols(width).unwrap_or_else(|| {
            ((img_w + cell_w - 1) / cell_w).max(1) as usize
        });
        let rows_out = calc_rows(height).unwrap_or_else(|| {
            ((img_h + cell_h - 1) / cell_h).max(1) as usize
        });

        (columns.min(max_cols), Some(rows_out.min(max_rows)))
    }

    fn handle_kitty_delete(&mut self, what: kitty::KittyImageDelete) {
        match what {
            kitty::KittyImageDelete::All { delete } => {
                self.image_placements.clear();
                self.virtual_placements.clear();
                if delete {
                    // Collect all hashes before clearing for atlas cleanup.
                    let hashes: Vec<[u8; 32]> = self.image_cache.all_hashes();
                    self.pending_image_deallocations.extend(hashes);
                    self.image_cache.clear();
                }
            }
            kitty::KittyImageDelete::ByImageId { image_id, placement_id, delete } => {
                self.image_placements.retain(|_, v| {
                    if v.image_id != Some(image_id) { return true; }
                    placement_id.map_or(false, |p| v.placement_id != Some(p))
                });
                self.virtual_placements.retain(|(id, pid), _| {
                    *id != image_id || placement_id.is_some_and(|p| *pid != Some(p))
                });
                if delete {
                    if let Some(hash) = self.image_cache.remove(image_id) {
                        self.pending_image_deallocations.push(hash);
                    }
                }
            }
            kitty::KittyImageDelete::ByImageNumber { image_number: _, placement_id, delete } => {
                // Look up the image_id from the number mapping.
                let ids: Vec<u32> = self.image_placements.iter()
                    .filter(|(_, v)| v.placement_id == placement_id)
                    .map(|(_, v)| v.image_id)
                    .flatten()
                    .collect();
                for id in ids {
                    self.image_placements.retain(|_, v| v.image_id != Some(id));
                    self.virtual_placements.retain(|(vid, pid), _| {
                        *vid != id || placement_id.is_some_and(|p| *pid != Some(p))
                    });
                    if delete {
                        self.image_cache.remove(id);
                    }
                }
            }
            kitty::KittyImageDelete::AtCursorPosition { delete } => {
                let cursor = self.cursor();
                self.image_placements.retain(|&(line, col), _| {
                    let viewport_row = line + self.term.grid().display_offset() as i32;
                    viewport_row != cursor.pos.line as i32 || col != cursor.pos.column
                });
                if delete {
                    // Can't delete data without knowing the image_id.
                    log::warn!("kitty delete AtCursorPosition with delete=true: image_id unknown");
                }
            }
            kitty::KittyImageDelete::DeleteAt { x, y, delete } => {
                let display_offset = self.term.grid().display_offset() as i32;
                let del_grid_line = y as i32 - display_offset;
                self.image_placements.retain(|&(line, col), _| {
                    !(line == del_grid_line && col == x as usize)
                });
                if delete {
                    log::warn!("kitty delete DeleteAt with delete=true: image_id unknown");
                }
            }
            kitty::KittyImageDelete::DeleteColumn { x, delete: _ } => {
                let display_offset = self.term.grid().display_offset() as i32;
                self.image_placements.retain(|&(line, _), _| {
                    let viewport_row = line + display_offset;
                    viewport_row != x as i32
                });
            }
            kitty::KittyImageDelete::DeleteRow { y, delete: _ } => {
                self.image_placements.retain(|&(_, col), _| col != y as usize);
            }
            kitty::KittyImageDelete::DeleteZ { z, delete: _ } => {
                self.image_placements.retain(|_, v| v.z_index != z);
            }
            kitty::KittyImageDelete::DeleteAnimationFrames { delete } => {
                // For each image in the cache, if it is animated (AnimRgba8),
                // convert it to single-frame Rgba8 (keep first frame only).
                // Then remove all placements for that image.
                let all_ids: Vec<u32> = self.image_cache.all_image_ids();
                for id in all_ids {
                    let dominated = self.image_cache.get(id).map(|d| {
                        let guard = d.data();
                        matches!(&*guard, zenterm_core::image::ImageDataType::AnimRgba8 { .. })
                    }).unwrap_or(false);

                    if dominated {
                        // Convert AnimRgba8 → Rgba8 (keep first frame).
                        if let Some(d) = self.image_cache.get(id) {
                            let mut guard = d.data();
                            if let zenterm_core::image::ImageDataType::AnimRgba8 {
                                ref width, ref height, ref frames, ..
                            } = *guard {
                                if let Some(first_frame) = frames.first() {
                                    let new_data = zenterm_core::image::ImageDataType::new_rgba8(
                                        first_frame.clone(), *width, *height,
                                    );
                                    *guard = new_data;
                                }
                            }
                        }
                        // Remove all placements for this image since animation changed.
                        self.image_placements.retain(|_, v| v.image_id != Some(id));
                        self.virtual_placements.retain(|(vid, _), _| *vid != id);
                    }
                    if delete {
                        if let Some(hash) = self.image_cache.remove(id) {
                            self.pending_image_deallocations.push(hash);
                        }
                        self.image_placements.retain(|_, v| v.image_id != Some(id));
                        self.virtual_placements.retain(|(vid, _), _| *vid != id);
                    }
                }
            }
            kitty::KittyImageDelete::DeleteAtCellZ { x, y, z, delete } => {
                let display_offset = self.term.grid().display_offset() as i32;
                let del_grid_line = y as i32 - display_offset;
                self.image_placements.retain(|&(line, col), v| {
                    !(line == del_grid_line && col == x as usize && v.z_index == z)
                });
                if delete {
                    log::warn!("kitty delete DeleteAtCellZ with delete=true: image_id unknown");
                }
            }
            kitty::KittyImageDelete::DeleteRange { first, last, delete } => {
                // Delete all placements whose image_id is in [first, last].
                let ids_to_delete: Vec<u32> = self.image_placements.iter()
                    .filter(|(_, v)| {
                        v.image_id.map_or(false, |id| id >= first && id <= last)
                    })
                    .map(|(_, v)| v.image_id.unwrap())
                    .collect();
                for id in ids_to_delete {
                    self.image_placements.retain(|_, v| v.image_id != Some(id));
                    if delete {
                        if let Some(hash) = self.image_cache.remove(id) {
                            self.pending_image_deallocations.push(hash);
                        }
                    }
                }
            }
        }
        self.damage.mark_all();
    }

    /// Handle a sixel image transmission.
    pub(crate) fn handle_sixel(&mut self, payload: &[u8], params: &[i64]) {
        if self.cell_pixel_width == 0 || self.cell_pixel_height == 0 {
            log::warn!("sixel: cell pixel size not set, skipping");
            return;
        }

        let mut builder = SixelBuilder::new(params);
        for &b in payload {
            builder.push(b);
        }
        builder.finish();

        match sixel::render_sixel(&builder.sixel) {
            Ok(data) => {
                let cursor = self.cursor();
                let cols = self.term.columns();
                let rows = self.term.screen_lines();
                let img_w = data.data().width();
                let img_h = data.data().height();

                let par = PlacementParams {
                    columns: None,
                    rows: None,
                    source_x: None,
                    source_y: None,
                    source_w: None,
                    source_h: None,
                    cell_padding_left: 0,
                    cell_padding_top: 0,
                    z_index: 0, // sixel is behind text
                    do_not_move_cursor: false,
                    image_id: None,
                    placement_id: None,
                    style: PlacementStyle::Sixel,
                };

                let result = assign_image_to_cells(
                    data,
                    img_w,
                    img_h,
                    &par,
                    self.cell_pixel_width,
                    self.cell_pixel_height,
                    cursor.pos.column,
                    cursor.pos.line.min(rows.saturating_sub(1)),
                    cols,
                    rows,
                );

                let display_offset = self.term.grid().display_offset() as i32;
                for (col, viewport_row, cell) in &result.cells {
                    let grid_line = *viewport_row as i32 - display_offset;
                    self.image_placements.insert((grid_line, *col), cell.clone());
                }
                self.damage.mark_all();
            }
            Err(e) => log::error!("sixel render: {e}"),
        }
    }
}

// ── Diagnostic helpers ────────────────────────────────────────────────

fn kitty_cmd_variant_name(cmd: &KittyImage) -> &'static str {
    match cmd {
        KittyImage::TransmitData { .. } => "TransmitData",
        KittyImage::TransmitDataAndDisplay { .. } => "TransmitDataAndDisplay",
        KittyImage::Display { .. } => "Display",
        KittyImage::Delete { .. } => "Delete",
        KittyImage::Query { .. } => "Query",
        KittyImage::TransmitFrame { .. } => "TransmitFrame",
        KittyImage::ComposeFrame { .. } => "ComposeFrame",
        KittyImage::AnimationControl { .. } => "AnimationControl",
    }
}
