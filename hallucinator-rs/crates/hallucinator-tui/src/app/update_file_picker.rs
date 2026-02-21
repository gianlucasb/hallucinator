use super::{App, FilePickerContext, Screen};
use crate::action::Action;

impl App {
    /// Handle input while on the file picker screen.
    /// Returns true if the action was handled.
    pub(super) fn handle_file_picker_action(&mut self, action: Action) {
        match action {
            Action::Quit => {
                self.confirm_quit = true;
            }
            Action::NavigateBack => {
                match &self.file_picker_context {
                    FilePickerContext::SelectDatabase { config_item } => {
                        // In db mode: if a file was selected, write it to config
                        let config_item = *config_item;
                        if let Some(path) = self.file_picker.selected.first() {
                            let canonical = super::clean_canonicalize(path);
                            if config_item == 0 {
                                self.config_state.dblp_offline_path = canonical;
                            } else if config_item == 1 {
                                self.config_state.acl_offline_path = canonical;
                            } else {
                                self.config_state.openalex_offline_path = canonical;
                            }
                            self.config_state.dirty = true;
                        }
                        self.file_picker.selected.clear();
                        self.file_picker_context = FilePickerContext::AddFiles;
                        self.screen = Screen::Config;
                    }
                    FilePickerContext::AddFiles => {
                        // Normal mode: add any selected files, go back to queue
                        if !self.file_picker.selected.is_empty() {
                            self.add_files_from_picker();
                        }
                        self.screen = Screen::Queue;
                    }
                }
            }
            Action::MoveDown => {
                let max = self.file_picker.entries.len().saturating_sub(1);
                if self.file_picker.cursor < max {
                    self.file_picker.cursor += 1;
                }
            }
            Action::MoveUp => {
                self.file_picker.cursor = self.file_picker.cursor.saturating_sub(1);
            }
            Action::PageDown => {
                let page = self.visible_rows.max(1);
                let max = self.file_picker.entries.len().saturating_sub(1);
                self.file_picker.cursor = (self.file_picker.cursor + page).min(max);
            }
            Action::PageUp => {
                let page = self.visible_rows.max(1);
                self.file_picker.cursor = self.file_picker.cursor.saturating_sub(page);
            }
            Action::GoTop => {
                self.file_picker.cursor = 0;
            }
            Action::GoBottom => {
                self.file_picker.cursor = self.file_picker.entries.len().saturating_sub(1);
            }
            Action::ToggleSafe => {
                if let FilePickerContext::SelectDatabase { config_item } = self.file_picker_context
                {
                    // Single-select: .db files for DBLP/ACL, directories for OpenAlex
                    if let Some(entry) = self.file_picker.entries.get(self.file_picker.cursor) {
                        let selectable = if config_item == 2 {
                            entry.is_dir // OpenAlex index is a directory
                        } else {
                            entry.is_db
                        };
                        if selectable {
                            self.file_picker.selected.clear();
                            self.file_picker.selected.push(entry.path.clone());
                        }
                    }
                } else {
                    // Normal mode: toggle selection of current entry
                    self.file_picker.toggle_selected();
                }
            }
            Action::DrillIn => {
                if matches!(
                    self.file_picker_context,
                    FilePickerContext::SelectDatabase { .. }
                ) {
                    // In db mode: Enter on .db → select & return to config
                    if let Some(entry) = self
                        .file_picker
                        .entries
                        .get(self.file_picker.cursor)
                        .cloned()
                    {
                        if entry.is_dir {
                            self.file_picker.enter_directory();
                        } else if entry.is_db {
                            let canonical = super::clean_canonicalize(&entry.path);
                            if let FilePickerContext::SelectDatabase { config_item } =
                                self.file_picker_context
                            {
                                if config_item == 0 {
                                    self.config_state.dblp_offline_path = canonical;
                                } else if config_item == 1 {
                                    self.config_state.acl_offline_path = canonical;
                                } else {
                                    self.config_state.openalex_offline_path = canonical;
                                }
                                self.config_state.dirty = true;
                            }
                            self.file_picker.selected.clear();
                            self.file_picker_context = FilePickerContext::AddFiles;
                            self.screen = Screen::Config;
                        }
                    }
                } else {
                    // Normal mode: Enter on directory opens it, on file toggles selection
                    if !self.file_picker.enter_directory() {
                        self.file_picker.toggle_selected();
                    }
                }
            }
            Action::OpenConfig => {
                if self.screen != Screen::Config {
                    self.config_state.prev_screen = Some(self.screen.clone());
                }
                self.screen = Screen::Config;
            }
            Action::ToggleHelp => {
                self.show_help = true;
            }
            Action::Tick => {
                self.tick = self.tick.wrapping_add(1);
            }
            Action::Resize(_w, h) => {
                self.visible_rows = (h as usize).saturating_sub(11);
            }
            _ => {}
        }
    }
}
