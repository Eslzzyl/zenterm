//! The main eframe application for Zenmux.
//!
//! Wires together the PTY session, terminal state machine, glyph atlas,
//! GPU renderer, and input mapper into a single egui application.
//!
//! # Rendering flow
//!
//! 1. [`update()`](Self::update) — pump PTY bytes, dispatch keyboard input.
//! 2. [`ui()`](Self::ui) — build cell instance data, register the
//!    [`egui_wgpu::Callback`] that renders the terminal grid via wgpu.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use egui::Context;

use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::selection::SelectionRange;

use zenmux_core::{Rgba, TermSize};
use zenmux_glyph::GlyphAtlas;
use zenmux_input::InputMapper;
use zenmux_pty::PtySession;
use zenmux_render::callback::{AtlasUpdate, SharedRenderState, TerminalWgpuCallback};
use zenmux_render::CallbackHandle;
use zenmux_render::CellInstance;
use zenmux_term::Terminal;

/// The top-level application state.
pub struct ZenmuxApp {
    terminal: Terminal,
    pty: PtySession,
    glyph_atlas: GlyphAtlas,

    // ── Wgpu resources ──────────────────────────────────────────────────
    shared: Arc<SharedRenderState>,
    callback: CallbackHandle,

    // ── Layout cache ────────────────────────────────────────────────────
    cell_width: f32,
    cell_height: f32,

    /// Last known atlas texture size (for detecting atlas growth).
    last_atlas_size: u32,

    // ── Selection state ──────────────────────────────────────────────────
    /// True while the left mouse button is held and a drag-selection is
    /// in progress.
    selecting: bool,
}

impl ZenmuxApp {
    /// Create a new Zenmux application with the given wgpu resources.
    ///
    /// Spawns a shell in a PTY, sets up the terminal state machine, and
    /// pre-rasterises the initial glyph atlas.
    pub fn new_with_wgpu(
        device: wgpu::Device,
        queue: wgpu::Queue,
        target_format: wgpu::TextureFormat,
        pixels_per_point: f32,
    ) -> Self {
        let size = TermSize::new(24, 80);

        let pty = PtySession::spawn(size).expect("failed to spawn PTY");
        let terminal = Terminal::new(size, Default::default());

        // Font size in physical pixels: logical points × DPI scale factor.
        // At 200% scaling, 18pt → 36 physical pixels.
        let font_size = 18.0 * pixels_per_point;
        let font_family = GlyphAtlas::default_font_family();
        let mut glyph_atlas = GlyphAtlas::new(font_size, font_family, pixels_per_point);

        // Seed the atlas with a few common ASCII characters so the first
        // frame has something to render before the user types anything.
        for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789 .,!?;:-=+*/\\|()[]{}<>\"'`~@#$%^&_"
            .chars()
        {
            let _ = glyph_atlas.ensure_glyph(c);
        }

        let (cell_width, cell_height) = glyph_atlas.cell_size().expect("failed to measure cell size");

        let shared = Arc::new(SharedRenderState::new(
            size.rows as usize * size.cols as usize,
        ));

        // Build the callback; its pipeline + bind group will be created
        // lazily on the first `prepare()` call.
        let callback = TerminalWgpuCallback::new(
            device.clone(),
            queue.clone(),
            target_format,
            shared.clone(),
        );
        let callback = CallbackHandle::new(callback);

        let last_atlas_size = glyph_atlas.texture_size;

        // Signal the initial atlas data so the callback creates its
        // GPU texture and render pass on the very first prepare() call.
        {
            let mut update = shared.atlas_update.lock().unwrap();
            *update = Some(AtlasUpdate {
                size: last_atlas_size,
                data: glyph_atlas.texture_data.clone(),
                resized: true,
            });
        }
        shared.atlas_dirty.store(true, Ordering::Release);

        Self {
            terminal,
            pty,
            glyph_atlas,
            shared,
            callback,
            cell_width,
            cell_height,
            last_atlas_size,
            selecting: false,
        }
    }

    /// Pump pending PTY bytes into the terminal state machine.
    ///
    /// Also handles VT sequences that ConPTY/WinPTY sends during
    /// initialisation — most importantly **DSR** (`\x1b[6n`, "Cursor
    /// Position Report") which must be answered or the PTY may never
    /// deliver the shell prompt.
    fn pump_pty(&mut self) {
        let mut total = 0usize;
        while let Some(result) = self.pty.try_read() {
            match result {
                Ok(data) => {
                    total += data.len();

                    // ── Respond to Device Status Report ──────────────
                    // Windows ConPTY sends \x1b[6n on startup and expects
                    // \x1b[<row>;<col>R back.  Without this response the
                    // shell may never output its prompt.
                    if data.windows(4).any(|w| w == b"\x1b[6n") {
                        if let Err(e) = self.pty.write(b"\x1b[1;1R") {
                            log::error!("failed to write DSR response: {e}");
                        } else {
                            log::debug!("pump_pty: responded to DSR query");
                        }
                    }

                    self.terminal.feed(&data);
                }
                Err(e) => {
                    log::error!("PTY error: {e}");
                    break;
                }
            }
        }
        if total > 0 {
            log::debug!("pump_pty: read {} bytes from PTY", total);
        }
    }

    /// Build `CellInstance` vector from the current visible grid and
    /// store it in the shared render state.
    ///
    /// Each glyph is rendered at its **native pixel size** positioned at
    /// the bearing offset within the cell — following the approach used
    /// by Alacritty and WezTerm.  The cell background is provided by
    /// egui's `rect_filled` underneath.
    ///
    /// If the terminal cursor is visible, the cell at the cursor position
    /// has its foreground and background colours **swapped** (inverse
    /// video block cursor).  A blank cell at the cursor position is
    /// rendered as a white block.
    fn update_cell_instances(
        &mut self,
        vp_width_px: f32,
        vp_height_px: f32,
    ) {
        // Read cursor info BEFORE visible_cells() since both borrow
        // self.terminal (one mut, one immut).
        let cursor = self.terminal.cursor();
        let cursor_row = cursor.pos.line;
        let cursor_col = cursor.pos.column;
        let cursor_visible = cursor.visible;

        // Pre-compute selection range so the inner loop does not need
        // an additional borrow on self.terminal during grid iteration.
        let sel_range: Option<SelectionRange> = self.terminal.selection_range();

        let grid = self.terminal.visible_cells();
        let rows = grid.row_count();
        let cols = grid.col_count();

        log::debug!(
            "update_cell_instances: {}x{} terminal, viewport {:.0}x{:.0}px, cell {:.1}x{:.1}px",
            cols,
            rows,
            vp_width_px,
            vp_height_px,
            self.cell_width,
            self.cell_height,
        );

        // Early-out for empty terminal.
        if rows == 0 || cols == 0 {
            log::warn!("update_cell_instances: empty terminal grid");
            return;
        }

        let tex_size = self.glyph_atlas.texture_size as f32;
        let cw = self.cell_width;
        let ch = self.cell_height;

        // Pre-compute pixel → clip-space conversion.
        let x_scale = 2.0 / vp_width_px;
        let y_scale = 2.0 / vp_height_px;

        let mut instances = Vec::with_capacity(rows * cols);
        let mut blank_count = 0u32;
        let mut glyph_fail = 0u32;
        let mut has_new_glyphs = false;

        for row in 0..rows {
            for col in 0..cols {
                let cell = match grid.cell(row, col) {
                    Some(c) => c,
                    None => continue,
                };

                let is_cursor = cursor_visible && row == cursor_row && col == cursor_col;

                let ch_char = cell.c;

                // Skip fully blank cells (space on default background),
                // UNLESS this is the cursor position (render as cursor block).
                if ch_char == ' ' && cell.bg == Rgba::BLACK && !is_cursor {
                    blank_count += 1;
                    continue;
                }

                // Look up — or rasterise — the glyph.
                let (entry, is_new) = match self.glyph_atlas.ensure_glyph(ch_char) {
                    Ok(e) => e,
                    Err(_) => {
                        glyph_fail += 1;
                        continue;
                    }
                };
                if is_new {
                    has_new_glyphs = true;
                }

                let atlas_w = (entry.atlas_rect.max.x - entry.atlas_rect.min.x) as f32;
                let atlas_h = (entry.atlas_rect.max.y - entry.atlas_rect.min.y) as f32;

                // ── Colours ──────────────────────────────────────────────
                let is_sel = sel_range.as_ref().is_some_and(|range| {
                    let pt = Point::new(Line(row as i32), Column(col));
                    range.contains(pt)
                });

                // ── Geometry ────────────────────────────────────────────
                //
                // Every cell renders a glyph-sized quad at the bearing
                // offset (native-resolution text).  For cursor and selected
                // cells we additionally render a FULL-CELL background quad
                // underneath so the colour fills the entire grid cell.
                let glyph_x_px = col as f32 * cw + entry.bearing_x;
                let glyph_y_px = row as f32 * ch + (ch - entry.bearing_y);
                let gqx = glyph_x_px * x_scale - 1.0;
                let gqy = 1.0 - glyph_y_px * y_scale;
                let gqw = atlas_w * x_scale;
                let gqh = atlas_h * y_scale;

                // ── UV coordinates ──────────────────────────────────────
                let u_min = (entry.atlas_rect.min.x as f32 + 0.5) / tex_size;
                let v_min = (entry.atlas_rect.min.y as f32 + 0.5) / tex_size;
                let u_max = (entry.atlas_rect.max.x as f32 - 0.5) / tex_size;
                let v_max = (entry.atlas_rect.max.y as f32 - 0.5) / tex_size;

                // ── Background quad (full cell, cursor/selected only) ───
                // Push BEFORE the glyph quad so it is drawn underneath.
                if is_cursor || is_sel {
                    let bqx = (col as f32 * cw) * x_scale - 1.0;
                    let bqy = 1.0 - (row as f32 * ch) * y_scale;
                    let bqw = cw * x_scale;
                    let bqh = ch * y_scale;
                    let bg_colour = if is_cursor {
                        [cell.fg.r(), cell.fg.g(), cell.fg.b(), 1.0]
                    } else {
                        [0.3, 0.5, 0.8, 1.0]
                    };
                    instances.push(CellInstance {
                        clip_pos: [bqx, bqy],
                        uv_min: [0.0, 0.0],
                        uv_max: [0.0, 0.0],
                        clip_cell_size: [bqw, bqh],
                        glyph_size: [0.0, 0.0],
                        glyph_offset: [0.0, 0.0],
                        fg_color: bg_colour,
                        bg_color: bg_colour,
                    });
                }

                // ── Foreground quad (glyph, all cells) ──────────────────
                if is_cursor {
                    // Inverse video: swap fg/bg for the glyph quad.
                    instances.push(CellInstance {
                        clip_pos: [gqx, gqy],
                        uv_min: [u_min, v_min],
                        uv_max: [u_max, v_max],
                        clip_cell_size: [gqw, gqh],
                        glyph_size: [atlas_w, atlas_h],
                        glyph_offset: [entry.bearing_x, ch - entry.bearing_y],
                        fg_color: [cell.bg.r(), cell.bg.g(), cell.bg.b(), 1.0],
                        bg_color: [cell.fg.r(), cell.fg.g(), cell.fg.b(), 1.0],
                    });
                } else if is_sel {
                    // Selected: normal fg on selection-highlight bg.
                    instances.push(CellInstance {
                        clip_pos: [gqx, gqy],
                        uv_min: [u_min, v_min],
                        uv_max: [u_max, v_max],
                        clip_cell_size: [gqw, gqh],
                        glyph_size: [atlas_w, atlas_h],
                        glyph_offset: [entry.bearing_x, ch - entry.bearing_y],
                        fg_color: [cell.fg.r(), cell.fg.g(), cell.fg.b(), 1.0],
                        bg_color: [0.3, 0.5, 0.8, 1.0],
                    });
                } else {
                    // Normal cell.
                    instances.push(CellInstance {
                        clip_pos: [gqx, gqy],
                        uv_min: [u_min, v_min],
                        uv_max: [u_max, v_max],
                        clip_cell_size: [gqw, gqh],
                        glyph_size: [atlas_w, atlas_h],
                        glyph_offset: [entry.bearing_x, ch - entry.bearing_y],
                        fg_color: [cell.fg.r(), cell.fg.g(), cell.fg.b(), 1.0],
                        bg_color: [cell.bg.r(), cell.bg.g(), cell.bg.b(), 1.0],
                    });
                }
            }
        }

        log::debug!(
            "update_cell_instances: {} instances built, {} blank skipped, {} glyph failures",
            instances.len(),
            blank_count,
            glyph_fail,
        );

        // Store for the callback's `prepare()`.
        *self.shared.instances.lock().unwrap() = instances;

        // ── Sync glyph atlas to GPU ──────────────────────────────────
        // Upload when the atlas has grown OR when new glyphs were added.
        let current_size = self.glyph_atlas.texture_size;
        let resized = current_size != self.last_atlas_size;
        if resized {
            self.last_atlas_size = current_size;
        }
        if resized || has_new_glyphs {
            *self.shared.atlas_update.lock().unwrap() = Some(AtlasUpdate {
                size: current_size,
                data: self.glyph_atlas.texture_data.clone(),
                resized,
            });
            self.shared.atlas_dirty.store(true, Ordering::Release);
        }
    }
}

impl eframe::App for ZenmuxApp {
    /// Phase 1: non-UI work.
    ///
    /// This is called before [`Self::ui()`] each frame.
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // 1. Read pending PTY bytes and feed the terminal parser.
        self.pump_pty();

        // 2. Handle keyboard input.
        //
        //    Some key combinations (Ctrl+Shift+C) are terminal-emulator
        //    commands rather than shell input — check for those first
        //    by iterating events outside the ctx.input borrow so we can
        //    call ctx.copy_text() if needed.
        let copy_requested = ctx.input(|input| {
            input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key {
                        key: egui::Key::C,
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.ctrl && modifiers.shift && !modifiers.alt
                )
            })
        });

        if copy_requested && self.terminal.has_selection() {
            if let Some(text) = self.terminal.selected_text() {
                ctx.copy_text(text);
                // Do NOT send Ctrl+C / 0x03 to the shell when we have a
                // selection — the user intended to copy, not interrupt.
            }
        } else {
            // Normal input: pass all events through InputMapper → PTY.
            ctx.input(|input| {
                for event in &input.events {
                    if let Some(bytes) = InputMapper::map(event) {
                        if let Err(e) = self.pty.write(&bytes) {
                            log::error!("PTY write error: {e}");
                        }
                    }
                }
            });
        }

        // 3. Request continuous repainting.
        ctx.request_repaint();
    }

    /// Phase 2: UI rendering.
    ///
    /// Builds cell instance data from the terminal grid and registers the
    /// wgpu-based paint callback inside a `CentralPanel`.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show_inside(ui, |ui| {
            let available = ui.available_size();
            let pixels_per_point = ui.ctx().pixels_per_point();
            let vp_width_px = available.x * pixels_per_point;
            let vp_height_px = available.y * pixels_per_point;

            // ── Resize terminal to match the available area ─────────────
            let cols = (vp_width_px / self.cell_width).max(10.0) as u16;
            let rows = (vp_height_px / self.cell_height).max(5.0) as u16;
            let new_size = TermSize::new(rows, cols);
            if new_size != self.terminal.size() {
                self.terminal.resize(new_size);
                self.pty.resize(new_size).ok();
            }

            // ── Allocate terminal area and capture interactions ─────────
            let sense = egui::Sense::click_and_drag();
            let (rect, response) = ui.allocate_exact_size(available, sense);

            // ── Mouse-driven text selection ────────────────────────────
            if response.drag_started() {
                // Convert pointer-down position to grid cell.
                if let Some(pos) = response.interact_pointer_pos() {
                    let col = ((pos.x - rect.left()) / self.cell_width) as usize;
                    let row = ((pos.y - rect.top()) / self.cell_height) as usize;
                    if col < cols as usize && row < rows as usize {
                        self.terminal.clear_selection();
                        self.terminal.start_selection(row, col);
                        self.selecting = true;
                    }
                }
            }

            if self.selecting && response.dragged() {
                if let Some(pos) = response.interact_pointer_pos() {
                    let col = ((pos.x - rect.left()) / self.cell_width) as usize;
                    let row = ((pos.y - rect.top()) / self.cell_height) as usize;
                    // Clamp to grid bounds.
                    let col = col.min(cols.saturating_sub(1) as usize);
                    let row = row.min(rows.saturating_sub(1) as usize);
                    self.terminal.update_selection(row, col);
                }
            }

            if self.selecting && response.drag_stopped() {
                self.selecting = false;
                // Selection is finalised — no further action needed here;
                // it will be used by the copy action.
            }

            // Left-click without drag → clear selection.
            if response.clicked() && !self.selecting {
                self.terminal.clear_selection();
            }

            // ── Build GPU instance data from visible cells ──────────────
            self.update_cell_instances(vp_width_px, vp_height_px);

            // ── Register the wgpu paint callback ────────────────────────
            let callback = egui_wgpu::Callback::new_paint_callback(
                rect,
                self.callback.clone(),
            );

            // Draw the terminal background underneath the cells.
            ui.painter().rect_filled(rect, 0.0, egui::Color32::BLACK);

            // Register the callback shape — egui-wgpu will call
            // prepare() and paint() on it.
            ui.painter().add(callback);

            // ── Right-click context menu ───────────────────────────────
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
                    ctx_ui.close();
                }
            });
        });
    }
}
