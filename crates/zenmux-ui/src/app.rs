//! The main eframe application for Zenmux.
//!
//! Wires together the PTY session, terminal state machine, glyph atlas,
//! GPU renderer, and input mapper into a single egui application.

use egui::{CentralPanel, Context};

use zenmux_core::TermSize;
use zenmux_glyph::GlyphAtlas;
use zenmux_input::InputMapper;
use zenmux_pty::PtySession;
use zenmux_term::Terminal;

/// The top-level application state.
pub struct ZenmuxApp {
    terminal: Terminal,
    pty: PtySession,
    glyph_atlas: GlyphAtlas,
    read_buf: Vec<u8>,
}

impl ZenmuxApp {
    /// Create a new Zenmux application.
    ///
    /// Spawns a shell in a PTY and sets up the terminal state machine.
    pub fn new() -> Self {
        let size = TermSize::new(24, 80);

        let pty = PtySession::spawn(size).expect("failed to spawn PTY");
        let terminal = Terminal::new(size, Default::default());
        let glyph_atlas = GlyphAtlas::new(16.0);

        Self {
            terminal,
            pty,
            glyph_atlas,
            read_buf: Vec::with_capacity(4096),
        }
    }

    fn pump_pty(&mut self) {
        while let Some(result) = self.pty.try_read() {
            match result {
                Ok(data) => {
                    self.terminal.feed(&data);
                }
                Err(e) => {
                    log::error!("PTY error: {e}");
                    break;
                }
            }
        }
    }
}

impl Default for ZenmuxApp {
    fn default() -> Self {
        Self::new()
    }
}

impl eframe::App for ZenmuxApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // 1. Read pending PTY bytes and feed the terminal parser.
        self.pump_pty();

        // 2. Handle keyboard input.
        ctx.input(|input| {
            for event in &input.events {
                if let Some(bytes) = InputMapper::map(event) {
                    if let Err(e) = self.pty.write(&bytes) {
                        log::error!("PTY write error: {e}");
                    }
                }
            }
        });

        // 3. Render the terminal into the central panel.
        CentralPanel::default().show(ctx, |ui| {
            let available = ui.available_size();

            // Resize terminal to match the available area.
            let cols = (available.x / 10.0).max(10.0) as u16;
            let rows = (available.y / 18.0).max(5.0) as u16;
            let new_size = TermSize::new(rows, cols);
            if new_size != self.terminal.size() {
                self.terminal.resize(new_size);
                self.pty.resize(new_size).ok();
            }

            let (rect, _response) =
                ui.allocate_exact_size(available, egui::Sense::hover());

            // Draw the terminal background.
            ui.painter().rect_filled(rect, 0.0, egui::Color32::BLACK);

            // Placeholder message until CallbackTrait is wired.
            let painter = ui.painter();
            painter.text(
                rect.left_top() + egui::vec2(4.0, 4.0),
                egui::Align2::LEFT_TOP,
                "Zenmux terminal ready",
                egui::FontId::monospace(14.0),
                egui::Color32::GREEN,
            );
        });

        // 4. Request continuous repainting.
        ctx.request_repaint();
    }

    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Not used — all logic is in `update`.
    }
}
