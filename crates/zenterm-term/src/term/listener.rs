//! Event listener that bridges [`alacritty_terminal::event::Event`]s from the
//! VT handler into an `mpsc` channel consumed by [`Terminal`](super::Terminal).

use std::sync::mpsc;

use alacritty_terminal::event::{Event, EventListener};

/// Collects [`Event`]s from the alacritty `Handler` via an `mpsc` channel.
///
/// The channel receiver lives in [`Terminal`] and is drained during
/// [`Terminal::feed()`] so that response bytes can be written back to the
/// PTY and other side-effects (title changes, clipboard operations, bell,
/// exit) can be handled by the application.
pub(crate) struct Listener {
    pub(crate) tx: mpsc::Sender<Event>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        if self.tx.send(event).is_err() {
            log::warn!("Terminal event channel closed, dropping event");
        }
    }
}
