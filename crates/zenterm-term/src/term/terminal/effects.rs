//! Terminal state accessors and externally visible side effects.
//!
//! VT protocol handling records effects while processing input. This module
//! owns the public API used by the UI to drain those effects and inspect
//! terminal state.

use std::collections::HashMap;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

use zenterm_core::color::Rgba;
use zenterm_core::damage::DamageSet;
use zenterm_core::position::TermPos;
use zenterm_core::{ITermProprietary, KittyNotification, Progress, SemanticPrompt};

use super::super::color_scheme::ColorScheme;
use super::super::grid_view::CursorInfo;
use super::{ClipboardLoad, Terminal};

impl Terminal {
    pub fn drain_damage(&mut self) -> DamageSet {
        let mut ds = DamageSet::new(self.term.screen_lines());
        std::mem::swap(&mut ds, &mut self.damage);
        ds
    }

    pub fn cursor(&self) -> CursorInfo {
        let point = self.term.grid().cursor.point;
        let display_offset = self.term.grid().display_offset();
        let viewport_line = point.line.0 + display_offset as i32;
        CursorInfo {
            pos: TermPos::new(viewport_line.max(0) as usize, point.column.0),
            style: self.term.cursor_style(),
            visible: self.term.mode().contains(TermMode::SHOW_CURSOR),
            cursor_bg: self.scheme.cursor_bg,
            cursor_fg: self.scheme.cursor_fg,
        }
    }

    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    pub fn kitty_keyboard_flags(&self) -> Option<zenterm_input::KittyKeyboardFlags> {
        zenterm_input::KittyKeyboardFlags::from_term_mode(self.term.mode().bits())
    }

    pub fn set_scheme(&mut self, scheme: ColorScheme) {
        self.scheme = scheme;
        self.damage.mark_all();
    }

    pub fn scheme(&self) -> &ColorScheme {
        &self.scheme
    }

    pub fn set_cursor_color(&mut self, rgb: Rgb) {
        self.scheme.set_cursor_color(rgb);
        self.damage.mark_all();
    }

    pub fn reset_cursor_color(&mut self) {
        self.scheme.reset_cursor_color();
        self.damage.mark_all();
    }

    pub fn take_title(&mut self) -> Option<String> {
        self.pending_title.take()
    }

    pub fn take_bell(&mut self) -> bool {
        let val = self.pending_bell;
        self.pending_bell = false;
        val
    }

    pub fn take_exit(&mut self) -> bool {
        let val = self.pending_exit;
        self.pending_exit = false;
        val
    }

    pub fn take_child_exit(&mut self) -> Option<std::process::ExitStatus> {
        self.pending_child_exit.take()
    }

    pub fn take_clipboard_store(&mut self) -> Option<String> {
        self.pending_clipboard_store.take()
    }

    pub fn take_notification(&mut self) -> Option<(String, String)> {
        self.pending_notification.take()
    }

    pub fn take_progress(&mut self) -> Option<Progress> {
        self.pending_progress.take()
    }

    pub fn take_semantic_prompt(&mut self) -> Option<SemanticPrompt> {
        self.pending_semantic_prompt.take()
    }

    pub fn take_kitty_notification(&mut self) -> Option<KittyNotification> {
        self.pending_kitty_notification.take()
    }

    pub fn take_iterm_action(&mut self) -> Option<ITermProprietary> {
        self.pending_iterm_action.take()
    }

    pub fn user_vars(&self) -> &HashMap<String, String> {
        &self.user_vars
    }

    pub fn take_marks(&mut self) -> Vec<(usize, usize)> {
        std::mem::take(&mut self.marks)
    }

    pub fn take_current_directory(&mut self) -> Option<String> {
        self.pending_current_directory.take()
    }

    pub fn take_clipboard_load(&mut self) -> Option<ClipboardLoad> {
        self.pending_clipboard_load.take()
    }

    pub fn default_bg(&self) -> Rgba {
        self.resolve_color(Color::Named(NamedColor::Background))
    }
}
