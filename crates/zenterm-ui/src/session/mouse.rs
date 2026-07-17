//! Per-tab mouse handling + context menu for [`TerminalSession`].

use std::time::Instant;

use alacritty_terminal::term::TermMode;

use super::types::{TerminalSession, SCROLLBAR_WIDTH, SCROLLBAR_MIN_THUMB_HEIGHT};

impl TerminalSession {
    /// Update `hover_cell` from the current egui pointer position.
    /// Must be called **before** `update_cell_instances` so the URL
    /// underline is rendered in the correct frame.
    pub fn compute_hover(&mut self, ui: &egui::Ui, cell_rect: egui::Rect) {
        if !self.url_hover_underline {
            self.hover_cell = None;
            return;
        }
        let ppp = ui.ctx().pixels_per_point();
        let pos = ui.ctx().input(|i| i.pointer.hover_pos());
        log::debug!(
            "compute_hover: pointer_pos={:?} cell_rect={:?} cw={} ch={}",
            pos,
            cell_rect,
            self.cell_width,
            self.cell_height,
        );
        let new_hover = pos
            .filter(|pos| cell_rect.contains(*pos))
            .and_then(|pos| {
                let col = ((pos.x - cell_rect.left()) * ppp / self.cell_width) as usize;
                let row = ((pos.y - cell_rect.top()) * ppp / self.cell_height) as usize;
                let cols = self.terminal.size().cols as usize;
                let rows = self.terminal.size().rows as usize;
                log::debug!(
                    "compute_hover: col={} row={} cols={} rows={} cw={} ch={}",
                    col, row, cols, rows, self.cell_width, self.cell_height,
                );
                if col < cols && row < rows {
                    Some((row, col))
                } else {
                    log::debug!("compute_hover: cell ({},{}) out of bounds", row, col);
                    None
                }
            });
        log::debug!(
            "compute_hover: old={:?} new={:?}",
            self.hover_cell,
            new_hover,
        );
        if new_hover != self.hover_cell {
            self.hover_cell = new_hover;
            if self.url_hover_underline {
                self.terminal_dirty = true;
            }
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
        let mode = self.terminal.mode();
        let mouse_reporting = mode.contains(TermMode::SGR_MOUSE)
            && mode.intersects(
                TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION,
            );
        log::info!(
            "[dbg] mouse_reporting={} mode={:#b} sgr={} click={} drag={} motion={} alt_screen={}",
            mouse_reporting,
            mode.bits(),
            mode.contains(TermMode::SGR_MOUSE),
            mode.contains(TermMode::MOUSE_REPORT_CLICK),
            mode.contains(TermMode::MOUSE_DRAG),
            mode.contains(TermMode::MOUSE_MOTION),
            mode.contains(TermMode::ALT_SCREEN),
        );

        // SGR modifier encoding: Shift=4, Alt=8, Ctrl=16
        let mods = ui.ctx().input(|i| i.modifiers);
        let mod_bits = (if mods.shift { 4u8 } else { 0 })
            | (if mods.alt { 8u8 } else { 0 })
            | (if mods.ctrl { 16u8 } else { 0 });

        let cw = self.cell_width;
        let ch = self.cell_height;
        let ppp = ui.ctx().pixels_per_point();
        let rows = self.terminal.size().rows as usize;
        let cols = self.terminal.size().cols as usize;
        let _ = size_px;

        // ── Scrollbar geometry ───────────────────────────────────────────
        let sb_rect = egui::Rect::from_min_max(
            egui::pos2(rect.right() - SCROLLBAR_WIDTH, rect.top()),
            egui::pos2(rect.right(), rect.bottom()),
        );
        let cell_area = egui::Rect::from_min_max(
            rect.min,
            egui::pos2(rect.right() - SCROLLBAR_WIDTH, rect.bottom()),
        );

        // ── Scrollbar: click / drag / track-click ──────────────────────
        if let Some(pos) = response.interact_pointer_pos() {
            if sb_rect.contains(pos) {
                // ── Drag start on the scrollbar ──
                if response.drag_started() {
                    self.scrollbar_dragging = true;
                    self.scrollbar_drag_start_y = pos.y;
                    self.scrollbar_drag_start_offset = self.terminal.display_offset();
                }
                // ── Track-click (above/below thumb) → page up/down ──
                if response.clicked() {
                    let hist = self.terminal.history_size();
                    if hist > 0 {
                        let (thumb, _) =
                            Self::scrollbar_thumb_rect(sb_rect, self.terminal.size().rows as usize, hist, self.terminal.display_offset());
                        if pos.y < thumb.top() {
                            self.terminal.scroll_display(rows as i32);
                        } else if pos.y > thumb.bottom() {
                            self.terminal.scroll_display(-(rows as i32));
                        }
                        self.terminal_dirty = true;
                    }
                }
                return; // scrollbar area: don't process cell events
            }
        }

        // ── Scrollbar: drag thumb update (tracked even if pointer left the bar) ──
        if self.scrollbar_dragging {
            if let Some(pos) = response.interact_pointer_pos() {
                let hist = self.terminal.history_size() as f32;
                if hist > 0.0 {
                    let dy = pos.y - self.scrollbar_drag_start_y;
                    let ratio_delta = dy / sb_rect.height();
                    let offset_delta = (ratio_delta * hist) as i32;
                    let target = (self.scrollbar_drag_start_offset as i32 - offset_delta)
                        .clamp(0, hist as i32);
                    let cur = self.terminal.display_offset() as i32;
                    if target != cur {
                        self.terminal.scroll_display(target - cur);
                        self.terminal_dirty = true;
                    }
                }
            }
            if response.drag_stopped() {
                self.scrollbar_dragging = false;
            }
        }

        // ── Pointer → cell coordinate helpers ──────────────────────────
        // NOTE: cw/ch are in physical pixels, but pos is in logical points.
        // Multiply by ppp to convert before dividing.
        let pixel_to_cell = |pos: egui::Pos2| -> Option<(usize, usize)> {
            let col = ((pos.x - cell_area.left()) * ppp / cw) as usize;
            let row = ((pos.y - cell_area.top()) * ppp / ch) as usize;
            if col < cols && row < rows {
                Some((row, col))
            } else {
                None
            }
        };

        // Clamped version: returns the nearest cell even when outside the area.
        let pixel_to_cell_clamped = |pos: egui::Pos2| -> (usize, usize) {
            let col = ((pos.x - cell_area.left()) * ppp / cw).round() as usize;
            let row = ((pos.y - cell_area.top()) * ppp / ch).round() as usize;
            (row.min(rows.saturating_sub(1)), col.min(cols.saturating_sub(1)))
        };

        // ── Hover tracking (for URL underline) ──────────────────────────
        let new_hover = if self.url_hover_underline {
            let pos = response.hover_pos();
            log::debug!("mouse: response.hover_pos()={:?}", pos);
            pos.and_then(|p| pixel_to_cell(p))
        } else {
            None
        };
        if new_hover != self.hover_cell {
            log::debug!("mouse: hover_cell {:?} → {:?}", self.hover_cell, new_hover);
            self.hover_cell = new_hover;
            if self.url_hover_underline {
                self.terminal_dirty = true;
            }
        }

        // ── Drag start / selection ─────────────────────────────────────
        if response.drag_started() {
            if let Some(pos) = response.interact_pointer_pos() {
                if let Some((row, col)) = pixel_to_cell(pos) {
                    if mouse_reporting {
                        let btn = 0 | mod_bits; // left button
                        self.sgr_mouse_buttons.retain(|&b| b & 0b11 != btn & 0b11);
                        self.sgr_mouse_buttons.push(btn);
                        self.send_sgr_mouse(row, col, btn, false);
                    } else {
                        self.terminal.clear_selection();
                        self.terminal.start_selection(row, col);
                        self.selecting = true;
                        self.terminal_dirty = true;
                    }
                }
            }
        }

        // ── Drag update (selection or edge-scroll) ─────────────────────
        //
        // Use the global pointer state to detect drags that continue outside
        // the terminal widget (edge-scroll).  `response.dragged()` may return
        // false once the pointer leaves the widget rect.
        let is_dragging = response.dragged()
            || ui.ctx().input(|i| i.pointer.is_decidedly_dragging());
        if is_dragging {
            let pointer_pos = response.interact_pointer_pos()
                .or_else(|| ui.ctx().input(|i| i.pointer.interact_pos()));
            if mouse_reporting {
                if let Some(pos) = pointer_pos {
                    if let Some((row, col)) = pixel_to_cell(pos) {
                        self.send_sgr_mouse(row, col, 32 | mod_bits, false);
                    }
                }
            } else if self.selecting {
                if let Some(pos) = pointer_pos {
                    // Normal: pointer inside the cell grid → update selection.
                    if let Some((row, col)) = pixel_to_cell(pos) {
                        self.terminal.update_selection(row, col);
                        self.terminal_dirty = true;
                    } else {
                        // Edge-scroll: pointer is outside the cell grid.
                        let rel_y = pos.y - cell_area.top();
                        let clamped = pixel_to_cell_clamped(pos);
                        if rel_y < 0.0 {
                            // Above top → scroll up.
                            let dist = -rel_y;
                            let lines = (dist * ppp / ch).ceil().max(1.0) as i32;
                            self.terminal.scroll_display(lines);
                            self.terminal.update_selection(0, clamped.1);
                        } else {
                            // Below bottom → scroll down.
                            let dist = pos.y - cell_area.bottom();
                            let lines = (dist * ppp / ch).ceil().max(1.0) as i32;
                            self.terminal.scroll_display(-lines);
                            self.terminal
                                .update_selection(rows.saturating_sub(1), clamped.1);
                        }
                        self.terminal_dirty = true;
                    }
                }
            }
        }

        // ── Right-click → context menu, or SGR right-click ────────────
        if response.secondary_clicked() {
            if mouse_reporting {
                if let Some(pos) = response.interact_pointer_pos() {
                    if let Some((row, col)) = pixel_to_cell(pos) {
                        let btn = 2 | mod_bits; // right button
                        self.sgr_mouse_buttons.retain(|&b| b & 0b11 != btn & 0b11);
                        self.sgr_mouse_buttons.push(btn);
                        self.send_sgr_mouse(row, col, btn, false);
                    }
                }
            }
        }

        // ── Middle-click → paste from selection (X11 convention) ──────
        if response.middle_clicked() {
            if mouse_reporting {
                if let Some(pos) = response.interact_pointer_pos() {
                    if let Some((row, col)) = pixel_to_cell(pos) {
                        let btn = 1 | mod_bits; // middle button
                        self.sgr_mouse_buttons.retain(|&b| b & 0b11 != btn & 0b11);
                        self.sgr_mouse_buttons.push(btn);
                        self.send_sgr_mouse(row, col, btn, false);
                    }
                }
            } else {
                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                    if let Ok(text) = clipboard.get_text() {
                        if !text.is_empty() {
                            if let Err(e) = self.pty.write(text.as_bytes()) {
                                log::error!("PTY paste error: {e}");
                            }
                        }
                    }
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
        let pointer_in_terminal = pointer_pos.map_or(false, |p| response.rect.contains(p));
        if pointer_in_terminal || self.scrollbar_dragging {
            log::info!(
                "[dbg] wheel: enter processing, pointer_in_terminal={} scrollbar_dragging={} mouse_reporting={} num_events={}",
                pointer_in_terminal, self.scrollbar_dragging, mouse_reporting,
                ui.ctx().input(|i| i.events.len()),
            );
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
                log::info!(
                    "[dbg] SGR branch: collected {} wheel events: {:?}, pointer_pos={:?}, pixel_to_cell={:?}",
                    scroll_ys.len(), scroll_ys,
                    pointer_pos,
                    pointer_pos.and_then(|p| pixel_to_cell(p)),
                );
                // Consume all wheel events to prevent egui from using them.
                ui.ctx()
                    .input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::MouseWheel { .. })));
                // Send SGR scroll events with delta accumulation.
                // Accumulate all scroll deltas and send one event per
                // line of total scroll.  Without this, each tiny sub-line
                // trackpad delta (e.g. 0.09 lines) would generate its own
                // SGR event, making scrolling feel sluggish.
                if !scroll_ys.is_empty() {
                    if let Some(pos) = pointer_pos {
                        if let Some((row, col)) = pixel_to_cell(pos) {
                            let total: f32 = scroll_ys.iter().sum();
                            // Accumulate in pixel space (alacritty-style).
                            // This preserves fractional deltas across frames
                            // so slow/precise scrolling doesn't lose events.
                            self.scroll_accumulator_y += total as f64 * self.cell_height as f64;
                            let lines = (self.scroll_accumulator_y / self.cell_height as f64).abs() as i32;
                            if lines != 0 {
                                let btn = if self.scroll_accumulator_y > 0.0 { 64 } else { 65 };
                                let btn_val = btn | mod_bits;
                                log::info!(
                                    "[dbg] SGR: acc={}, sending {} events btn={} col={} row={}",
                                    self.scroll_accumulator_y, lines, btn_val, col + 1, row + 1,
                                );
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
                                self.scroll_accumulator_y %= self.cell_height as f64;
                                let write_start = Instant::now();
                                if let Err(e) = self.pty.write(&batch) {
                                    log::error!("SGR mouse batch write error: {e}");
                                }
                                let write_elapsed = write_start.elapsed();
                                if write_elapsed > std::time::Duration::from_millis(10) {
                                    log::warn!(
                                        "[perf] SGR batch write: {} bytes in {:?}",
                                        batch.len(), write_elapsed,
                                    );
                                }
                            } else {
                                log::info!("[dbg] SGR: accumulated total={} too small, skipping", total);
                            }
                        } else {
                            log::info!("[dbg] SGR: pixel_to_cell returned None (pointer over scrollbar?)");
                        }
                    } else {
                        log::info!("[dbg] SGR: pointer_pos is None, can't send SGR");
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
                    ui.ctx()
                        .input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::MouseWheel { .. })));
                    // Do not scroll while an alternate-screen app is running
                    // (e.g. vim, less — the app handles its own scrolling).
                    if !mode.contains(TermMode::ALT_SCREEN) {
                        let lines = total_scroll.round() as i32;
                        if lines != 0 {
                            self.terminal.scroll_display(lines);
                            self.terminal_dirty = true;
                        }
                    } else {
                        log::info!(
                            "[dbg] non-SGR + ALT_SCREEN: consuming {} scroll events without forwarding! total_scroll={}",
                            total_scroll.abs().round() as i32,
                            total_scroll,
                        );
                    }
                }
            }
        }

        // ── Drag stop ──────────────────────────────────────────────────
        let drag_ended = response.drag_stopped()
            || (self.selecting && !ui.ctx().input(|i| i.pointer.is_decidedly_dragging()));
        if drag_ended {
            if mouse_reporting {
                if let Some(pos) = response.interact_pointer_pos()
                    .or_else(|| ui.ctx().input(|i| i.pointer.interact_pos()))
                {
                    if let Some((row, col)) = pixel_to_cell(pos) {
                        // Use the last tracked button for the release encoding;
                        // fall back to left button (0) if nothing is tracked.
                        let base = self.sgr_mouse_buttons.last().copied().unwrap_or(0);
                        self.sgr_mouse_buttons.pop();
                        self.send_sgr_mouse(row, col, base | mod_bits, true);
                    }
                }
            } else {
                self.selecting = false;
                self.terminal_dirty = true;
            }
        }

        // ── SGR left-click ───────────────────────────────────────────
        // NOTE: `response.clicked()` fires for a press-release sequence
        // without significant drag.  When SGR mouse is active we must
        // forward both the press and the release to the PTY.
        if response.clicked() && mouse_reporting {
            if let Some(pos) = response.interact_pointer_pos() {
                if let Some((row, col)) = pixel_to_cell(pos) {
                    let btn = 0 | mod_bits; // left button
                    self.sgr_mouse_buttons.retain(|&b| b & 0b11 != btn & 0b11);
                    self.send_sgr_mouse(row, col, btn, false); // press
                    self.sgr_mouse_buttons.retain(|&b| b & 0b11 != btn & 0b11);
                    self.send_sgr_mouse(row, col, btn, true);  // release
                }
            }
            return;
        }

        // ── Single click: URL open (Ctrl+Click) or clear selection ───
        if response.clicked() && !self.selecting && !mouse_reporting {
            if self.url_open && !self.url_click_handled {
                let ctrl = ui.ctx().input(|i| i.modifiers.ctrl || i.modifiers.mac_cmd);
                if ctrl {
                    if let Some(pos) = response.interact_pointer_pos() {
                        if let Some((row, col)) = pixel_to_cell(pos) {
                            let line = self.terminal.line_text(row);
                            let finder = linkify::LinkFinder::new();
                            for link in finder.links(&line) {
                                let start_col = line[..link.start()].chars().count();
                                let end_col = line[..link.end()].chars().count();
                                if col >= start_col && col < end_col {
                                    let url = link.as_str().to_string();
                                    log::info!("url click: opening {url}");
                                    let _ = open::that(&url);
                                    self.url_click_handled = true;
                                    return;
                                }
                            }
                        }
                    }
                }
            }
            self.url_click_handled = false;
            self.terminal.clear_selection();
            self.terminal_dirty = true;
        }

        // ── SGR motion (hover / drag move) ──────────────────────────────
        // Forwarded when:
        //   • any-event-mouse (1003) is active – any pointer movement, OR
        //   • a button is currently pressed (drag, mode 1002).
        if mouse_reporting {
            let any_event = mode.contains(TermMode::MOUSE_MOTION);
            let button_pressed = !self.sgr_mouse_buttons.is_empty();
            if any_event || button_pressed {
                let pos = ui.ctx().input(|i| i.pointer.hover_pos());
                if let Some(pos) = pos {
                    if let Some((row, col)) = pixel_to_cell(pos) {
                        if self.last_sgr_motion_pos != Some((row, col)) {
                            // Base button: 32 (motion flag) + last tracked
                            // base button, or 32 if nothing is pressed (pure
                            // hover with any-event-mouse).
                            let base = self.sgr_mouse_buttons.last().copied().unwrap_or(0);
                            self.send_sgr_mouse(row, col, 32 | base | mod_bits, false);
                            self.last_sgr_motion_pos = Some((row, col));
                        }
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
        let history = self.terminal.history_size();
        let screen = self.terminal.size().rows as usize;
        let total = history + screen;
        if total == 0 {
            return;
        }

        let track = egui::Rect::from_min_max(
            egui::pos2(rect.right() - SCROLLBAR_WIDTH, rect.top()),
            egui::pos2(rect.right(), rect.bottom()),
        );

        let (thumb, _thumb_h) =
            Self::scrollbar_thumb_rect(track, screen, history, self.terminal.display_offset());

        let active = self.scrollbar_dragging || ui.rect_contains_pointer(track);

        // Track background.
        ui.painter()
            .rect_filled(track, 0.0, egui::Color32::from_black_alpha(if active { 40 } else { 15 }));

        // Thumb – only draw when there is actually something to scroll.
        if screen < total {
            ui.painter().rect_filled(
                thumb,
                4.0,
                egui::Color32::from_gray(if active { 160 } else { 100 }),
            );
        }
    }

    /// Render the right-click context menu (Copy / Paste).
    pub fn render_context_menu(
        &mut self,
        _ui: &egui::Ui,
        response: &egui::Response,
    ) {
        response.context_menu(|ctx_ui| {
            if self.terminal.has_selection() {
                if ctx_ui.button("Copy").clicked() {
                    if let Some(text) = self.terminal.selected_text() {
                        ctx_ui.ctx().copy_text(text);
                    }
                    ctx_ui.close();
                }
            }
            if ctx_ui.button("Paste").clicked() {
                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                    if let Ok(text) = clipboard.get_text() {
                        if !text.is_empty() {
                            if let Err(e) = self.pty.write(text.as_bytes()) {
                                log::error!("PTY paste error: {e}");
                            }
                        }
                    }
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
        assert!((thumb.bottom() - 400.0).abs() < 1.0, "thumb.bottom={}", thumb.bottom());
        assert!((thumb.top() - (400.0 - 133.33)).abs() < 1.0, "thumb.top={}", thumb.top());
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
