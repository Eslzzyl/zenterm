//! Grid sizing, scrollback, and resolved cell projection.

use std::collections::HashSet;

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::vte::ansi::Color;

use zenterm_core::cell::Cell;
use zenterm_core::image::ImageCell;
use zenterm_core::size::TermSize;

use crate::term::TermDimensions;

use super::super::grid_view::GridView;
use super::Terminal;
use super::unicode::{PLACEHOLDER_CHAR, diacritic_value};

impl Terminal {
    fn placement_image_hashes(&self) -> HashSet<[u8; 32]> {
        let mut hashes = self
            .image_placements
            .values()
            .map(|cell| cell.data.hash())
            .collect::<HashSet<_>>();
        for placement in self.virtual_placements.values() {
            if let Some(data) = self.image_cache.get(placement.image_id) {
                hashes.insert(data.hash());
            }
        }
        hashes
    }

    fn queue_released_placement_hashes(&mut self, hashes: HashSet<[u8; 32]>) {
        for hash in hashes {
            if !self.pending_image_deallocations.contains(&hash) {
                self.pending_image_deallocations.push(hash);
            }
        }
    }

    /// Update the physical cell metrics without changing the terminal grid.
    ///
    /// Font and DPI changes can leave the row/column count unchanged, so a
    /// normal resize would be skipped by the UI.  Keep the image protocol's
    /// cell metrics and the PTY-reported pixel dimensions in sync explicitly.
    pub fn set_cell_pixel_size(&mut self, width: u32, height: u32) {
        self.cell_pixel_width = width.max(1);
        self.cell_pixel_height = height.max(1);
        self.pixel_width = (self.term.columns() as u32).saturating_mul(self.cell_pixel_width);
        self.pixel_height =
            (self.term.screen_lines() as u32).saturating_mul(self.cell_pixel_height);
        self.damage.mark_all();
    }

    pub fn resize(&mut self, size: TermSize) {
        let released_image_hashes = self.placement_image_hashes();
        let dim = TermDimensions(size);
        let cols = dim.columns();
        let rows = dim.screen_lines();

        self.term.resize(dim);
        self.damage.resize(rows);
        self.grid_cache.resize(rows, vec![Cell::blank(); cols]);
        for row in self.grid_cache.iter_mut() {
            row.resize(cols, Cell::blank());
        }
        self.image_placements.clear();
        self.virtual_placements.clear();
        self.queue_released_placement_hashes(released_image_hashes);
        self.damage.mark_all();
        self.pixel_width = size.pixel_width as u32;
        self.pixel_height = size.pixel_height as u32;
    }

    /// Get the current terminal size (in cells and pixels).
    pub fn size(&self) -> TermSize {
        TermSize::new(
            self.term.screen_lines() as u16,
            self.term.columns() as u16,
            self.pixel_width as u16,
            self.pixel_height as u16,
        )
    }

    /// Return the visible text of a viewport row as a `String`.
    pub fn line_text(&self, row: usize) -> String {
        use alacritty_terminal::index::{Column, Line};
        let cols = self.term.columns();
        let display_offset = self.term.grid().display_offset();
        let grid_line = Line(row as i32 - display_offset as i32);
        let mut text = String::with_capacity(cols);
        for col in 0..cols {
            text.push(self.term.grid()[grid_line][Column(col)].c);
        }
        text
    }

    // ── Scrollback / display offset ─────────────────────────────────────

    /// Scroll the viewport by `count` lines.
    ///
    /// Positive = scroll up (into history), negative = scroll down (toward bottom).
    /// Returns `true` if the display offset actually changed.
    pub fn scroll_display(&mut self, count: i32) -> bool {
        let old = self.term.grid().display_offset();
        self.term.scroll_display(Scroll::Delta(count));
        if self.term.grid().display_offset() != old {
            self.damage.mark_all();
            return true;
        }
        false
    }

    /// Jump to the bottom of the scrollback (latest output).
    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
        self.damage.mark_all();
    }

    /// Jump to the top of the scrollback (oldest history).
    pub fn scroll_to_top(&mut self) {
        self.term.scroll_display(Scroll::Top);
        self.damage.mark_all();
    }

    /// Number of lines currently in scrollback history.
    pub fn history_size(&self) -> usize {
        self.term.grid().history_size()
    }

    /// Update the scrollback limit and immediately release rows above it.
    pub fn set_scrollback_lines(&mut self, scrollback_lines: usize) {
        self.term_config.scrolling_history = scrollback_lines;
        self.term.set_options(self.term_config.clone());
        self.damage.mark_all();
    }

    /// Current scroll position. 0 = at bottom, larger = scrolled into history.
    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    /// Whether the viewport is at the bottom (showing latest output).
    pub fn is_at_bottom(&self) -> bool {
        self.term.grid().display_offset() == 0
    }

    /// Return the number of active image placements (for diagnostics).
    pub fn image_placements_count(&self) -> usize {
        self.image_placements.len() + self.virtual_placements.len()
    }

    /// Get a view of the visible grid with resolved colours.
    ///
    /// Only dirty rows are re-converted; clean rows come from the cache.
    pub fn visible_cells(&mut self) -> GridView<'_> {
        let cols = self.term.columns();
        let screen_lines = self.term.screen_lines();

        // Collect dirty row indices first to avoid borrow conflicts.
        let dirty: Vec<usize> = self.damage.iter().collect();
        let grid = self.term.grid();

        for &row_idx in &dirty {
            if row_idx >= screen_lines {
                continue;
            }
            let grid_line = Line(row_idx as i32 - grid.display_offset() as i32);
            for col_idx in 0..cols.min(self.grid_cache[row_idx].len()) {
                let alacell = &grid[grid_line][Column(col_idx)];
                self.grid_cache[row_idx][col_idx] = self.resolve_cell(alacell);
            }
        }

        // Clear the damage set — it has been consumed by the re-resolution above.
        self.damage.clear();

        // Attach image placements (keyed by grid line) to the grid cache.
        let display_offset = grid.display_offset() as i32;
        for (&(grid_line, col), img_cell) in &self.image_placements {
            let viewport_row = grid_line + display_offset;
            if viewport_row >= 0 && (viewport_row as usize) < self.grid_cache.len() {
                let row = viewport_row as usize;
                if col < self.grid_cache[row].len() {
                    self.grid_cache[row][col].image = Some(img_cell.clone());
                }
            }
        }

        // ── Unicode placeholder rendering (Kitty U=1) ──────────────────
        // Scan the visible grid for cells containing PLACEHOLDER_CHAR
        // (U+10EEEE).  For each one, decode the image ID from the fg color
        // and the row/col from combining diacritics, then create an
        // ImageCell pointing at the correct slice of the image.
        //
        // ratatui-image encodes image IDs using TrueColor fg (`38;2;R;G;B`)
        // → `Color::Spec(Rgb{r,g,b})` and only attaches diacritics to the
        // FIRST placeholder character per row.  Subsequent characters
        // inherit row/id_extra and auto-increment col.
        //
        // We collect render params in a first pass (while grid is borrowed),
        // then create ImageCells in a second pass (after grid is released).
        if !self.virtual_placements.is_empty() {
            // Per-row inheritance state for U+10EEEE without diacritics.
            let mut inh_image_id: Option<u32> = None;
            let mut inh_row: Option<u32> = None;
            let mut inh_id_extra: Option<u32> = None;
            let mut inh_col: u32 = 0;

            // (row_idx, col_idx, image_id, row_val, col_val, id_extra)
            let mut placeholders: Vec<(usize, usize, u32, u32, u32, u32)> = Vec::new();

            for row_idx in 0..screen_lines.min(self.grid_cache.len()) {
                let grid_line = Line(row_idx as i32 - grid.display_offset() as i32);
                for col_idx in 0..cols {
                    let alacell = &grid[grid_line][Column(col_idx)];
                    if alacell.c != PLACEHOLDER_CHAR {
                        // Reset inheritance when a non-placeholder is encountered.
                        inh_image_id = None;
                        inh_row = None;
                        inh_id_extra = None;
                        inh_col = 0;
                        continue;
                    }

                    // ── Extract image_id from foreground color ──────────
                    // ratatui-image uses TrueColor: \x1b[38;2;R;G;Bm
                    // which alacritty stores as Color::Spec(Rgb{r,g,b}).
                    let base_id = match alacell.fg {
                        Color::Spec(rgb) => {
                            (rgb.r as u32) << 16 | (rgb.g as u32) << 8 | rgb.b as u32
                        }
                        Color::Indexed(idx) => idx as u32,
                        _ => {
                            // Unknown color format — skip this cell.
                            continue;
                        }
                    };

                    // ── Extract row/col/id_extra from diacritics ───────
                    let zerowidth = alacell.zerowidth().unwrap_or(&[]);

                    if zerowidth.len() >= 2 {
                        // First character in a row — has full diacritics.
                        let row_val = match diacritic_value(zerowidth[0]) {
                            Some(v) => v,
                            None => {
                                log::warn!(
                                    "[img] unicode placeholder: invalid row diacritic cp={:X}",
                                    zerowidth[0] as u32
                                );
                                continue;
                            }
                        };
                        let col_val = match diacritic_value(zerowidth[1]) {
                            Some(v) => v,
                            None => {
                                log::warn!(
                                    "[img] unicode placeholder: invalid col diacritic cp={:X}",
                                    zerowidth[1] as u32
                                );
                                continue;
                            }
                        };
                        let high = if zerowidth.len() >= 3 {
                            diacritic_value(zerowidth[2]).unwrap_or(0)
                        } else {
                            0
                        };
                        let full_id = (high << 24) | base_id;

                        // Update inheritance state.
                        inh_image_id = Some(full_id);
                        inh_row = Some(row_val);
                        inh_id_extra = Some(high);
                        inh_col = col_val;

                        placeholders.push((row_idx, col_idx, full_id, row_val, col_val, high));
                    } else if inh_row.is_some() {
                        // Subsequent character — no diacritics, use inherited values.
                        let full_id = inh_image_id.unwrap_or(base_id);
                        let row_val = inh_row.unwrap();
                        let col_val = inh_col;
                        inh_col = inh_col.saturating_add(1);

                        placeholders.push((
                            row_idx,
                            col_idx,
                            full_id,
                            row_val,
                            col_val,
                            inh_id_extra.unwrap_or(0),
                        ));
                    }
                    // else: no inherited state yet — skip.
                }
            }

            // Release grid borrow, then create ImageCells.
            let _ = grid; // release immutable borrow before mutable access
            for (row_idx, col_idx, image_id, row_val, col_val, id_extra) in placeholders {
                self.render_unicode_placeholder_cell(
                    row_idx, col_idx, image_id, row_val, col_val, id_extra,
                );
            }
        }

        GridView {
            rows: &self.grid_cache[..screen_lines.min(self.grid_cache.len())],
        }
    }

    /// Render a single Unicode placeholder cell: look up the virtual
    /// placement, compute the per-cell UV coordinates, and store an
    /// `ImageCell` in the grid cache.
    fn render_unicode_placeholder_cell(
        &mut self,
        row_idx: usize,
        col_idx: usize,
        image_id: u32,
        row_val: u32,
        col_val: u32,
        _id_extra: u32,
    ) {
        // Find the virtual placement for this image_id.
        let vp = self.virtual_placements.get(&(image_id, None)).or_else(|| {
            self.virtual_placements
                .iter()
                .find(|((id, _), _)| *id == image_id)
                .map(|(_, vp)| vp)
        });
        let vp = match vp {
            Some(vp) => vp,
            None => {
                log::warn!(
                    "[img] unicode placeholder: no virtual placement for image_id={image_id}"
                );
                return;
            }
        };

        let data = match self.image_cache.get(vp.image_id) {
            Some(d) => d.clone(),
            None => return,
        };
        let img_w = data.data().width();
        let img_h = data.data().height();

        if self.cell_pixel_width == 0 || self.cell_pixel_height == 0 {
            return;
        }

        // ── Grid size ──────────────────────────────────────────────
        let grid_cols = if vp.columns > 0 {
            vp.columns
        } else {
            img_w.div_ceil(self.cell_pixel_width).max(1)
        };
        let grid_rows = if vp.rows > 0 {
            vp.rows
        } else {
            img_h.div_ceil(self.cell_pixel_height).max(1)
        };

        // ── Aspect-ratio-preserving scale ─────────────────────────
        let placement_px_w = grid_cols as f64 * self.cell_pixel_width as f64;
        let placement_px_h = grid_rows as f64 * self.cell_pixel_height as f64;
        let src_x = vp.source_x.unwrap_or(0) as f64;
        let src_y = vp.source_y.unwrap_or(0) as f64;
        let src_w = vp.source_w.unwrap_or(img_w).min(img_w) as f64;
        let src_h = vp.source_h.unwrap_or(img_h).min(img_h) as f64;

        let scale = if src_w * placement_px_h > src_h * placement_px_w {
            placement_px_w / src_w.max(1.0)
        } else {
            placement_px_h / src_h.max(1.0)
        };

        let scaled_src_w = src_w * scale;
        let scaled_src_h = src_h * scale;
        let center_offset_x = (placement_px_w - scaled_src_w) / 2.0;
        let center_offset_y = (placement_px_h - scaled_src_h) / 2.0;

        // ── Per-cell UV calculation ────────────────────────────────
        let cell_px_x = col_val as f64 * self.cell_pixel_width as f64;
        let cell_px_y = row_val as f64 * self.cell_pixel_height as f64;

        let cell_src_x = src_x + (cell_px_x - center_offset_x) / scale;
        let cell_src_y = src_y + (cell_px_y - center_offset_y) / scale;
        let cell_src_w = self.cell_pixel_width as f64 / scale;
        let cell_src_h = self.cell_pixel_height as f64 / scale;

        let clamped_x = cell_src_x.max(src_x);
        let clamped_y = cell_src_y.max(src_y);
        let clamped_w = (cell_src_x + cell_src_w - clamped_x)
            .min(src_x + src_w - clamped_x)
            .max(0.0);
        let clamped_h = (cell_src_y + cell_src_h - clamped_y)
            .min(src_y + src_h - clamped_y)
            .max(0.0);

        if clamped_w <= 0.0 || clamped_h <= 0.0 || img_w == 0 || img_h == 0 {
            return;
        }

        let u0 = clamped_x / img_w as f64;
        let v0 = clamped_y / img_h as f64;
        let u1 = (clamped_x + clamped_w) / img_w as f64;
        let v1 = (clamped_y + clamped_h) / img_h as f64;

        let top_left = zenterm_core::image::TextureCoordinate::new(u0 as f32, v0 as f32);
        let bottom_right = zenterm_core::image::TextureCoordinate::new(u1 as f32, v1 as f32);

        let img_cell = ImageCell {
            top_left,
            bottom_right,
            data,
            z_index: vp.z_index,
            padding_left: vp.x_offset.unwrap_or(0) as u16,
            padding_top: vp.y_offset.unwrap_or(0) as u16,
            padding_right: 0,
            padding_bottom: 0,
            image_id: Some(vp.image_id),
            placement_id: vp.placement_id,
        };

        if row_idx < self.grid_cache.len() && col_idx < self.grid_cache[row_idx].len() {
            self.grid_cache[row_idx][col_idx].image = Some(img_cell);
        }
    }
}
