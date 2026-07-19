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
        let batch = &mut self.batch_buf;
        batch.clear();
        if batch.capacity() < 65536 {
            batch.reserve(65536 - batch.capacity());
        }
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
            log::trace!("pump_pty: batching {} bytes from PTY", batch.len());
            let replies = self.terminal.feed(&batch);
            if !replies.is_empty() {
                log::trace!(
                    "pump_pty: writing {} reply bytes",
                    replies.len(),
                );
                if let Err(e) = self.pty.write(&replies) {
                    log::error!("failed to write pty reply: {e}");
                }
            }
            self.terminal_dirty = true;
        }

        // Drain Kitty OSC 99 notification responses (a=report, c=1,
        // button clicks) back to the PTY.
        while let Ok(resp) = self.notification_resp_rx.try_recv() {
            log::debug!("pump_pty: writing notification response: {resp}");
            if let Err(e) = self.pty.write(resp.as_bytes()) {
                log::error!("failed to write notification response: {e}");
            }
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
                self.seen_terminal_title = true;

                if title.is_empty() {
                    // Empty title → fallback to cwd basename.
                    // This matches Ghostty's behaviour: an empty OSC title
                    // sequence is treated as a reset, and we show the
                    // working directory name instead.
                    let fallback = self
                        .cwd
                        .as_ref()
                        .and_then(|p| p.file_name())
                        .and_then(|n| n.to_str())
                        .map(|s| s.to_string())
                        .unwrap_or_default();
                    if self.title != fallback {
                        log::debug!(
                            "session: empty title → fallback to cwd '{:?}'",
                            fallback,
                        );
                        self.title = fallback;
                        effects.push(SessionEffect::WindowTitle(self.title.clone()));
                    }
                } else if self.title != *title {
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

        // ── Desktop notification ────────────────────────────────────────
        // Three protocols feed into desktop notifications:
        //   OSC 99 (Kitty)     → richest metadata (urgency, icon, sound, …)
        //   OSC 9 (iTerm2)     → title + body only
        //   OSC 777 (rxvt)     → title + body only
        // Prefer the Kitty notification when available.
        let kitty_notif = self.terminal.take_kitty_notification();
        let basic_notif = self.terminal.take_notification();
        if let Some(kitty) = kitty_notif {
            log::info!("desktop notification (Kitty OSC 99): {:?}", kitty);

            // ── `o=` occasion filtering ──────────────────────────────
            let window_focused = egui_ctx.input(|i| i.viewport().focused).unwrap_or(true);
            let should_show = match kitty.occasion {
                zenterm_core::KittyOccasion::Always => true,
                zenterm_core::KittyOccasion::Unfocused => !window_focused,
                zenterm_core::KittyOccasion::Invisible => !window_focused || !self.tab_active,
            };
            if !should_show {
                log::debug!(
                    "suppressed Kitty notification (occasion={:?}, window_focused={}, tab_active={})",
                    kitty.occasion, window_focused, self.tab_active,
                );
            } else {
                let title = if kitty.title.is_empty() {
                    kitty.body.clone()
                } else {
                    kitty.title.clone()
                };
                let body = if kitty.title.is_empty() {
                    String::new()
                } else {
                    kitty.body.clone()
                };
                let app_name = kitty
                    .app_name
                    .clone()
                    .unwrap_or_else(|| "Zenterm".to_string());
                let icon_names = kitty.icon_names.clone();
                let icon_data = kitty.icon_data.clone();
                let buttons = kitty.buttons.clone();
                let timeout_ms = kitty.timeout_ms;
                let sound = kitty.sound.clone();
                let resp_tx = self.notification_resp_tx.clone();
                let notif_id = kitty.id.clone();
                let report_click = kitty.report_click;
                let close_report = kitty.close_report;
                std::thread::Builder::new()
                    .name("notify".into())
                    .spawn(move || {
                        let mut n = notify_rust::Notification::new();
                        n.summary(&title);
                        n.body(&body);
                        n.appname(&app_name);

                        // Icon: prefer icon_data (write temp file, works on all platforms),
                        // fall back to icon name (XDG only).
                        if !icon_data.is_empty() {
                            let tmp_dir = std::env::temp_dir();
                            let path = tmp_dir.join(format!("zenterm-icon-{}.png", std::process::id()));
                            if let Ok(mut file) = std::fs::File::create(&path) {
                                use std::io::Write;
                                if file.write_all(&icon_data).is_ok() {
                                    if let Some(p) = path.to_str() {
                                        n.image_path(p);
                                    }
                                }
                            }
                        } else if let Some(icon) = icon_names.first() {
                            n.icon(icon);
                        }

                        // Timeout.
                        match timeout_ms {
                            -1 => {} // system default
                            0 => { n.timeout(notify_rust::Timeout::Never); }
                            ms if ms > 0 => {
                                n.timeout(notify_rust::Timeout::Milliseconds(ms as u32));
                            }
                            _ => {}
                        }

                        // Sound (XDG only via Hint).
                        #[cfg(all(unix, not(target_os = "macos")))]
                        if let Some(ref name) = sound {
                            n.hint(notify_rust::Hint::Sound(name.clone()));
                        }
                        #[cfg(not(all(unix, not(target_os = "macos"))))]
                        let _ = sound;

                        // Buttons (XDG only via action).
                        #[cfg(all(unix, not(target_os = "macos")))]
                        for (i, label) in buttons.iter().enumerate() {
                            n.action(&format!("btn{}", i + 1), label);
                        }
                        #[cfg(not(all(unix, not(target_os = "macos"))))]
                        let _ = buttons;

                        // Show notification and handle callbacks.
                        if report_click || close_report {
                            #[cfg(all(unix, not(target_os = "macos")))]
                            {
                                if let Ok(mut handle) = n.show() {
                                    use notify_rust::ActionResponse;
                                    loop {
                                        match handle.wait_for_action() {
                                            ActionResponse::Closed(_reason) => {
                                                if close_report {
                                                    let id = notif_id.as_deref().unwrap_or("0");
                                                    let resp = format!("\x1b]99;i={}:p=close;\x1b\\\\", id);
                                                    let _ = resp_tx.send(resp);
                                                }
                                                break;
                                            }
                                            ActionResponse::Action(act) => {
                                                if report_click {
                                                    let id = notif_id.as_deref().unwrap_or("0");
                                                    if let Some(num) = act.strip_prefix("btn") {
                                                        let resp = format!("\x1b]99;i={};{}\x1b\\\\", id, num);
                                                        let _ = resp_tx.send(resp);
                                                    } else {
                                                        let resp = format!("\x1b]99;i={};\x1b\\\\", id);
                                                        let _ = resp_tx.send(resp);
                                                    }
                                                }
                                                break;
                                            }
                                            _ => break,
                                        }
                                    }
                                }
                            }
                            #[cfg(not(all(unix, not(target_os = "macos"))))]
                            {
                                let _ = n.show();
                                let _ = resp_tx;
                                let _ = notif_id;
                            }
                        } else {
                            if let Err(e) = n.show() {
                                log::error!("failed to show desktop notification: {e}");
                            }
                        }
                    })
                    .ok();
            }
        } else if let Some((title, body)) = basic_notif {
            log::info!("desktop notification (OSC 9/777): title={title:?} body={body:?}");
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

        // ── ConEmu progress bar (OSC 9;4) ─────────────────────────────
        if let Some(prog) = self.terminal.take_progress() {
            self.progress = prog;
        }

        // ── FinalTerm semantic prompt (OSC 133) ──────────────────────────
        if let Some(prompt) = self.terminal.take_semantic_prompt() {
            log::trace!("session: OSC 133 semantic prompt: {prompt:?}");
            self.latest_semantic_prompt = Some(prompt);
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

        // ── OSC 1337 (iTerm2 proprietary) actions ───────────────────
        // Consume navigation marks first (stored directly in terminal layer).
        let marks = self.terminal.take_marks();
        for (col, line) in &marks {
            log::info!("session: mark placed at ({col}, {line})");
        }

        if let Some(action) = self.terminal.take_iterm_action() {
            log::debug!("session: OSC 1337 action: {action:?}");
            match action {
                zenterm_core::ITermProprietary::StealFocus => {
                    effects.push(SessionEffect::StealFocus);
                }
                zenterm_core::ITermProprietary::SetProfile(name) => {
                    // zenterm does not have a named-profile system yet.
                    log::info!("session: profile change requested: {name}");
                }
                zenterm_core::ITermProprietary::HighlightCursorLine(enabled) => {
                    self.highlight_cursor_line = enabled;
                }
                zenterm_core::ITermProprietary::SetBadgeFormat(fmt) => {
                    self.badge_format = Some(fmt);
                }
                zenterm_core::ITermProprietary::ReportVariable(name) => {
                    // The terminal layer already handles lookup and response.
                    log::debug!("session: report variable: {name}");
                }
                zenterm_core::ITermProprietary::File(file) => {
                    // File download: sanitise the filename to prevent path traversal.
                    let raw_name = file
                        .name
                        .clone()
                        .unwrap_or_else(|| "iterm2_download".to_string());
                    // Strip any directory components — keep only the filename.
                    let fname = std::path::Path::new(&raw_name)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("iterm2_download")
                        .to_string();
                    let path = std::path::PathBuf::from(&fname);
                    log::info!(
                        "session: saving {} byte file from OSC 1337 File to {}",
                        file.data.len(),
                        path.display(),
                    );
                    if let Err(e) = std::fs::write(&path, &file.data) {
                        log::error!("session: failed to save downloaded file: {e}");
                    }
                }
                _ => {
                    // All other variants are handled directly in the
                    // terminal layer (ClearScrollback, CurrentDir, Copy,
                    // SetUserVar, RequestCellSize, UnicodeVersion).
                }
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
            log::info!(
                "[dbg] pty::send_sgr_mouse: mouse not active, discarding seq button={}",
                button,
            );
            return;
        }
        let suffix = if release { "m" } else { "M" };
        // SGR format:  CSI < Cb ; Cx ; Cy M/m
        //              button ; column ; row
        let seq = format!("\x1b[<{};{};{}{}", button, col + 1, row + 1, suffix);
        log::info!("[dbg] pty: writing SGR seq: {:?}", seq.as_bytes());
        if let Err(e) = self.pty.write(seq.as_bytes()) {
            log::error!("SGR mouse write error: {e}");
        }
    }
}
