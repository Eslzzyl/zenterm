//! PTY pumping and side-effect processing for [`TerminalSession`].

use std::time::Instant;

use alacritty_terminal::term::TermMode;

use super::effects::SessionEffect;
use super::osc7::osc7_url_to_path;
use super::types::{TerminalSession, TITLE_DEBOUNCE_MS};

impl TerminalSession {
    /// Drain pending PTY bytes into the terminal state machine, write
    /// terminal-query responses back to the PTY, and detect shell exit
    /// (the latter is required for Windows ConPTY where the output
    /// pipe is not closed on child exit).
    ///
    /// All pending chunks are **batched** into a single `feed()` call
    /// to minimise VT parser, lock, and damage-propagation overhead
    /// under high-throughput output (e.g. `cat` of a large file).
    pub fn pump_pty(&mut self) {
        if self.pty_exited {
            return;
        }
        let mut batch = Vec::with_capacity(65536);
        while let Some(result) = self.pty.try_read() {
            match result {
                Ok(data) => batch.extend_from_slice(&data),
                Err(e) => {
                    log::info!("PTY session ended ({e}), exiting");
                    self.pty_exited = true;
                    self.pty.close();
                    break;
                }
            }
        }
        if !batch.is_empty() {
            log::debug!("pump_pty: batching {} bytes from PTY", batch.len());
            let replies = self.terminal.feed(&batch);
            if !replies.is_empty() {
                log::debug!(
                    "pump_pty: writing {} reply bytes: {:02x?}",
                    replies.len(),
                    &replies
                );
                if let Err(e) = self.pty.write(&replies) {
                    log::error!("failed to write pty reply: {e}");
                }
            }
            self.terminal_dirty = true;
        }

        if !self.pty_exited {
            if let Some(status) = self.pty.try_wait() {
                log::info!("shell exited with status: {status:?}, closing");
                self.pty.close();
                self.pty_exited = true;
            }
        }
    }

    /// Apply the side-effects produced by [`Self::pump_pty`]:
    /// window title, bell, exit, clipboard store/load, **OSC 7 cwd**.
    ///
    /// Returns side-effect events the caller must handle
    /// (currently: `WindowTitle`, `CloseWindow`).
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    pub fn handle_side_effects(
        &mut self,
        egui_ctx: &egui::Context,
    ) -> Vec<SessionEffect> {
        let mut effects = Vec::new();

        // Buffer incoming title event (don't apply yet — wait for stability).
        if let Some(title) = self.terminal.take_title() {
            log::trace!("session: title event '{:?}' (debouncing)", title);
            self.pending_title = Some((title, Instant::now()));
        }

        // Apply pending title if it has been stable long enough.
        if let Some((title, at)) = &self.pending_title {
            if at.elapsed().as_secs_f64() * 1000.0 >= TITLE_DEBOUNCE_MS {
                if self.title != *title {
                    log::debug!("session: window title changed: {:?} -> {:?}", self.title, title);
                    self.title = title.clone();
                    effects.push(SessionEffect::WindowTitle(title.clone()));
                } else {
                    log::trace!("session: window title unchanged ({:?}), skipping", self.title);
                }
                self.pending_title = None;
            }
        }

        if self.terminal.take_bell() {
            log::debug!("update: bell");
            self.notification = super::types::NotificationState::Bell;
        }

        // ── Desktop notification (OSC 9 / OSC 777) ──────────────────────
        if let Some((title, body)) = self.terminal.take_notification() {
            log::info!("desktop notification: title={title:?} body={body:?}");
            // Spawn on a background thread to avoid conflicts with the
            // main thread's winit event loop (macOS UserNotifications
            // framework can trigger unexpected Cocoa dialog callbacks).
            std::thread::Builder::new()
                .name("notify".into())
                .spawn(move || {
                    if let Err(e) = notify_rust::Notification::new()
                        .summary(&title)
                        .body(&body)
                        .appname("Zenterm")
                        .show()
                    {
                        log::error!("failed to show desktop notification: {e}");
                    }
                })
                .ok();
        }

        if !self.exit_effect_sent {
            if self.terminal.take_exit() || self.terminal.take_child_exit().is_some() {
                log::info!("update: terminal requested exit, closing");
                self.pty_exited = true;
            }
            if self.pty_exited {
                log::info!("handle_side_effects: session exited, emitting CloseWindow");
                self.exit_effect_sent = true;
                effects.push(SessionEffect::CloseWindow);
            }
        }

        if let Some(text) = self.terminal.take_clipboard_store() {
            if let Ok(mut cb) = arboard::Clipboard::new() {
                if let Err(e) = cb.set_text(text) {
                    log::error!("failed to store clipboard text: {e}");
                }
            }
        }

        if let Some(formatter) = self.terminal.take_clipboard_load() {
            if let Ok(mut cb) = arboard::Clipboard::new() {
                match cb.get_text() {
                    Ok(text) => {
                        let seq = formatter(&text);
                        if let Err(e) = self.pty.write(seq.as_bytes()) {
                            log::error!("failed to write clipboard-load response: {e}");
                        }
                    }
                    Err(e) => {
                        log::error!("failed to read clipboard for terminal: {e}");
                    }
                }
            }
        }

        // ── OSC 7: working directory (current working directory URL) ──
        if let Some(url) = self.terminal.take_current_directory() {
            if let Some(path) = osc7_url_to_path(&url) {
                self.cwd = Some(path);
            }
        }

        let _ = egui_ctx; // kept for future per-session inputs
        effects
    }

    /// Send an SGR mouse event to the PTY.
    pub fn send_sgr_mouse(&mut self, row: usize, col: usize, button: u8, release: bool) {
        let mode = self.terminal.mode();
        let mouse_active = mode.contains(TermMode::SGR_MOUSE)
            && mode.intersects(
                TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION,
            );
        if !mouse_active {
            return;
        }
        let suffix = if release { "m" } else { "M" };
        let seq = format!("\x1b[{};{};{}{}", row + 1, col + 1, button, suffix);
        if let Err(e) = self.pty.write(seq.as_bytes()) {
            log::error!("SGR mouse write error: {e}");
        }
    }
}
