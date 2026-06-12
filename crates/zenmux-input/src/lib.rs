//! Keyboard input encoding.
//!
//! Maps egui keyboard events into the byte sequences that the shell
//! expects (Phase 1: ASCII, arrows, Ctrl combos).

/// Maps egui input events to terminal byte sequences.
pub struct InputMapper;

impl InputMapper {
    /// Convert an [`egui::Event`] into bytes to write to the PTY.
    ///
    /// Returns `None` if the event should be ignored (e.g. it was already
    /// consumed by egui).
    pub fn map(event: &egui::Event) -> Option<Vec<u8>> {
        match event {
            egui::Event::Text(text) => {
                // Printable text — send the UTF-8 bytes directly.
                if text.is_empty() {
                    return None;
                }
                // Filter out control characters that egui sometimes
                // delivers as text.
                let has_printable = text.chars().any(|c| !c.is_control() && c != '\n' && c != '\r');
                if !has_printable {
                    return None;
                }
                Some(text.as_bytes().to_vec())
            }

            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                let ctrl = modifiers.ctrl;
                let alt = modifiers.alt;
                let shift = modifiers.shift;

                match key {
                    egui::Key::Enter => Some(vec![b'\r']),
                    egui::Key::Backspace => Some(vec![0x7f]),
                    egui::Key::Tab => {
                        if shift {
                            Some(vec![0x1b, b'Z']) // ESC Z = backward tab
                        } else {
                            Some(vec![b'\t'])
                        }
                    }
                    egui::Key::Escape => Some(vec![0x1b]),

                    // Arrow keys
                    egui::Key::ArrowUp => Some(b"\x1b[A".to_vec()),
                    egui::Key::ArrowDown => Some(b"\x1b[B".to_vec()),
                    egui::Key::ArrowRight => Some(b"\x1b[C".to_vec()),
                    egui::Key::ArrowLeft => Some(b"\x1b[D".to_vec()),

                    // Home / End
                    egui::Key::Home => Some(b"\x1b[H".to_vec()),
                    egui::Key::End => Some(b"\x1b[F".to_vec()),

                    // Page Up / Down
                    egui::Key::PageUp => Some(b"\x1b[5~".to_vec()),
                    egui::Key::PageDown => Some(b"\x1b[6~".to_vec()),

                    // Delete
                    egui::Key::Delete => Some(b"\x1b[3~".to_vec()),

                    // Insert
                    egui::Key::Insert => Some(b"\x1b[2~".to_vec()),

                    // Ctrl+letter → control codes 0x01–0x1a
                    _ => {
                        if ctrl && !alt {
                            if let Some(code) = key_to_ctrl_code(key) {
                                Some(vec![code])
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }
                }
            }

            _ => None,
        }
    }
}

/// Map an egui `Key` to its Ctrl+key control code (0x01–0x1a).
fn key_to_ctrl_code(key: &egui::Key) -> Option<u8> {
    match key {
        egui::Key::A => Some(0x01),
        egui::Key::B => Some(0x02),
        egui::Key::C => Some(0x03),
        egui::Key::D => Some(0x04),
        egui::Key::E => Some(0x05),
        egui::Key::F => Some(0x06),
        egui::Key::G => Some(0x07),
        egui::Key::H => Some(0x08), // Backspace
        egui::Key::I => Some(0x09), // Tab
        egui::Key::J => Some(0x0a), // Line feed
        egui::Key::K => Some(0x0b),
        egui::Key::L => Some(0x0c),
        egui::Key::M => Some(0x0d), // Carriage return
        egui::Key::N => Some(0x0e),
        egui::Key::O => Some(0x0f),
        egui::Key::P => Some(0x10),
        egui::Key::Q => Some(0x11),
        egui::Key::R => Some(0x12),
        egui::Key::S => Some(0x13),
        egui::Key::T => Some(0x14),
        egui::Key::U => Some(0x15),
        egui::Key::V => Some(0x16),
        egui::Key::W => Some(0x17),
        egui::Key::X => Some(0x18),
        egui::Key::Y => Some(0x19),
        egui::Key::Z => Some(0x1a),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enter() {
        let event = egui::Event::Key {
            key: egui::Key::Enter,
            pressed: true,
            modifiers: egui::Modifiers::default(),
            repeat: false,
            physical_key: None,
        };
        assert_eq!(InputMapper::map(&event), Some(vec![b'\r']));
    }

    #[test]
    fn test_ctrl_c() {
        let event = egui::Event::Key {
            key: egui::Key::C,
            pressed: true,
            modifiers: egui::Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
            physical_key: None,
        };
        assert_eq!(InputMapper::map(&event), Some(vec![0x03]));
    }

    #[test]
    fn test_arrow_up() {
        let event = egui::Event::Key {
            key: egui::Key::ArrowUp,
            pressed: true,
            modifiers: egui::Modifiers::default(),
            repeat: false,
            physical_key: None,
        };
        assert_eq!(InputMapper::map(&event), Some(b"\x1b[A".to_vec()));
    }
}
