//! VS Code-style command palette for the main Zenterm viewport.
//!
//! The palette deliberately keeps its command list in the UI crate.  These
//! commands are app-level actions, so they must be resolved before keyboard
//! events are forwarded to the active PTY session.

use egui::{Align2, Color32, Context, FontId, Id, Key, Modal, ScrollArea, TextEdit};

use super::ZentermApp;
use crate::workspace::WorkspaceId;

/// Persistent interaction state for the command palette.
#[derive(Default)]
pub(crate) struct CommandPaletteState {
    pub open: bool,
    pub query: String,
    pub selected: usize,
    focus_input: bool,
}

#[derive(Clone, Copy)]
enum CommandAction {
    NewTerminal,
    NewWorkspace,
    OpenSettings,
    ReloadConfig,
    CloseActiveTab,
    RenameActiveTab,
    CloseActiveWorkspace,
    SwitchWorkspace(WorkspaceId),
    CycleWorkspace(isize),
    ToggleSidebar,
    ToggleTabs,
}

struct CommandItem {
    title: String,
    category: &'static str,
    shortcut: Option<&'static str>,
    action: CommandAction,
}

struct MatchedCommand {
    item: CommandItem,
    score: i32,
}

impl ZentermApp {
    /// Open the palette and put the search field into a known initial state.
    pub(crate) fn open_command_palette(&mut self) {
        self.command_palette.open = true;
        self.command_palette.query.clear();
        self.command_palette.selected = 0;
        self.command_palette.focus_input = true;
    }

    fn close_command_palette(&mut self) {
        self.command_palette = CommandPaletteState::default();
    }

    fn command_items(&self) -> Vec<CommandItem> {
        let mut items = Vec::new();

        if self.config.ui.tabs_enabled {
            items.push(CommandItem {
                title: "New Terminal".into(),
                category: "Terminal",
                shortcut: None,
                action: CommandAction::NewTerminal,
            });
            items.push(CommandItem {
                title: "New Workspace".into(),
                category: "Workspace",
                shortcut: None,
                action: CommandAction::NewWorkspace,
            });

            if self.active_session_id.is_some() {
                items.push(CommandItem {
                    title: "Close Active Tab".into(),
                    category: "Terminal",
                    shortcut: None,
                    action: CommandAction::CloseActiveTab,
                });
                items.push(CommandItem {
                    title: "Rename Active Tab".into(),
                    category: "Terminal",
                    shortcut: None,
                    action: CommandAction::RenameActiveTab,
                });
            }

            if self.workspaces.workspaces.len() > 1 {
                items.push(CommandItem {
                    title: "Next Workspace".into(),
                    category: "Workspace",
                    shortcut: Some("Ctrl+Tab"),
                    action: CommandAction::CycleWorkspace(1),
                });
                items.push(CommandItem {
                    title: "Previous Workspace".into(),
                    category: "Workspace",
                    shortcut: Some("Ctrl+Shift+Tab"),
                    action: CommandAction::CycleWorkspace(-1),
                });
                items.push(CommandItem {
                    title: "Close Active Workspace".into(),
                    category: "Workspace",
                    shortcut: None,
                    action: CommandAction::CloseActiveWorkspace,
                });
            }

            for (index, workspace) in self.workspaces.workspaces.iter().enumerate() {
                let shortcut = match index {
                    0 => Some("Ctrl+1"),
                    1 => Some("Ctrl+2"),
                    2 => Some("Ctrl+3"),
                    3 => Some("Ctrl+4"),
                    4 => Some("Ctrl+5"),
                    5 => Some("Ctrl+6"),
                    6 => Some("Ctrl+7"),
                    7 => Some("Ctrl+8"),
                    8 => Some("Ctrl+9"),
                    _ => None,
                };
                items.push(CommandItem {
                    title: format!("Switch to Workspace: {}", workspace.name),
                    category: "Workspace",
                    shortcut,
                    action: CommandAction::SwitchWorkspace(workspace.id),
                });
            }

        }

        // View toggles for sidebar and tab bar (always accessible)
        let sidebar_title = if self.config.ui.sidebar_enabled {
            "Hide Sidebar"
        } else {
            "Show Sidebar"
        };
        items.push(CommandItem {
            title: sidebar_title.into(),
            category: "View",
            shortcut: None,
            action: CommandAction::ToggleSidebar,
        });

        let tabs_title = if self.config.ui.tabs_enabled {
            "Hide Tab Bar"
        } else {
            "Show Tab Bar"
        };
        items.push(CommandItem {
            title: tabs_title.into(),
            category: "View",
            shortcut: None,
            action: CommandAction::ToggleTabs,
        });

        items.push(CommandItem {
            title: "Open Settings".into(),
            category: "Application",
            shortcut: Some("Ctrl+,"),
            action: CommandAction::OpenSettings,
        });
        items.push(CommandItem {
            title: "Reload Configuration".into(),
            category: "Application",
            shortcut: Some("Ctrl+Shift+R"),
            action: CommandAction::ReloadConfig,
        });

        items
    }

    fn matched_commands(&self, query: &str) -> Vec<MatchedCommand> {
        let mut matches: Vec<_> = self
            .command_items()
            .into_iter()
            .filter_map(|item| {
                let searchable = format!("{} {}", item.category, item.title);
                fuzzy_score(query, &searchable).map(|score| MatchedCommand { item, score })
            })
            .collect();

        matches.sort_by_key(|matched| std::cmp::Reverse(matched.score));
        matches
    }

    /// Render the palette above the terminal content.
    pub(crate) fn render_command_palette(&mut self, ctx: &Context) {
        if !self.command_palette.open {
            return;
        }

        let mut query = self.command_palette.query.clone();
        let matches = self.matched_commands(&query);
        let mut selected = self
            .command_palette
            .selected
            .min(matches.len().saturating_sub(1));
        let focus_input = self.command_palette.focus_input;
        let mut chosen: Option<CommandAction> = None;

        let response = Modal::new(Id::new("zenterm_command_palette"))
            .area(
                Modal::default_area(Id::new("zenterm_command_palette"))
                    .anchor(Align2::CENTER_TOP, egui::vec2(0.0, 72.0)),
            )
            .backdrop_color(Color32::from_black_alpha(120))
            .show(ctx, |ui| {
                ui.set_min_width(620.0);
                ui.set_max_width(760.0);

                let input_id = Id::new("zenterm_command_palette_input");
                let input_response = ui.add(
                    TextEdit::singleline(&mut query)
                        .id(input_id)
                        .desired_width(f32::INFINITY)
                        .min_size(egui::vec2(0.0, 34.0))
                        .vertical_align(egui::Align::Center)
                        .font(FontId::proportional(18.0))
                        .hint_text("Type a command to search…"),
                );
                if focus_input {
                    ui.memory_mut(|memory| memory.request_focus(input_id));
                }

                ui.add_space(8.0);

                if matches.is_empty() {
                    ui.add_space(6.0);
                    ui.colored_label(ui.visuals().weak_text_color(), "No matching commands");
                    ui.add_space(6.0);
                } else {
                    ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                        for (index, matched) in matches.iter().enumerate() {
                            let is_selected = index == selected;
                            let mut button =
                                egui::Button::selectable(is_selected, &matched.item.title)
                                    .min_size(egui::vec2(ui.available_width(), 32.0));
                            if let Some(shortcut) = matched.item.shortcut {
                                button = button.shortcut_text(shortcut);
                            }
                            if ui.add(button).clicked() {
                                chosen = Some(matched.item.action);
                            }
                        }
                    });
                }

                ui.add_space(8.0);
                // The bundled proportional font does not contain the arrow
                // glyphs on every platform, which would render them as tofu
                // boxes.  ASCII keeps the keyboard hint portable.
                ui.weak("Up/Down Select    Enter Run    Esc Close");

                // Keep the query field focused after mouse interaction and
                // prevent its caret navigation from changing the selection.
                if input_response.clicked() {
                    ui.memory_mut(|memory| memory.request_focus(input_id));
                }

                if ui.input(|input| input.key_pressed(Key::ArrowDown)) {
                    if !matches.is_empty() {
                        selected = (selected + 1) % matches.len();
                    }
                    ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::ArrowDown));
                }
                if ui.input(|input| input.key_pressed(Key::ArrowUp)) {
                    if !matches.is_empty() {
                        selected = selected.checked_sub(1).unwrap_or(matches.len() - 1);
                    }
                    ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::ArrowUp));
                }
                if ui.input(|input| input.key_pressed(Key::Enter)) && !matches.is_empty() {
                    chosen = Some(matches[selected].item.action);
                    ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Enter));
                }
            });

        self.command_palette.query = query;
        self.command_palette.selected = selected;
        self.command_palette.focus_input = false;

        if response.should_close() {
            self.close_command_palette();
        } else if let Some(action) = chosen {
            self.execute_command(action, ctx);
        }
    }

    fn execute_command(&mut self, action: CommandAction, ctx: &Context) {
        self.close_command_palette();

        match action {
            CommandAction::NewTerminal => {
                self.spawn_session();
            }
            CommandAction::NewWorkspace => {
                let active_session = self.active_session_id.and_then(|id| self.sessions.get(&id));
                let name = Self::generate_workspace_name(&self.workspaces, active_session);
                self.workspaces.create_workspace(name);
                self.spawn_session();
            }
            CommandAction::OpenSettings => {
                self.settings_state.open = true;
                self.settings_state.reset_to(&self.config);
            }
            CommandAction::ReloadConfig => {
                self.reload_config(ctx);
            }
            CommandAction::CloseActiveTab => {
                if let Some(id) = self.active_session_id {
                    self.close_session(id);
                }
            }
            CommandAction::RenameActiveTab => {
                self.pending_rename = self.active_session_id;
            }
            CommandAction::CloseActiveWorkspace => {
                let workspace_id = self.workspaces.active_workspace_id;
                let sessions = self.workspaces.active_workspace().all_tab_ids();
                if self.workspaces.close_workspace(workspace_id) {
                    for id in sessions {
                        self.sessions.remove(&id);
                    }
                    self.focus_first_tab_in_active_workspace();
                    self.mark_layout_dirty();
                }
            }
            CommandAction::SwitchWorkspace(id) => {
                if self.workspaces.switch_to(id) {
                    self.focus_first_tab_in_active_workspace();
                    self.mark_layout_dirty();
                }
            }
            CommandAction::CycleWorkspace(direction) => {
                let len = self.workspaces.workspaces.len();
                if len > 0 {
                    let current_index = self
                        .workspaces
                        .workspaces
                        .iter()
                        .position(|workspace| workspace.id == self.workspaces.active_workspace_id)
                        .unwrap_or(0);
                    let next_index =
                        ((current_index as isize + direction).rem_euclid(len as isize)) as usize;
                    let id = self.workspaces.workspaces[next_index].id;
                    self.workspaces.switch_to(id);
                    self.focus_first_tab_in_active_workspace();
                    self.mark_layout_dirty();
                }
            }
            CommandAction::ToggleSidebar => {
                self.config.ui.sidebar_enabled = !self.config.ui.sidebar_enabled;
                if let Err(e) = self.config.save() {
                    log::warn!("Failed to save config after toggling sidebar: {e}");
                }
            }
            CommandAction::ToggleTabs => {
                self.config.ui.tabs_enabled = !self.config.ui.tabs_enabled;
                if let Err(e) = self.config.save() {
                    log::warn!("Failed to save config after toggling tabs: {e}");
                }
            }
        }
    }
}

/// Return a score when `query` is a case-insensitive subsequence of `text`.
/// Higher scores prefer prefixes, word boundaries, and contiguous matches.
fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    let query: Vec<char> = query.to_lowercase().chars().collect();
    if query.is_empty() {
        return Some(0);
    }

    let text: Vec<char> = text.to_lowercase().chars().collect();
    let mut text_index = 0;
    let mut previous_match = None;
    let mut score = 0;

    for query_char in query {
        let match_index = text
            .iter()
            .enumerate()
            .skip(text_index)
            .find_map(|(index, text_char)| (*text_char == query_char).then_some(index))?;

        score += 100 - match_index.min(100) as i32;
        if previous_match.is_some_and(|previous| match_index == previous + 1) {
            score += 30;
        }
        if match_index == 0
            || text[match_index - 1].is_whitespace()
            || matches!(text[match_index - 1], ':' | '-' | '_' | '/')
        {
            score += 20;
        }

        previous_match = Some(match_index);
        text_index = match_index + 1;
    }

    Some(score)
}

#[cfg(test)]
mod tests {
    use super::fuzzy_score;

    #[test]
    fn fuzzy_score_matches_subsequences() {
        assert!(fuzzy_score("nws", "New Workspace").is_some());
        assert!(fuzzy_score("CONFIG", "Reload Configuration").is_some());
        assert!(fuzzy_score("xyz", "New Terminal").is_none());
    }

    #[test]
    fn fuzzy_score_prefers_prefix_and_contiguous_matches() {
        let prefix = fuzzy_score("new", "New Terminal").unwrap();
        let scattered = fuzzy_score("new", "Now Enter Workspace").unwrap();
        assert!(prefix > scattered);
    }
}
