//! Per-tab mouse handling + context menu for [`TerminalSession`].

use std::time::Instant;

use alacritty_terminal::term::TermMode;

use zenterm_term::Terminal;

use super::types::{SCROLLBAR_MIN_THUMB_HEIGHT, TerminalSession, terminal_content_rect};

// ── Selection helpers ───────────────────────────────────────────────────

/// Selection boundary threshold (Ghostty-style).
///
/// When the pointer falls in the right `(1.0 - SELECTION_THRESHOLD)` portion
/// of a cell, the selection boundary is placed at the leading edge of the
/// *next* cell.  A threshold of 0.6 means the first 60% of a cell belongs to
/// that cell, and the remaining 40% is treated as "lean forward" toward the
/// next cell.  This makes click-drag selection feel more natural at cell
/// boundaries.
const SELECTION_THRESHOLD: f32 = 0.6;

/// If `(row, col)` points to a spacer cell (the trailing half of a wide
/// CJK / emoji character), return the column of the leading (real) cell.
/// Otherwise return `col` unchanged.
fn snap_col(terminal: &mut Terminal, row: usize, col: usize) -> usize {
    if col == 0 {
        return col;
    }
    let grid = terminal.visible_cells();
    if grid.cell(row, col).is_some_and(|c| c.is_spacer) {
        col - 1
    } else {
        col
    }
}

impl TerminalSession {
    /// Update `hover_cell` from the current egui pointer position.
    /// Must be called **before** `update_cell_instances` so the URL
    /// underline is rendered in the correct frame.
    pub fn compute_hover(&mut self, ui: &egui::Ui, cell_rect: egui::Rect) {
        if !self.input.url_hover_underline {
            if self.input.hovered_link.is_some() {
                self.runtime.terminal_dirty = true;
            }
            self.input.hover_cell = None;
            self.input.hovered_link = None;
            return;
        }
        let cell_area = terminal_content_rect(cell_rect, ui.ctx().pixels_per_point());
        let ppp = ui.ctx().pixels_per_point();
        let pos = ui.ctx().input(|i| i.pointer.hover_pos());
        let new_hover = pos.filter(|pos| cell_area.contains(*pos)).and_then(|pos| {
            // URL hover tracks the cell actually under the pointer.  The
            // forward-lean threshold is reserved for text selection.
            let col = ((pos.x - cell_area.left()) * ppp / self.view.cell_width).floor() as usize;
            let row = ((pos.y - cell_area.top()) * ppp / self.view.cell_height).floor() as usize;
            let cols = self.runtime.terminal.size().cols as usize;
            let rows = self.runtime.terminal.size().rows as usize;
            if col < cols && row < rows {
                Some((row, col))
            } else {
                None
            }
        });

        // Hover state is cell-granular.  Pointer motion inside one cell
        // cannot change the effective link, so avoid touching the terminal
        // grid or scanning every detected link again.  A dirty terminal is
        // still refreshed by `update_cell_instances`, which rebuilds the
        // link list before rendering the changed frame.
        if new_hover == self.input.hover_cell {
            return;
        }

        let mut new_hover = new_hover;

        // Snap hover cell from spacer (right half of CJK / emoji wide chars)
        // back to the leading cell so URL hover-underline works over the
        // entire glyph.
        if let Some((row, col)) = new_hover {
            new_hover = Some((row, snap_col(&mut self.runtime.terminal, row, col)));
        }
        let new_link = new_hover.and_then(|(row, col)| {
            self.input
                .detected_links
                .iter()
                .position(|link| link.contains_cell(row, col))
        });
        let link_changed = new_link != self.input.hovered_link;
        if new_hover != self.input.hover_cell {
            self.input.hover_cell = new_hover;
        }
        if link_changed {
            self.input.hovered_link = new_link;
            self.runtime.terminal_dirty = true;
        }
    }

    /// Handle mouse events for this session's cell rectangle.
    ///
    /// Behaviour:
    ///
    /// * If the terminal has `SGR_MOUSE` enabled, every pointer event is
    ///   encoded as an SGR escape sequence and written to the PTY.
    /// * Otherwise, click-drag performs text selection; single click
    ///   clears the selection.
    pub fn handle_mouse(
        &mut self,
        ui: &egui::Ui,
        rect: egui::Rect,
        size_px: [f32; 2],
        response: &egui::Response,
    ) {
        let mode = self.runtime.terminal.mode();
        let mouse_reporting = mode.contains(TermMode::SGR_MOUSE)
            && mode.intersects(
                TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION,
            );

        // Ordinary pointer movement does not affect terminal selection,
        // scrolling, or PTY mouse reporting. URL hover was already computed
        // before this method, so avoid rebuilding all mouse geometry and
        // scanning the input queue for this common path.
        let has_mouse_event = ui.ctx().input(|input| {
            input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::PointerButton { .. }
                        | egui::Event::MouseWheel { .. }
                        | egui::Event::Touch { .. }
                )
            })
        });
        let interaction_active = self.input.selecting
            || self.input.scrollbar_dragging
            || response.clicked()
            || response.drag_started()
            || response.dragged()
            || response.drag_stopped()
            || response.secondary_clicked()
            || response.middle_clicked();
        if !mouse_reporting && !has_mouse_event && !interaction_active {
            return;
        }

        // SGR modifier encoding: Shift=4, Alt=8, Ctrl=16
        let mods = ui.ctx().input(|i| i.modifiers);
        let mod_bits = (if mods.shift { 4u8 } else { 0 })
            | (if mods.alt { 8u8 } else { 0 })
            | (if mods.ctrl { 16u8 } else { 0 });

        let cw = self.view.cell_width;
        let ch = self.view.cell_height;
        let ppp = ui.ctx().pixels_per_point();
        let rows = self.runtime.terminal.size().rows as usize;
        let cols = self.runtime.terminal.size().cols as usize;
        let _ = size_px;

        // ── Scrollbar geometry ───────────────────────────────────────────
        let cell_area = terminal_content_rect(rect, ppp);
        let sb_rect = egui::Rect::from_min_max(
            egui::pos2(cell_area.right(), rect.top()),
            egui::pos2(rect.right(), rect.bottom()),
        );

        // ── Scrollbar: click / drag / track-click ──────────────────────
        if let Some(pos) = response.interact_pointer_pos()
            && sb_rect.contains(pos)
        {
            // ── Drag start on the scrollbar ──
            if response.drag_started() {
                self.input.scrollbar_dragging = true;
                self.input.scrollbar_drag_start_y = pos.y;
                self.input.scrollbar_drag_start_offset = self.runtime.terminal.display_offset();
            }
            // ── Track-click (above/below thumb) → page up/down ──
            if response.clicked() {
                let hist = self.runtime.terminal.history_size();
                if hist > 0 {
                    let (thumb, _) = Self::scrollbar_thumb_rect(
                        sb_rect,
                        self.runtime.terminal.size().rows as usize,
                        hist,
                        self.runtime.terminal.display_offset(),
                    );
                    if pos.y < thumb.top() {
                        self.runtime.terminal.scroll_display(rows as i32);
                    } else if pos.y > thumb.bottom() {
                        self.runtime.terminal.scroll_display(-(rows as i32));
                    }
                    self.runtime.terminal_dirty = true;
                }
            }
            return; // scrollbar area: don't process cell events
        }

        // ── Scrollbar: drag thumb update (tracked even if pointer left the bar) ──
        if self.input.scrollbar_dragging {
            if let Some(pos) = response.interact_pointer_pos() {
                let hist = self.runtime.terminal.history_size() as f32;
                if hist > 0.0 {
                    let dy = pos.y - self.input.scrollbar_drag_start_y;
                    let ratio_delta = dy / sb_rect.height();
                    let offset_delta = (ratio_delta * hist) as i32;
                    let target = (self.input.scrollbar_drag_start_offset as i32 - offset_delta)
                        .clamp(0, hist as i32);
                    let cur = self.runtime.terminal.display_offset() as i32;
                    if target != cur {
                        self.runtime.terminal.scroll_display(target - cur);
                        self.runtime.terminal_dirty = true;
                    }
                }
            }
            if response.drag_stopped() {
                self.input.scrollbar_dragging = false;
            }
        }

        // ── Pointer → cell coordinate helpers ──────────────────────────
        // NOTE: cw/ch are in physical pixels, but pos is in logical points.
        // Multiply by ppp to convert before dividing.
        //
        // Column is computed with a Ghostty-style threshold: clicking in the
        // right `(1 - SELECTION_THRESHOLD)` portion of a cell nudges the
        // boundary to the next cell.  Callers must also apply `snap_col()` to
        // snap back from spacer cells (CJK wide chars).
        let pixel_to_cell = |pos: egui::Pos2| -> Option<(usize, usize)> {
            let col_f = (pos.x - cell_area.left()) * ppp / cw;
            let col = (col_f + (1.0 - SELECTION_THRESHOLD)) as usize;
            let row = ((pos.y - cell_area.top()) * ppp / ch) as usize;
            if col < cols && row < rows {
                Some((row, col))
            } else {
                None
            }
        };

        // Clamped version: returns the nearest cell even when outside the area.
        let pixel_to_cell_clamped = |pos: egui::Pos2| -> (usize, usize) {
            let col_f = (pos.x - cell_area.left()) * ppp / cw;
            let col = (col_f + (1.0 - SELECTION_THRESHOLD)) as usize;
            let row = ((pos.y - cell_area.top()) * ppp / ch) as usize;
            (
                row.min(rows.saturating_sub(1)),
                col.min(cols.saturating_sub(1)),
            )
        };

        // URL hit-testing uses the physical cell under the pointer.  Text
        // selection keeps the forward-lean threshold above for its own UX.
        let pixel_to_link_cell = |pos: egui::Pos2| -> Option<(usize, usize)> {
            if !cell_area.contains(pos) {
                return None;
            }
            let col = ((pos.x - cell_area.left()) * ppp / cw).floor() as usize;
            let row = ((pos.y - cell_area.top()) * ppp / ch).floor() as usize;
            if col < cols && row < rows {
                Some((row, col))
            } else {
                None
            }
        };

        // ── Drag start / selection ─────────────────────────────────────
        if response.drag_started()
            && let Some(pos) = response.interact_pointer_pos()
            && let Some((row, col)) = pixel_to_cell(pos)
        {
            let col = snap_col(&mut self.runtime.terminal, row, col);
            if mouse_reporting {
                let btn = mod_bits; // left button
                self.input
                    .sgr_mouse_buttons
                    .retain(|&b| b & 0b11 != btn & 0b11);
                self.input.sgr_mouse_buttons.push(btn);
                self.send_sgr_mouse(row, col, btn, false);
            } else {
                self.runtime.terminal.clear_selection();
                self.runtime.terminal.start_selection(row, col);
                self.input.selecting = true;
                self.runtime.terminal_dirty = true;
            }
        }

        // ── Drag update (selection or edge-scroll) ─────────────────────
        //
        // Use the global pointer state to detect drags that continue outside
        // the terminal widget (edge-scroll).  `response.dragged()` may return
        // false once the pointer leaves the widget rect.
        let is_dragging =
            response.dragged() || ui.ctx().input(|i| i.pointer.is_decidedly_dragging());
        if is_dragging {
            let pointer_pos = response
                .interact_pointer_pos()
                .or_else(|| ui.ctx().input(|i| i.pointer.interact_pos()));
            if mouse_reporting {
                if let Some(pos) = pointer_pos
                    && let Some((row, col)) = pixel_to_cell(pos)
                {
                    let col = snap_col(&mut self.runtime.terminal, row, col);
                    self.send_sgr_mouse(row, col, 32 | mod_bits, false);
                }
            } else if self.input.selecting
                && let Some(pos) = pointer_pos
            {
                // Normal: pointer inside the cell grid → update selection.
                if let Some((row, col)) = pixel_to_cell(pos) {
                    let col = snap_col(&mut self.runtime.terminal, row, col);
                    self.runtime.terminal.update_selection(row, col);
                    self.runtime.terminal_dirty = true;
                } else {
                    // Edge-scroll: pointer is outside the cell grid.
                    let rel_y = pos.y - cell_area.top();
                    let clamped = pixel_to_cell_clamped(pos);
                    let col = snap_col(&mut self.runtime.terminal, clamped.0, clamped.1);
                    if rel_y < 0.0 {
                        // Above top → scroll up.
                        let dist = -rel_y;
                        let lines = (dist * ppp / ch).ceil().max(1.0) as i32;
                        self.runtime.terminal.scroll_display(lines);
                        self.runtime.terminal.update_selection(0, col);
                    } else {
                        // Below bottom → scroll down.
                        let dist = pos.y - cell_area.bottom();
                        let lines = (dist * ppp / ch).ceil().max(1.0) as i32;
                        self.runtime.terminal.scroll_display(-lines);
                        self.runtime
                            .terminal
                            .update_selection(rows.saturating_sub(1), col);
                    }
                    self.runtime.terminal_dirty = true;
                }
            }
        }

        // ── Right-click → context menu, or SGR right-click ────────────
        if response.secondary_clicked()
            && mouse_reporting
            && let Some(pos) = response.interact_pointer_pos()
            && let Some((row, col)) = pixel_to_cell(pos)
        {
            let col = snap_col(&mut self.runtime.terminal, row, col);
            let btn = 2 | mod_bits; // right button
            self.input
                .sgr_mouse_buttons
                .retain(|&b| b & 0b11 != btn & 0b11);
            self.input.sgr_mouse_buttons.push(btn);
            self.send_sgr_mouse(row, col, btn, false);
        }

        // ── Middle-click → paste from selection (X11 convention) ──────
        if response.middle_clicked() {
            if mouse_reporting {
                if let Some(pos) = response.interact_pointer_pos()
                    && let Some((row, col)) = pixel_to_cell(pos)
                {
                    let col = snap_col(&mut self.runtime.terminal, row, col);
                    let btn = 1 | mod_bits; // middle button
                    self.input
                        .sgr_mouse_buttons
                        .retain(|&b| b & 0b11 != btn & 0b11);
                    self.input.sgr_mouse_buttons.push(btn);
                    self.send_sgr_mouse(row, col, btn, false);
                }
            } else {
                if let Some(ref mut clipboard) = self.input.clipboard
                    && let Ok(text) = clipboard.get_text()
                    && !text.is_empty()
                    && let Err(e) = self.runtime.pty.write(text.as_bytes())
                {
                    log::error!("PTY paste error: {e}");
                }
            }
        }

        // ── Mouse wheel: consume events to prevent egui scrolling ──────
        // NOTE: use direct rect+pointer comparison instead of
        // `response.hovered()` / `rect_contains_pointer()` because egui's
        // `rect_contains_pointer` also checks that the top-most widget at the
        // pointer position belongs to the same layer; a dock-internal
        // `Sense::hover()` overlay covering the root node may cause that check
        // to fail even when the pointer is physically in the terminal area.
        let pointer_pos = ui.ctx().input(|i| i.pointer.hover_pos());
        let pointer_in_terminal = pointer_pos.is_some_and(|p| response.rect.contains(p));
        if pointer_in_terminal || self.input.scrollbar_dragging {
            if mouse_reporting {
                // Collect each scroll event's direction so we can forward
                // them individually as SGR mouse events.
                let scroll_ys: Vec<f32> = ui.ctx().input(|i| {
                    i.events
                        .iter()
                        .filter_map(|e| match e {
                            egui::Event::MouseWheel { delta, unit, .. } => {
                                let y = match unit {
                                    egui::MouseWheelUnit::Line => delta.y,
                                    egui::MouseWheelUnit::Point => delta.y * 4.0 / ch,
                                    egui::MouseWheelUnit::Page => delta.y * rows as f32,
                                };
                                if y != 0.0 { Some(y) } else { None }
                            }
                            _ => None,
                        })
                        .collect()
                });
                // Consume all wheel events to prevent egui from using them.
                ui.ctx().input_mut(|i| {
                    i.events
                        .retain(|e| !matches!(e, egui::Event::MouseWheel { .. }))
                });
                // Send SGR scroll events with delta accumulation.
                // Accumulate all scroll deltas and send one event per
                // line of total scroll.  Without this, each tiny sub-line
                // trackpad delta (e.g. 0.09 lines) would generate its own
                // SGR event, making scrolling feel sluggish.
                if !scroll_ys.is_empty()
                    && let Some(pos) = pointer_pos
                    && let Some((row, col)) = pixel_to_cell(pos)
                {
                    let total: f32 = scroll_ys.iter().sum();
                    // Accumulate in pixel space (alacritty-style).
                    // This preserves fractional deltas across frames
                    // so slow/precise scrolling doesn't lose events.
                    self.input.scroll_accumulator_y += total as f64 * self.view.cell_height as f64;
                    let lines = (self.input.scroll_accumulator_y / self.view.cell_height as f64)
                        .abs() as i32;
                    if lines != 0 {
                        let btn = if self.input.scroll_accumulator_y > 0.0 {
                            64
                        } else {
                            65
                        };
                        let btn_val = btn | mod_bits;
                        // Batch all SGR sequences into a single PTY
                        // write to avoid N `flush()` calls per frame.
                        // Rapid scrolling can fill the PTY buffer and
                        // cause individual flushes to block.
                        let col_1 = col + 1;
                        let row_1 = row + 1;
                        let count = lines as usize;
                        let mut batch = Vec::with_capacity(count * 16);
                        for _ in 0..count {
                            batch.push(b'\x1b');
                            batch.push(b'[');
                            batch.push(b'<');
                            // button (always 2 digits: 64-81)
                            batch.push(b'0' + (btn_val / 10));
                            batch.push(b'0' + (btn_val % 10));
                            batch.push(b';');
                            // column (1-3 digits)
                            if col_1 >= 100 {
                                batch.push(b'0' + (col_1 / 100) as u8);
                                batch.push(b'0' + ((col_1 / 10) % 10) as u8);
                            } else if col_1 >= 10 {
                                batch.push(b'0' + (col_1 / 10) as u8);
                            }
                            batch.push(b'0' + (col_1 % 10) as u8);
                            batch.push(b';');
                            // row (1-3 digits)
                            if row_1 >= 100 {
                                batch.push(b'0' + (row_1 / 100) as u8);
                                batch.push(b'0' + ((row_1 / 10) % 10) as u8);
                            } else if row_1 >= 10 {
                                batch.push(b'0' + (row_1 / 10) as u8);
                            }
                            batch.push(b'0' + (row_1 % 10) as u8);
                            batch.push(b'M');
                        }
                        // Preserve the fractional remainder in pixel
                        // space, matching alacritty's approach.
                        self.input.scroll_accumulator_y %= self.view.cell_height as f64;
                        let write_start = Instant::now();
                        if let Err(e) = self.runtime.pty.write(&batch) {
                            log::error!("SGR mouse batch write error: {e}");
                        }
                        let write_elapsed = write_start.elapsed();
                        if write_elapsed > std::time::Duration::from_millis(10) {
                            log::warn!(
                                "[perf] SGR batch write: {} bytes in {:?}",
                                batch.len(),
                                write_elapsed,
                            );
                        }
                    }
                }
            } else {
                // Accumulate scroll amount for local scrollback scrolling.
                let total_scroll: f32 = ui.ctx().input(|i| {
                    i.events
                        .iter()
                        .filter_map(|e| match e {
                            egui::Event::MouseWheel { delta, unit, .. } => {
                                let y = delta.y;
                                match unit {
                                    egui::MouseWheelUnit::Line => Some(y),
                                    egui::MouseWheelUnit::Point => Some(y * 4.0 / ch),
                                    egui::MouseWheelUnit::Page => Some(y * rows as f32),
                                }
                            }
                            _ => None,
                        })
                        .sum()
                });
                if total_scroll.abs() > 0.0 {
                    ui.ctx().input_mut(|i| {
                        i.events
                            .retain(|e| !matches!(e, egui::Event::MouseWheel { .. }))
                    });
                    // Do not scroll while an alternate-screen app is running
                    // (e.g. vim, less — the app handles its own scrolling).
                    if !mode.contains(TermMode::ALT_SCREEN) {
                        let lines = total_scroll.round() as i32;
                        if lines != 0 {
                            self.runtime.terminal.scroll_display(lines);
                            self.runtime.terminal_dirty = true;
                        }
                    }
                }
            }
        }

        // ── Drag stop ──────────────────────────────────────────────────
        let drag_ended = response.drag_stopped()
            || (self.input.selecting && !ui.ctx().input(|i| i.pointer.is_decidedly_dragging()));
        if drag_ended {
            if mouse_reporting {
                if let Some(pos) = response
                    .interact_pointer_pos()
                    .or_else(|| ui.ctx().input(|i| i.pointer.interact_pos()))
                    && let Some((row, col)) = pixel_to_cell(pos)
                {
                    let col = snap_col(&mut self.runtime.terminal, row, col);
                    // Use the last tracked button for the release encoding;
                    // fall back to left button (0) if nothing is tracked.
                    let base = self.input.sgr_mouse_buttons.last().copied().unwrap_or(0);
                    self.input.sgr_mouse_buttons.pop();
                    self.send_sgr_mouse(row, col, base | mod_bits, true);
                }
            } else {
                self.input.selecting = false;
                self.runtime.terminal_dirty = true;

                // ── Auto-copy selection to clipboard ──────────────
                if self.input.save_to_clipboard
                    && let Some(text) = self.runtime.terminal.selected_text()
                    && !text.is_empty()
                    && let Some(ref mut cb) = self.input.clipboard
                    && let Err(e) = cb.set_text(text)
                {
                    log::error!("failed to copy selection to clipboard: {e}");
                }
            }
        }

        // ── SGR left-click ───────────────────────────────────────────
        // NOTE: `response.clicked()` fires for a press-release sequence
        // without significant drag.  When SGR mouse is active we must
        // forward both the press and the release to the PTY.
        if response.clicked() && mouse_reporting {
            if let Some(pos) = response.interact_pointer_pos()
                && let Some((row, col)) = pixel_to_cell(pos)
            {
                let col = snap_col(&mut self.runtime.terminal, row, col);
                let btn = mod_bits; // left button
                self.input
                    .sgr_mouse_buttons
                    .retain(|&b| b & 0b11 != btn & 0b11);
                self.send_sgr_mouse(row, col, btn, false); // press
                self.input
                    .sgr_mouse_buttons
                    .retain(|&b| b & 0b11 != btn & 0b11);
                self.send_sgr_mouse(row, col, btn, true); // release
            }
            return;
        }

        // A duplicated `clicked()` frame must be suppressed, but a later
        // real click after an idle frame must be accepted.
        if !response.clicked() {
            self.input.url_click_handled = false;
        }

        // ── Single click: URL open (Ctrl+Click) or clear selection ───
        if response.clicked() && !self.input.selecting && !mouse_reporting {
            if self.input.url_open && !self.input.url_click_handled {
                let ctrl = ui.ctx().input(|i| i.modifiers.ctrl || i.modifiers.mac_cmd);
                if ctrl
                    && let Some(pos) = response.interact_pointer_pos()
                    && let Some((row, col)) = pixel_to_link_cell(pos)
                {
                    let col = snap_col(&mut self.runtime.terminal, row, col);
                    if let Some(link) = self
                        .input
                        .detected_links
                        .iter()
                        .find(|link| link.contains_cell(row, col))
                    {
                        log::info!(
                            "link click: opening kind={:?} original={} target={}",
                            link.kind,
                            link.original,
                            link.target
                        );
                        if let Err(error) = open::that(&link.target) {
                            log::warn!("link click: failed to open {}: {error}", link.target);
                        }
                        self.input.url_click_handled = true;
                        return;
                    }
                }
            }
            self.input.url_click_handled = false;
            self.runtime.terminal.clear_selection();
            self.runtime.terminal_dirty = true;
        }

        // ── SGR motion (hover / drag move) ──────────────────────────────
        // Forwarded when:
        //   • any-event-mouse (1003) is active – any pointer movement, OR
        //   • a button is currently pressed (drag, mode 1002).
        if mouse_reporting {
            let any_event = mode.contains(TermMode::MOUSE_MOTION);
            let button_pressed = !self.input.sgr_mouse_buttons.is_empty();
            if any_event || button_pressed {
                let pos = ui.ctx().input(|i| i.pointer.hover_pos());
                if let Some(pos) = pos
                    && let Some((row, col)) = pixel_to_cell(pos)
                {
                    let col = snap_col(&mut self.runtime.terminal, row, col);
                    if self.input.last_sgr_motion_pos != Some((row, col)) {
                        // Base button: 32 (motion flag) + last tracked
                        // base button, or 32 if nothing is pressed (pure
                        // hover with any-event-mouse).
                        let base = self.input.sgr_mouse_buttons.last().copied().unwrap_or(0);
                        self.send_sgr_mouse(row, col, 32 | base | mod_bits, false);
                        self.input.last_sgr_motion_pos = Some((row, col));
                    }
                }
            }
        }
    }

    /// Compute the scrollbar thumb rectangle.
    fn scrollbar_thumb_rect(
        track: egui::Rect,
        screen_lines: usize,
        history_size: usize,
        display_offset: usize,
    ) -> (egui::Rect, f32) {
        let total = (history_size + screen_lines).max(1);
        let thumb_ratio = screen_lines as f32 / total as f32;
        let thumb_h = (track.height() * thumb_ratio).max(SCROLLBAR_MIN_THUMB_HEIGHT);
        let avail = track.height() - thumb_h;
        let pos_ratio = if history_size > 0 {
            (history_size - display_offset) as f32 / history_size as f32
        } else {
            1.0
        };
        let thumb_y = track.top() + avail * pos_ratio;
        let thumb = egui::Rect::from_min_max(
            egui::pos2(track.left(), thumb_y),
            egui::pos2(track.right(), (thumb_y + thumb_h).min(track.bottom())),
        );
        (thumb, thumb_h)
    }

    /// Render a custom overlay scrollbar on the right edge of the terminal area.
    pub fn render_scrollbar(&mut self, ui: &egui::Ui, rect: egui::Rect) {
        let history = self.runtime.terminal.history_size();
        let screen = self.runtime.terminal.size().rows as usize;
        if history == 0 {
            return;
        }
        let total = history + screen;

        let terminal_rect = terminal_content_rect(rect, ui.ctx().pixels_per_point());
        let track = egui::Rect::from_min_max(
            egui::pos2(terminal_rect.right(), rect.top()),
            egui::pos2(rect.right(), rect.bottom()),
        );

        let (thumb, _thumb_h) = Self::scrollbar_thumb_rect(
            track,
            screen,
            history,
            self.runtime.terminal.display_offset(),
        );

        let active = self.input.scrollbar_dragging || ui.rect_contains_pointer(track);

        // Track background.
        ui.painter()
            .rect_filled(track, 0.0, self.background_color());

        // Thumb – only draw when there is actually something to scroll.
        if screen < total {
            ui.painter().rect_filled(
                thumb,
                4.0,
                if active {
                    ui.visuals().strong_text_color()
                } else {
                    ui.visuals()
                        .weak_text_color
                        .unwrap_or_else(|| ui.visuals().text_color().gamma_multiply(0.65))
                },
            );
        }
    }

    /// Render the right-click context menu (Copy / Paste).
    pub fn render_context_menu(&mut self, _ui: &egui::Ui, response: &egui::Response) {
        response.context_menu(|ctx_ui| {
            if self.runtime.terminal.has_selection() && ctx_ui.button("Copy").clicked() {
                if let Some(text) = self.runtime.terminal.selected_text()
                    && let Some(ref mut cb) = self.input.clipboard
                    && let Err(e) = cb.set_text(text)
                {
                    log::error!("failed to copy to clipboard: {e}");
                }
                ctx_ui.close();
            }
            if ctx_ui.button("Paste").clicked() {
                if let Some(ref mut clipboard) = self.input.clipboard
                    && let Ok(text) = clipboard.get_text()
                    && !text.is_empty()
                    && let Err(e) = self.runtime.pty.write(text.as_bytes())
                {
                    log::error!("PTY paste error: {e}");
                }
                ctx_ui.close();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::TerminalSession;
    use egui::Rect;

    #[test]
    fn scrollbar_thumb_full_screen_no_history() {
        let track = Rect::from_min_max(egui::pos2(100.0, 0.0), egui::pos2(110.0, 400.0));
        let (thumb, _h) = TerminalSession::scrollbar_thumb_rect(track, 25, 0, 0);
        // With no history, thumb should fill the entire track
        assert!((thumb.top() - 0.0).abs() < 0.001);
        assert!((thumb.bottom() - 400.0).abs() < 0.001);
    }

    #[test]
    fn scrollbar_thumb_scrolled_to_top() {
        let track = Rect::from_min_max(egui::pos2(100.0, 0.0), egui::pos2(110.0, 400.0));
        // 50 lines of history + 25 screen lines = 75 total
        // thumb_ratio = 25/75 = 0.333, thumb_h = 400*0.333 = 133.33
        // pos_ratio = (50-0)/50 = 1.0 → thumb at bottom
        let (thumb, _h) = TerminalSession::scrollbar_thumb_rect(track, 25, 50, 0);
        assert!(
            (thumb.bottom() - 400.0).abs() < 1.0,
            "thumb.bottom={}",
            thumb.bottom()
        );
        assert!(
            (thumb.top() - (400.0 - 133.33)).abs() < 1.0,
            "thumb.top={}",
            thumb.top()
        );
    }

    #[test]
    fn scrollbar_thumb_scrolled_to_bottom() {
        let track = Rect::from_min_max(egui::pos2(100.0, 0.0), egui::pos2(110.0, 400.0));
        // display_offset = 50 (at bottom of history)
        // pos_ratio = (50-50)/50 = 0.0 → thumb at top
        let (thumb, _h) = TerminalSession::scrollbar_thumb_rect(track, 25, 50, 50);
        assert!((thumb.top() - 0.0).abs() < 1.0, "thumb.top={}", thumb.top());
    }

    #[test]
    fn scrollbar_thumb_min_height() {
        // Very tall track + tiny history → thumb should not go below MIN_THUMB_HEIGHT
        let track = Rect::from_min_max(egui::pos2(100.0, 0.0), egui::pos2(110.0, 10000.0));
        let (thumb, h) = TerminalSession::scrollbar_thumb_rect(track, 25, 10000, 0);
        assert!(h >= 24.0, "thumb_h={}", h);
        assert!((thumb.bottom() - 10000.0).abs() < 1.0);
    }
}
