use std::path::PathBuf;
use std::time::Instant;

use super::{App, FilePickerContext, InputMode, Screen};
use crate::action::Action;
use crate::model::paper::{FpReason, RefPhase};
use crate::model::queue::PaperVerdict;
use crate::tui_event::BackendCommand;

impl App {
    /// Process a user action and update state. Returns true if the app should quit.
    pub fn update(&mut self, action: Action) -> bool {
        // Mark-safe propagation modal (#266): Space/Enter confirms the
        // sweep across the queue; Esc cancels (origin stays marked,
        // other papers untouched).
        if self.pending_propagation.is_some() {
            match action {
                Action::ToggleSafe | Action::SearchConfirm => {
                    self.confirm_pending_propagation();
                }
                Action::NavigateBack => {
                    self.cancel_pending_propagation();
                }
                Action::Quit => {
                    self.confirm_quit = true;
                }
                Action::Tick => {
                    self.tick = self.tick.wrapping_add(1);
                }
                Action::Resize(_w, h) => {
                    self.visible_rows = (h as usize).saturating_sub(11);
                }
                _ => {}
            }
            return false;
        }

        // Quit confirmation modal — q confirms, Esc cancels
        if self.confirm_quit {
            match action {
                Action::Quit => {
                    self.should_quit = true;
                    return true;
                }
                Action::NavigateBack => {
                    self.confirm_quit = false;
                }
                Action::Tick => {
                    self.tick = self.tick.wrapping_add(1);
                }
                Action::Resize(_w, h) => {
                    self.visible_rows = (h as usize).saturating_sub(11);
                }
                _ => {}
            }
            return false;
        }

        // Export modal intercepts — but not while the file picker
        // is open on top of it (issue #112 browse flow). Without
        // this guard, MoveDown/MoveUp would cycle the export modal
        // cursor instead of moving the file picker cursor, making
        // the picker appear frozen.
        if self.export_state.active && self.screen != Screen::FilePicker {
            // If editing path, handle text input
            if self.export_state.editing_path {
                match action {
                    Action::Quit => {
                        self.should_quit = true;
                        return true;
                    }
                    Action::SearchCancel => {
                        // Cancel path editing
                        self.export_state.editing_path = false;
                        self.input_mode = InputMode::Normal;
                    }
                    Action::SearchConfirm => {
                        // Confirm path edit
                        let buf = self.export_state.edit_buffer.clone();
                        if !buf.is_empty() {
                            self.export_state.output_path = buf;
                        }
                        self.export_state.editing_path = false;
                        self.input_mode = InputMode::Normal;
                    }
                    Action::SearchInput(ch) => {
                        if ch == '\x08' {
                            // Backspace: delete char before cursor
                            if self.export_state.edit_cursor > 0 {
                                let prev = self.export_state.edit_buffer
                                    [..self.export_state.edit_cursor]
                                    .char_indices()
                                    .next_back()
                                    .map(|(i, _)| i)
                                    .unwrap_or(0);
                                self.export_state
                                    .edit_buffer
                                    .drain(prev..self.export_state.edit_cursor);
                                self.export_state.edit_cursor = prev;
                            }
                        } else {
                            self.export_state
                                .edit_buffer
                                .insert(self.export_state.edit_cursor, ch);
                            self.export_state.edit_cursor += ch.len_utf8();
                        }
                    }
                    Action::CursorLeft => {
                        let cur = &mut self.export_state.edit_cursor;
                        *cur = self.export_state.edit_buffer[..*cur]
                            .char_indices()
                            .next_back()
                            .map(|(i, _)| i)
                            .unwrap_or(0);
                    }
                    Action::CursorRight => {
                        let cur = &mut self.export_state.edit_cursor;
                        if *cur < self.export_state.edit_buffer.len() {
                            *cur += self.export_state.edit_buffer[*cur..]
                                .chars()
                                .next()
                                .map(|c| c.len_utf8())
                                .unwrap_or(0);
                        }
                    }
                    Action::CursorHome => {
                        self.export_state.edit_cursor = 0;
                    }
                    Action::CursorEnd => {
                        self.export_state.edit_cursor = self.export_state.edit_buffer.len();
                    }
                    Action::DeleteForward => {
                        let cur = self.export_state.edit_cursor;
                        if cur < self.export_state.edit_buffer.len() {
                            let next = cur
                                + self.export_state.edit_buffer[cur..]
                                    .chars()
                                    .next()
                                    .map(|c| c.len_utf8())
                                    .unwrap_or(0);
                            self.export_state.edit_buffer.drain(cur..next);
                        }
                    }
                    Action::Tick => {
                        self.tick = self.tick.wrapping_add(1);
                    }
                    _ => {}
                }
                return false;
            }
            match action {
                Action::Quit => {
                    self.confirm_quit = true;
                }
                Action::NavigateBack => {
                    self.export_state.active = false;
                }
                Action::MoveDown => {
                    self.export_state.cursor = (self.export_state.cursor + 1).min(4);
                }
                Action::MoveUp => {
                    self.export_state.cursor = self.export_state.cursor.saturating_sub(1);
                }
                Action::BrowsePath => {
                    // Issue #112: when the user is on the path field,
                    // `.` opens the file picker in directory-select
                    // mode. The file picker, on confirm, rebuilds the
                    // output_path as `<picked_dir>/<filename_stem>`
                    // and restores this screen. Inline-edit (Enter on
                    // cursor=3) is still available for users who
                    // prefer typing the path directly.
                    if self.export_state.cursor == 3 {
                        // Extract just the filename part of the
                        // current output_path so we preserve the
                        // user's chosen stem across the browse
                        // round-trip.
                        let stem = std::path::Path::new(&self.export_state.output_path)
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_else(|| self.export_state.output_path.clone());
                        self.pre_file_picker_screen = Some(self.screen.clone());
                        self.file_picker_context = FilePickerContext::SelectExportDirectory {
                            filename_stem: stem,
                        };
                        self.screen = Screen::FilePicker;
                    }
                }
                Action::DrillIn => match self.export_state.cursor {
                    0 => {
                        let formats = crate::view::export::ExportFormat::all();
                        let idx = formats
                            .iter()
                            .position(|&f| f == self.export_state.format)
                            .unwrap_or(0);
                        self.export_state.format = formats[(idx + 1) % formats.len()];
                    }
                    1 => {
                        self.export_state.scope = self.export_state.scope.next();
                        self.export_state.output_path =
                            self.export_default_path(self.export_state.scope);
                    }
                    2 => {
                        // Toggle problematic-only filter
                        self.export_state.problematic_only = !self.export_state.problematic_only;
                    }
                    3 => {
                        // Start editing the output path
                        self.export_state.editing_path = true;
                        self.export_state.edit_buffer = self.export_state.output_path.clone();
                        self.export_state.edit_cursor = self.export_state.edit_buffer.len();
                        self.input_mode = InputMode::TextInput;
                    }
                    4 => {
                        let path = format!(
                            "{}.{}",
                            self.export_state.output_path,
                            self.export_state.format.extension()
                        );
                        let paper_indices = match self.export_state.scope {
                            crate::view::export::ExportScope::AllPapers => {
                                (0..self.papers.len()).collect::<Vec<_>>()
                            }
                            crate::view::export::ExportScope::ThisPaper => {
                                let idx = match self.screen {
                                    Screen::Paper(i) | Screen::RefDetail(i, _) => Some(i),
                                    Screen::Queue => {
                                        self.queue_sorted.get(self.queue_cursor).copied()
                                    }
                                    _ => None,
                                };
                                idx.map(|i| vec![i])
                                    .unwrap_or_else(|| (0..self.papers.len()).collect())
                            }
                            crate::view::export::ExportScope::ProblematicPapers => {
                                // Exclude papers where every problematic ref
                                // has been marked safe (fp_reason set). The
                                // raw `p.stats.*` counters don't decrement
                                // on fp_reason, so consult the actual ref
                                // states instead — `has_unresolved_problems`
                                // returns true only when at least one ref is
                                // still an unresolved problem.
                                (0..self.papers.len())
                                    .filter(|&i| {
                                        self.ref_states.get(i).is_some_and(|refs| {
                                            crate::model::paper::has_unresolved_problems(refs)
                                        })
                                    })
                                    .collect::<Vec<_>>()
                            }
                        };
                        // Build full results from ref_states for export
                        let results_vecs: Vec<Vec<Option<hallucinator_core::ValidationResult>>> =
                            paper_indices
                                .iter()
                                .map(|&i| {
                                    self.ref_states
                                        .get(i)
                                        .map(|refs| {
                                            refs.iter().map(|rs| rs.result.clone()).collect()
                                        })
                                        .unwrap_or_default()
                                })
                                .collect();
                        let report_papers: Vec<hallucinator_reporting::ReportPaper<'_>> =
                            paper_indices
                                .iter()
                                .zip(results_vecs.iter())
                                .filter_map(|(&i, results)| {
                                    let paper = self.papers.get(i)?;
                                    Some(hallucinator_reporting::ReportPaper {
                                        filename: &paper.filename,
                                        stats: &paper.stats,
                                        results,
                                        verdict: paper.verdict,
                                    })
                                })
                                .collect();
                        let report_refs: Vec<Vec<hallucinator_reporting::ReportRef>> =
                            paper_indices
                                .iter()
                                .map(|&i| {
                                    self.ref_states
                                        .get(i)
                                        .map(|refs| {
                                            refs.iter()
                                                .map(|rs| hallucinator_reporting::ReportRef {
                                                    index: rs.index,
                                                    title: rs.title.clone(),
                                                    skip_info: if let RefPhase::Skipped(reason) =
                                                        &rs.phase
                                                    {
                                                        Some(hallucinator_reporting::SkipInfo {
                                                            reason: reason.clone(),
                                                        })
                                                    } else {
                                                        None
                                                    },
                                                    fp_reason: rs.fp_reason,
                                                })
                                                .collect()
                                        })
                                        .unwrap_or_default()
                                })
                                .collect();
                        let ref_slices: Vec<&[hallucinator_reporting::ReportRef]> =
                            report_refs.iter().map(|v| v.as_slice()).collect();
                        match hallucinator_reporting::export_results(
                            &report_papers,
                            &ref_slices,
                            self.export_state.format,
                            std::path::Path::new(&path),
                            self.export_state.problematic_only,
                        ) {
                            Ok(()) => {
                                self.export_state.message = Some(format!("Saved to {}", path));
                            }
                            Err(e) => {
                                self.export_state.message = Some(format!("Error: {}", e));
                            }
                        }
                    }
                    _ => {}
                },
                Action::Tick => {
                    self.tick = self.tick.wrapping_add(1);
                }
                _ => {}
            }
            return false;
        }

        // Help overlay
        if self.show_help {
            match action {
                Action::Quit => {
                    self.confirm_quit = true;
                }
                Action::ToggleHelp | Action::NavigateBack => {
                    self.show_help = false;
                }
                Action::Tick => {
                    self.tick = self.tick.wrapping_add(1);
                }
                Action::Resize(_w, h) => {
                    self.visible_rows = (h as usize).saturating_sub(11);
                }
                _ => {}
            }
            return false;
        }

        // Config "unsaved changes" prompt
        if self.config_state.confirm_exit {
            match action {
                Action::Quit => {
                    self.should_quit = true;
                    return true;
                }
                // y key (mapped to CopyToClipboard in normal mode) = save & exit
                Action::CopyToClipboard => {
                    self.save_config();
                    self.config_state.confirm_exit = false;
                    if let Some(prev) = self.config_state.prev_screen.clone() {
                        self.screen = prev;
                    } else {
                        self.screen = Screen::Queue;
                    }
                }
                // n key (mapped to NextMatch in normal mode) = discard & exit
                Action::NextMatch => {
                    self.config_state.confirm_exit = false;
                    self.config_state.dirty = false;
                    if let Some(prev) = self.config_state.prev_screen.clone() {
                        self.screen = prev;
                    } else {
                        self.screen = Screen::Queue;
                    }
                }
                // Esc = cancel, stay on config
                Action::NavigateBack => {
                    self.config_state.confirm_exit = false;
                }
                Action::Tick => {
                    self.tick = self.tick.wrapping_add(1);
                }
                Action::Resize(_w, h) => {
                    self.visible_rows = (h as usize).saturating_sub(11);
                }
                _ => {}
            }
            return false;
        }

        // Banner screen input handling
        if self.screen == Screen::Banner {
            let elapsed = self
                .banner_start
                .map(|s| s.elapsed())
                .unwrap_or(std::time::Duration::ZERO);

            match action {
                Action::Quit => {
                    self.should_quit = true;
                    return true;
                }
                Action::Tick => {
                    self.tick = self.tick.wrapping_add(1);
                    // hacker/modern: auto-dismiss after 2 seconds
                    // T-800: interactive (user presses Enter), no auto-dismiss
                    if !self.theme.is_t800() && elapsed >= std::time::Duration::from_secs(2) {
                        self.dismiss_banner();
                    }
                }
                Action::Resize(_w, h) => {
                    self.visible_rows = (h as usize).saturating_sub(11);
                }
                Action::None => {}
                _ => {
                    if self.theme.is_t800() {
                        // During boot phases (< 3.5s), any key skips to phase 4 (awaiting)
                        // At phase 4 (>= 3.5s), any key dismisses
                        if elapsed >= std::time::Duration::from_millis(3500) {
                            self.dismiss_banner();
                        } else {
                            // Skip to phase 4 by rewinding banner_start
                            self.banner_start =
                                Some(Instant::now() - std::time::Duration::from_millis(3500));
                        }
                    } else {
                        self.dismiss_banner();
                    }
                }
            }
            return false;
        }

        // File picker screen
        if self.screen == Screen::FilePicker {
            self.handle_file_picker_action(action);
            return false;
        }

        match action {
            Action::Quit => {
                self.confirm_quit = true;
            }
            Action::ToggleHelp => {
                self.show_help = true;
            }
            Action::NavigateBack => match &self.screen {
                Screen::RefDetail(paper_idx, _) => {
                    let paper_idx = *paper_idx;
                    self.screen = Screen::Paper(paper_idx);
                }
                Screen::Paper(paper_idx) => {
                    if !self.single_paper_mode {
                        let paper_idx = *paper_idx;
                        self.screen = Screen::Queue;
                        self.paper_cursor = 0;
                        // Restore cursor to the same paper even if sort order changed
                        self.queue_cursor = self
                            .queue_sorted
                            .iter()
                            .position(|&i| i == paper_idx)
                            .unwrap_or(
                                self.queue_cursor
                                    .min(self.queue_sorted.len().saturating_sub(1)),
                            );
                    }
                }
                Screen::Queue => {
                    if !self.search_query.is_empty() {
                        self.search_query.clear();
                        self.recompute_sorted_indices();
                    }
                }
                Screen::Config => {
                    // Clean up any in-progress editing
                    self.config_state.editing = false;
                    self.config_state.edit_buffer.clear();
                    self.config_state.edit_cursor = 0;
                    self.input_mode = InputMode::Normal;

                    if self.config_state.dirty && !self.config_state.confirm_exit {
                        // Show "unsaved changes" prompt instead of exiting
                        self.config_state.confirm_exit = true;
                    } else {
                        self.config_state.confirm_exit = false;
                        if let Some(prev) = self.config_state.prev_screen.clone() {
                            self.screen = prev;
                        } else {
                            self.screen = Screen::Queue;
                        }
                    }
                }
                Screen::Banner | Screen::FilePicker => {}
            },
            Action::DrillIn => match &self.screen {
                Screen::Queue => {
                    if self.queue_cursor < self.queue_sorted.len() {
                        let paper_idx = self.queue_sorted[self.queue_cursor];
                        self.screen = Screen::Paper(paper_idx);
                        self.paper_cursor = 0;
                    }
                }
                Screen::Paper(idx) => {
                    let idx = *idx;
                    let indices = self.paper_ref_indices(idx);
                    if self.paper_cursor < indices.len() {
                        let ref_idx = indices[self.paper_cursor];
                        self.detail_scroll = 0;
                        self.screen = Screen::RefDetail(idx, ref_idx);
                    }
                }
                Screen::Config => {
                    // Enter on config: start editing the current field
                    self.handle_config_enter();
                }
                Screen::RefDetail(..) | Screen::Banner | Screen::FilePicker => {}
            },
            Action::MoveDown => match &self.screen {
                Screen::Queue => {
                    if self.queue_cursor + 1 < self.queue_sorted.len() {
                        self.queue_cursor += 1;
                    }
                }
                Screen::Paper(idx) => {
                    let max = self.paper_ref_indices(*idx).len().saturating_sub(1);
                    if self.paper_cursor < max {
                        self.paper_cursor += 1;
                    }
                }
                Screen::RefDetail(..) => {
                    self.detail_scroll = self.detail_scroll.saturating_add(1);
                }
                Screen::Config => {
                    let max = self.config_section_item_count().saturating_sub(1);
                    if self.config_state.item_cursor < max {
                        self.config_state.item_cursor += 1;
                    }
                }
                Screen::Banner | Screen::FilePicker => {}
            },
            Action::MoveUp => match &self.screen {
                Screen::Queue => {
                    self.queue_cursor = self.queue_cursor.saturating_sub(1);
                }
                Screen::Paper(_) => {
                    self.paper_cursor = self.paper_cursor.saturating_sub(1);
                }
                Screen::RefDetail(..) => {
                    self.detail_scroll = self.detail_scroll.saturating_sub(1);
                }
                Screen::Config => {
                    self.config_state.item_cursor = self.config_state.item_cursor.saturating_sub(1);
                }
                Screen::Banner | Screen::FilePicker => {}
            },
            Action::PageDown => {
                let page = self.visible_rows.max(1);
                match &self.screen {
                    Screen::Queue => {
                        self.queue_cursor = (self.queue_cursor + page)
                            .min(self.queue_sorted.len().saturating_sub(1));
                    }
                    Screen::Paper(idx) => {
                        let max = self.paper_ref_indices(*idx).len().saturating_sub(1);
                        self.paper_cursor = (self.paper_cursor + page).min(max);
                    }
                    Screen::RefDetail(..) => {
                        self.detail_scroll = self.detail_scroll.saturating_add(page as u16);
                    }
                    Screen::Config | Screen::Banner | Screen::FilePicker => {}
                }
            }
            Action::PageUp => {
                let page = self.visible_rows.max(1);
                match &self.screen {
                    Screen::Queue => {
                        self.queue_cursor = self.queue_cursor.saturating_sub(page);
                    }
                    Screen::Paper(_) => {
                        self.paper_cursor = self.paper_cursor.saturating_sub(page);
                    }
                    Screen::RefDetail(..) => {
                        self.detail_scroll = self.detail_scroll.saturating_sub(page as u16);
                    }
                    Screen::Config | Screen::Banner | Screen::FilePicker => {}
                }
            }
            Action::GoTop => match &self.screen {
                Screen::Queue => self.queue_cursor = 0,
                Screen::Paper(_) => self.paper_cursor = 0,
                Screen::RefDetail(..) => self.detail_scroll = 0,
                Screen::Config => self.config_state.item_cursor = 0,
                Screen::Banner | Screen::FilePicker => {}
            },
            Action::GoBottom => match &self.screen {
                Screen::Queue => {
                    self.queue_cursor = self.queue_sorted.len().saturating_sub(1);
                }
                Screen::Paper(idx) => {
                    self.paper_cursor = self.paper_ref_indices(*idx).len().saturating_sub(1);
                }
                Screen::RefDetail(..) => {
                    self.detail_scroll = u16::MAX;
                }
                Screen::Config => {
                    self.config_state.item_cursor =
                        self.config_section_item_count().saturating_sub(1);
                }
                Screen::Banner | Screen::FilePicker => {}
            },
            Action::CycleSort => match &self.screen {
                Screen::Queue => {
                    self.sort_order = self.sort_order.next();
                    self.sort_reversed = false;
                    self.recompute_sorted_indices();
                }
                Screen::Paper(_) => {
                    self.paper_sort = self.paper_sort.next();
                }
                _ => {}
            },
            Action::ReverseSortDirection => {
                if self.screen == Screen::Queue {
                    self.sort_reversed = !self.sort_reversed;
                    self.recompute_sorted_indices();
                }
            }
            Action::CycleFilter => match &self.screen {
                Screen::Queue => {
                    self.queue_filter = self.queue_filter.next();
                    self.recompute_sorted_indices();
                    self.queue_cursor = 0;
                }
                Screen::Paper(_) => {
                    self.paper_filter = self.paper_filter.next();
                    self.paper_cursor = 0;
                }
                _ => {}
            },
            Action::StartSearch => {
                self.input_mode = InputMode::Search;
                self.search_query.clear();
            }
            Action::CursorLeft
            | Action::CursorRight
            | Action::CursorHome
            | Action::CursorEnd
            | Action::DeleteForward => {
                if self.config_state.editing {
                    let buf = &mut self.config_state.edit_buffer;
                    let cur = &mut self.config_state.edit_cursor;
                    match action {
                        Action::CursorLeft => {
                            // Move to previous char boundary
                            *cur = buf[..*cur]
                                .char_indices()
                                .next_back()
                                .map(|(i, _)| i)
                                .unwrap_or(0);
                        }
                        Action::CursorRight => {
                            // Move to next char boundary
                            if *cur < buf.len() {
                                *cur += buf[*cur..]
                                    .chars()
                                    .next()
                                    .map(|c| c.len_utf8())
                                    .unwrap_or(0);
                            }
                        }
                        Action::CursorHome => *cur = 0,
                        Action::CursorEnd => *cur = buf.len(),
                        Action::DeleteForward => {
                            if *cur < buf.len() {
                                let next = *cur
                                    + buf[*cur..]
                                        .chars()
                                        .next()
                                        .map(|c| c.len_utf8())
                                        .unwrap_or(0);
                                buf.drain(*cur..next);
                            }
                        }
                        _ => unreachable!(),
                    }
                } else if self.export_state.editing_path {
                    let buf = &mut self.export_state.edit_buffer;
                    let cur = &mut self.export_state.edit_cursor;
                    match action {
                        Action::CursorLeft => {
                            *cur = buf[..*cur]
                                .char_indices()
                                .next_back()
                                .map(|(i, _)| i)
                                .unwrap_or(0);
                        }
                        Action::CursorRight => {
                            if *cur < buf.len() {
                                *cur += buf[*cur..]
                                    .chars()
                                    .next()
                                    .map(|c| c.len_utf8())
                                    .unwrap_or(0);
                            }
                        }
                        Action::CursorHome => *cur = 0,
                        Action::CursorEnd => *cur = buf.len(),
                        Action::DeleteForward => {
                            if *cur < buf.len() {
                                let next = *cur
                                    + buf[*cur..]
                                        .chars()
                                        .next()
                                        .map(|c| c.len_utf8())
                                        .unwrap_or(0);
                                buf.drain(*cur..next);
                            }
                        }
                        _ => unreachable!(),
                    }
                }
            }
            Action::SearchInput(c) => {
                if self.config_state.editing {
                    // Route to config text editing
                    if c == '\x08' {
                        // Backspace: delete char before cursor
                        if self.config_state.edit_cursor > 0 {
                            let prev = self.config_state.edit_buffer
                                [..self.config_state.edit_cursor]
                                .char_indices()
                                .next_back()
                                .map(|(i, _)| i)
                                .unwrap_or(0);
                            self.config_state
                                .edit_buffer
                                .drain(prev..self.config_state.edit_cursor);
                            self.config_state.edit_cursor = prev;
                        }
                    } else {
                        self.config_state
                            .edit_buffer
                            .insert(self.config_state.edit_cursor, c);
                        self.config_state.edit_cursor += c.len_utf8();
                    }
                } else {
                    if c == '\x08' {
                        self.search_query.pop();
                    } else {
                        self.search_query.push(c);
                    }
                    if self.screen == Screen::Queue {
                        self.recompute_sorted_indices();
                        self.queue_cursor = 0;
                    }
                }
            }
            Action::SearchConfirm => {
                if self.config_state.editing {
                    self.confirm_config_edit();
                } else {
                    self.input_mode = InputMode::Normal;
                }
            }
            Action::SearchCancel => {
                if self.config_state.editing {
                    self.config_state.editing = false;
                    self.config_state.edit_buffer.clear();
                    self.config_state.edit_cursor = 0;
                    self.input_mode = InputMode::Normal;
                } else {
                    self.input_mode = InputMode::Normal;
                    self.search_query.clear();
                    if self.screen == Screen::Queue {
                        self.recompute_sorted_indices();
                    }
                }
            }
            Action::NextMatch | Action::PrevMatch => {}
            Action::ToggleActivityPanel => {
                self.activity_panel_visible = !self.activity_panel_visible;
            }
            Action::OpenConfig => {
                if self.screen != Screen::Config {
                    self.config_state.prev_screen = Some(self.screen.clone());
                }
                self.screen = Screen::Config;
            }
            Action::Export => {
                self.export_state.active = true;
                self.export_state.cursor = 0;
                self.export_state.message = None;
                self.export_state.output_path = self.export_default_path(self.export_state.scope);
            }
            Action::StartProcessing => {
                if self.screen == Screen::Queue {
                    if !self.processing_started {
                        self.start_processing();
                    } else if !self.batch_complete {
                        // Cancel the active batch
                        if let Some(tx) = &self.backend_cmd_tx {
                            let _ = tx.send(BackendCommand::CancelProcessing);
                        }
                        self.frozen_elapsed = Some(self.elapsed());
                        self.batch_complete = true;
                        self.processing_started = false;
                        self.activity.active_queries.clear();
                    } else {
                        // Batch completed — allow restart
                        self.processing_started = false;
                        self.start_processing();
                    }
                }
            }
            Action::ToggleSafe => {
                match &self.screen {
                    Screen::Queue => {
                        // Space on queue: cycle paper verdict (None → Safe → Questionable → None)
                        if self.queue_cursor < self.queue_sorted.len() {
                            let paper_idx = self.queue_sorted[self.queue_cursor];
                            if let Some(paper) = self.papers.get_mut(paper_idx) {
                                paper.verdict = PaperVerdict::cycle(paper.verdict);
                            }
                        }
                    }
                    Screen::Paper(idx) => {
                        // Space on paper: cycle FP reason on current reference
                        let idx = *idx;
                        let indices = self.paper_ref_indices(idx);
                        if self.paper_cursor < indices.len() {
                            let ref_idx = indices[self.paper_cursor];
                            self.toggle_fp_and_maybe_propagate(idx, ref_idx);
                        }
                    }
                    Screen::RefDetail(paper_idx, ref_idx) => {
                        // Space on detail: cycle FP reason
                        let paper_idx = *paper_idx;
                        let ref_idx = *ref_idx;
                        self.toggle_fp_and_maybe_propagate(paper_idx, ref_idx);
                    }
                    Screen::Config => {
                        // Space on config: toggle database or cycle theme
                        self.handle_config_space();
                    }
                    _ => {}
                }
            }
            Action::ClickAt(x, y) => {
                self.handle_click(x, y);
            }
            Action::CycleConfigSection => {
                if self.screen == Screen::Config {
                    let sections = crate::model::config::ConfigSection::all();
                    let idx = sections
                        .iter()
                        .position(|&s| s == self.config_state.section)
                        .unwrap_or(0);
                    self.config_state.section = sections[(idx + 1) % sections.len()];
                    self.config_state.item_cursor = 0;
                }
            }
            Action::AddFiles => {
                if self.screen == Screen::Config
                    && self.config_state.section == crate::model::config::ConfigSection::Databases
                    && self.config_state.item_cursor <= 2
                {
                    // Open file picker in database selection mode
                    let config_item = self.config_state.item_cursor;
                    self.file_picker_context = FilePickerContext::SelectDatabase { config_item };
                    self.file_picker.selected.clear();

                    // Navigate to the current path's parent if set
                    let current_path = if config_item == 0 {
                        &self.config_state.dblp_offline_path
                    } else if config_item == 1 {
                        &self.config_state.acl_offline_path
                    } else {
                        &self.config_state.openalex_offline_path
                    };
                    if !current_path.is_empty() {
                        let p = PathBuf::from(current_path);
                        if let Some(parent) = p.parent()
                            && parent.is_dir()
                        {
                            self.file_picker.current_dir = parent.to_path_buf();
                            self.file_picker.refresh_entries();
                        }
                    }

                    self.screen = Screen::FilePicker;
                } else if self.screen != Screen::Config {
                    self.file_picker_context = FilePickerContext::AddFiles;
                    self.screen = Screen::FilePicker;
                }
            }
            Action::CopyToClipboard => {
                if let Some(text) = self.get_copyable_text() {
                    super::osc52_copy(&text);
                    self.activity.log("Copied to clipboard".to_string());
                }
            }
            Action::OpenPdf => {
                let paper_idx = match &self.screen {
                    Screen::Queue => self.queue_sorted.get(self.queue_cursor).copied(),
                    Screen::Paper(i) | Screen::RefDetail(i, _) => Some(*i),
                    _ => None,
                };
                if let Some(idx) = paper_idx
                    && let Some(path) = self.file_paths.get(idx)
                {
                    if path.as_os_str().is_empty() {
                        self.activity
                            .log_warn("No source file path available for this paper".to_string());
                    } else if !path.exists() {
                        self.activity
                            .log_warn(format!("File not found: {}", path.display()));
                    } else if let Err(e) = open::that(path) {
                        self.activity
                            .log_warn(format!("Failed to open {}: {}", path.display(), e));
                    }
                }
            }
            Action::SaveConfig => {
                self.save_config();
                if matches!(self.screen, Screen::Config) {
                    if let Some(prev) = self.config_state.prev_screen.clone() {
                        self.screen = prev;
                    } else {
                        self.screen = Screen::Queue;
                    }
                }
            }
            Action::BuildDatabase => {
                self.handle_build_database();
            }
            Action::Retry => {
                self.handle_retry_single();
            }
            Action::RetryAll => {
                self.handle_retry_all();
            }
            Action::RemovePaper => {
                // Placeholder for future implementation
            }
            Action::Tick => {
                self.tick = self.tick.wrapping_add(1);

                // Drain streaming archive channel (if active)
                if self.archive_rx.is_some() {
                    self.drain_archive_channel();
                }
                // Start next archive extraction if none in progress
                if self.archive_rx.is_none() && !self.pending_archive_extractions.is_empty() {
                    self.start_next_archive_extraction();
                }

                if self.screen == Screen::Queue {
                    self.recompute_sorted_indices();
                }
                // Throughput tracking: push a bucket every ~1 second
                if self.tick.wrapping_sub(self.last_throughput_tick)
                    >= self.config_state.fps as usize
                {
                    self.activity.push_throughput(self.throughput_since_last);
                    self.throughput_since_last = 0;
                    self.last_throughput_tick = self.tick;
                }
            }
            Action::Resize(_w, h) => {
                self.visible_rows = (h as usize).saturating_sub(11);
            }
            // No-op outside the export modal — handled above when
            // `export_state.active` is true.
            Action::BrowsePath => {}
            Action::None => {}
        }
        false
    }

    /// Space handler for mark-safe on a ref: toggles the origin's
    /// fp_reason + updates origin's stats, then either propagates
    /// immediately (silent, below threshold) or opens a confirmation
    /// dialog (at/above threshold) for retroactive queue-wide sweep.
    fn toggle_fp_and_maybe_propagate(&mut self, paper_idx: usize, ref_idx: usize) {
        let (origin_identity, new_fp_reason) = cycle_fp_reason_and_adjust_stats(
            &mut self.papers,
            &mut self.ref_states,
            paper_idx,
            ref_idx,
            self.current_query_cache.as_ref(),
        );
        let Some(identity) = origin_identity else {
            return; // session-local mark only; nothing to propagate
        };

        let targets = collect_propagation_targets(
            &self.ref_states,
            paper_idx,
            ref_idx,
            &identity,
            new_fp_reason,
        );
        if targets.is_empty() {
            return; // no matching refs anywhere else
        }

        let other_papers = distinct_other_papers(&targets, paper_idx);
        if other_papers < super::PROPAGATION_CONFIRM_THRESHOLD {
            // Fast path: silent sweep — the user probably wants this,
            // and interrupting on every Space press would be annoying
            // for the common case of just 1-2 other papers.
            propagate_fp_override(
                &mut self.papers,
                &mut self.ref_states,
                paper_idx,
                ref_idx,
                &identity,
                new_fp_reason,
            );
        } else {
            // Threshold crossed: defer the sweep behind a dialog.
            let summary = summarize_targets_by_paper(&targets, &self.papers);
            self.pending_propagation = Some(super::PendingPropagation {
                origin_identity: identity,
                new_fp_reason,
                origin_paper_idx: paper_idx,
                origin_ref_idx: ref_idx,
                affected_summary: summary,
                total_refs: targets.len(),
            });
        }
    }

    /// User confirmed a pending propagation — apply the sweep and
    /// clear the dialog state. No-op when there's no pending dialog.
    pub(crate) fn confirm_pending_propagation(&mut self) {
        let Some(pending) = self.pending_propagation.take() else {
            return;
        };
        propagate_fp_override(
            &mut self.papers,
            &mut self.ref_states,
            pending.origin_paper_idx,
            pending.origin_ref_idx,
            &pending.origin_identity,
            pending.new_fp_reason,
        );
    }

    /// User cancelled a pending propagation — the origin stays marked,
    /// other papers are left untouched.
    pub(crate) fn cancel_pending_propagation(&mut self) {
        self.pending_propagation = None;
    }
}

/// Cycle the FP reason on one reference and keep the paper-level
/// stats in sync.
///
/// Pressing Space cycles `rs.fp_reason` through None → Some(r1) →
/// Some(r2) → ... → None. Only the None ↔ Some transitions move the
/// reference between the problematic bucket (not_found /
/// mismatch / retracted) and `verified`; going between two Some
/// variants is a purely-informational change. For the None↔Some
/// transitions we call `PaperState::apply_fp_delta` so the queue
/// table columns, the bottom-of-screen totals, and the paper-view
/// "problems" counter all reflect the override immediately.
///
/// Takes disjoint mutable borrows (&mut papers, &mut ref_states)
/// instead of &mut self so it can be called from both Screen::Paper
/// and Screen::RefDetail arms without fighting the borrow checker
/// over the enclosing match.
/// Cycle the origin ref's fp_reason, persist to cache, and adjust
/// the origin paper's stats. Returns `(origin_identity, new_fp_reason)`
/// so the caller can decide whether to propagate the change across
/// the loaded queue (issue #266) — this function itself does NOT
/// propagate. The caller typically routes either to
/// [`propagate_fp_override`] (below threshold, silent) or to a
/// confirmation popup (at/above threshold).
fn cycle_fp_reason_and_adjust_stats(
    papers: &mut [crate::model::queue::PaperState],
    ref_states: &mut [Vec<crate::model::paper::RefState>],
    paper_idx: usize,
    ref_idx: usize,
    cache: Option<&std::sync::Arc<hallucinator_core::QueryCache>>,
) -> (Option<String>, Option<FpReason>) {
    let Some(refs) = ref_states.get_mut(paper_idx) else {
        return (None, None);
    };
    let Some(rs) = refs.get_mut(ref_idx) else {
        return (None, None);
    };

    let was_safe = rs.fp_reason.is_some();
    rs.fp_reason = FpReason::cycle(rs.fp_reason);
    let is_safe = rs.fp_reason.is_some();

    // Persist the mark only when the ref has enough identity
    // information (title + ≥1 extracted author). Empty-author
    // refs get a session-local mark — in-memory only — because
    // a title-only key would collide with every other same-
    // titled ref and could silently mark a fabricated ref as
    // safe on paper load. See issue #267.
    let identity = hallucinator_core::cache::compute_fp_identity(&rs.title, &rs.authors);
    if let Some(cache) = cache
        && let Some(ref key) = identity
    {
        cache.set_fp_override(key, rs.fp_reason.map(|r| r.as_str()));
    }

    // Adjust origin paper's stats on a None↔Some transition.
    // Some(r1)↔Some(r2) keeps the ref marked-safe, so the stats
    // are already correct.
    if was_safe != is_safe
        && let Some(result) = &rs.result
    {
        let is_retracted = result
            .retraction_info
            .as_ref()
            .is_some_and(|r| r.is_retracted);
        let status = result.status.clone();
        let dir: i32 = if is_safe { 1 } else { -1 };
        if let Some(paper) = papers.get_mut(paper_idx) {
            paper.apply_fp_delta(&status, is_retracted, dir);
        }
    }

    let new_fp = rs.fp_reason;
    (identity, new_fp)
}

/// Dry-run counterpart to [`propagate_fp_override`]: walks the queue
/// and returns the `(paper_idx, ref_idx)` pairs that WOULD be flipped
/// by a propagation with `origin_identity` and `new_fp_reason`, with
/// no mutations. Used to decide whether the propagation should fire
/// silently or pop a confirmation dialog.
pub(crate) fn collect_propagation_targets(
    ref_states: &[Vec<crate::model::paper::RefState>],
    origin_paper_idx: usize,
    origin_ref_idx: usize,
    origin_identity: &str,
    new_fp_reason: Option<FpReason>,
) -> Vec<(usize, usize)> {
    let mut targets = Vec::new();
    for (p_idx, refs) in ref_states.iter().enumerate() {
        for (r_idx, rs) in refs.iter().enumerate() {
            if (p_idx, r_idx) == (origin_paper_idx, origin_ref_idx) {
                continue;
            }
            let Some(ident) = hallucinator_core::cache::compute_fp_identity(&rs.title, &rs.authors)
            else {
                continue;
            };
            if ident != origin_identity {
                continue;
            }
            if rs.fp_reason == new_fp_reason {
                continue;
            }
            targets.push((p_idx, r_idx));
        }
    }
    targets
}

/// Group propagation targets by paper, returning `(paper_filename,
/// refs_in_this_paper)` pairs sorted by filename for stable display
/// in the confirmation dialog.
pub(crate) fn summarize_targets_by_paper(
    targets: &[(usize, usize)],
    papers: &[crate::model::queue::PaperState],
) -> Vec<(String, usize)> {
    use std::collections::BTreeMap;
    let mut per_paper: BTreeMap<usize, usize> = BTreeMap::new();
    for &(p, _) in targets {
        *per_paper.entry(p).or_insert(0) += 1;
    }
    let mut out: Vec<(String, usize)> = per_paper
        .into_iter()
        .map(|(p_idx, count)| {
            let fname = papers
                .get(p_idx)
                .map(|p| p.filename.clone())
                .unwrap_or_default();
            (fname, count)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Count of distinct papers (excluding the origin paper) in a target
/// set. Used to decide threshold-crossing for the confirmation popup.
pub(crate) fn distinct_other_papers(targets: &[(usize, usize)], origin_paper_idx: usize) -> usize {
    use std::collections::BTreeSet;
    targets
        .iter()
        .filter_map(|&(p, _)| (p != origin_paper_idx).then_some(p))
        .collect::<BTreeSet<_>>()
        .len()
}

#[cfg(test)]
pub(crate) fn __test_propagate_fp_override(
    papers: &mut [crate::model::queue::PaperState],
    ref_states: &mut [Vec<crate::model::paper::RefState>],
    origin_paper_idx: usize,
    origin_ref_idx: usize,
    origin_identity: &str,
    new_fp_reason: Option<FpReason>,
) -> usize {
    propagate_fp_override(
        papers,
        ref_states,
        origin_paper_idx,
        origin_ref_idx,
        origin_identity,
        new_fp_reason,
    )
}

/// Apply `new_fp_reason` to every ref (across all loaded papers)
/// whose composite identity key matches `origin_identity`, skipping
/// the origin ref itself. Adjusts the `PaperState` stats for each
/// paper whose ref's safe-state flipped. Returns the count of refs
/// actually updated (useful for tests / diagnostics).
///
/// Refs whose `compute_fp_identity` returns `None` (empty authors)
/// are skipped — they couldn't have been persisted and aren't
/// meaningfully "the same reference" as the origin from our
/// identity model's perspective.
fn propagate_fp_override(
    papers: &mut [crate::model::queue::PaperState],
    ref_states: &mut [Vec<crate::model::paper::RefState>],
    origin_paper_idx: usize,
    origin_ref_idx: usize,
    origin_identity: &str,
    new_fp_reason: Option<FpReason>,
) -> usize {
    let mut updated = 0;
    for (p_idx, refs) in ref_states.iter_mut().enumerate() {
        for (r_idx, rs) in refs.iter_mut().enumerate() {
            if (p_idx, r_idx) == (origin_paper_idx, origin_ref_idx) {
                continue;
            }
            let Some(ident) = hallucinator_core::cache::compute_fp_identity(&rs.title, &rs.authors)
            else {
                continue;
            };
            if ident != origin_identity {
                continue;
            }
            // Already in the target state? nothing to do (avoids
            // spurious stat churn from toggling a ref that was
            // already synced on a previous propagation or on paper
            // load via `get_fp_override`).
            if rs.fp_reason == new_fp_reason {
                continue;
            }
            let was_safe = rs.fp_reason.is_some();
            let will_be_safe = new_fp_reason.is_some();
            rs.fp_reason = new_fp_reason;
            updated += 1;

            if was_safe == will_be_safe {
                // Some(a) → Some(b): the safe-state didn't flip, so
                // the paper's bucket counts are already correct.
                // We've just updated the displayed reason.
                continue;
            }
            let Some(result) = &rs.result else {
                continue; // unvalidated ref; no stats to adjust
            };
            let is_retracted = result
                .retraction_info
                .as_ref()
                .is_some_and(|r| r.is_retracted);
            let status = result.status.clone();
            let dir: i32 = if will_be_safe { 1 } else { -1 };
            if let Some(paper) = papers.get_mut(p_idx) {
                paper.apply_fp_delta(&status, is_retracted, dir);
            }
        }
    }
    updated
}

#[cfg(test)]
mod propagation_tests {
    use super::*;
    use crate::model::paper::{FpReason, RefPhase, RefState};
    use crate::model::queue::PaperState;
    use hallucinator_core::cache::compute_fp_identity;
    use hallucinator_core::{Status, ValidationResult};

    fn val(status: Status) -> ValidationResult {
        ValidationResult {
            title: String::new(),
            raw_citation: String::new(),
            ref_authors: Vec::new(),
            status,
            source: None,
            found_authors: Vec::new(),
            paper_url: None,
            failed_dbs: Vec::new(),
            db_results: Vec::new(),
            doi_info: None,
            arxiv_info: None,
            retraction_info: None,
        }
    }

    fn refs(
        title: &str,
        authors: &[&str],
        status: Option<Status>,
        fp_reason: Option<FpReason>,
    ) -> RefState {
        RefState {
            index: 0,
            title: title.into(),
            phase: RefPhase::Done,
            result: status.map(val),
            fp_reason,
            raw_citation: String::new(),
            authors: authors.iter().map(|s| s.to_string()).collect(),
            doi: None,
            arxiv_id: None,
            urls: Vec::new(),
        }
    }

    /// Build `n_papers` each holding one ref with the given title/authors/status,
    /// populating `paper.stats` so propagation can adjust it.
    fn fixture(
        n_papers: usize,
        title: &str,
        authors: &[&str],
        status: Status,
    ) -> (Vec<PaperState>, Vec<Vec<RefState>>) {
        let mut papers = Vec::with_capacity(n_papers);
        let mut ref_states = Vec::with_capacity(n_papers);
        for i in 0..n_papers {
            let mut p = PaperState::new(format!("paper{i}.pdf"));
            p.init_results(1);
            p.record_status(0, status.clone(), false);
            papers.push(p);
            ref_states.push(vec![refs(title, authors, Some(status.clone()), None)]);
        }
        (papers, ref_states)
    }

    #[test]
    fn propagate_across_papers_flips_safe_counts() {
        let (mut papers, mut ref_states) =
            fixture(3, "Shared Paper", &["Alice Author"], Status::NotFound);
        assert_eq!(papers[0].stats.not_found, 1);
        assert_eq!(papers[1].stats.not_found, 1);
        assert_eq!(papers[2].stats.not_found, 1);

        ref_states[0][0].fp_reason = Some(FpReason::KnownGood);
        papers[0].apply_fp_delta(&Status::NotFound, false, 1);

        let key = compute_fp_identity("Shared Paper", &["Alice Author".into()]).unwrap();
        let n = __test_propagate_fp_override(
            &mut papers,
            &mut ref_states,
            0,
            0,
            &key,
            Some(FpReason::KnownGood),
        );
        assert_eq!(n, 2, "two other refs should have been updated");

        for p in &papers {
            assert_eq!(p.stats.not_found, 0);
            assert_eq!(p.stats.verified, 1);
        }
        for refs in &ref_states {
            assert_eq!(refs[0].fp_reason, Some(FpReason::KnownGood));
        }
    }

    #[test]
    fn propagate_ignores_nonmatching_titles() {
        let mut papers = vec![
            PaperState::new("a.pdf".into()),
            PaperState::new("b.pdf".into()),
        ];
        papers[0].init_results(1);
        papers[0].record_status(0, Status::NotFound, false);
        papers[1].init_results(1);
        papers[1].record_status(0, Status::NotFound, false);
        let mut ref_states = vec![
            vec![refs(
                "Some Paper",
                &["A. Author"],
                Some(Status::NotFound),
                None,
            )],
            vec![refs(
                "A Completely Different Paper",
                &["A. Author"],
                Some(Status::NotFound),
                None,
            )],
        ];
        let key = compute_fp_identity("Some Paper", &["A. Author".into()]).unwrap();
        let n = __test_propagate_fp_override(
            &mut papers,
            &mut ref_states,
            0,
            0,
            &key,
            Some(FpReason::KnownGood),
        );
        assert_eq!(n, 0, "no other refs share the origin's identity");
        assert!(ref_states[1][0].fp_reason.is_none());
        assert_eq!(papers[1].stats.not_found, 1);
    }

    #[test]
    fn propagate_skips_origin() {
        let (mut papers, mut ref_states) =
            fixture(1, "Only Paper", &["A. Author"], Status::NotFound);
        let key = compute_fp_identity("Only Paper", &["A. Author".into()]).unwrap();
        let n = __test_propagate_fp_override(
            &mut papers,
            &mut ref_states,
            0,
            0,
            &key,
            Some(FpReason::KnownGood),
        );
        assert_eq!(n, 0, "origin skipped; no other refs in queue");
        assert!(ref_states[0][0].fp_reason.is_none());
        assert_eq!(papers[0].stats.not_found, 1);
    }

    #[test]
    fn propagate_handles_same_paper_siblings() {
        let mut paper = PaperState::new("p.pdf".into());
        paper.init_results(2);
        paper.record_status(0, Status::NotFound, false);
        paper.record_status(1, Status::NotFound, false);
        assert_eq!(paper.stats.not_found, 2);

        let mut papers = vec![paper];
        let mut ref_states = vec![vec![
            refs("Dup", &["A. Author"], Some(Status::NotFound), None),
            refs("Dup", &["A. Author"], Some(Status::NotFound), None),
        ]];

        ref_states[0][0].fp_reason = Some(FpReason::KnownGood);
        papers[0].apply_fp_delta(&Status::NotFound, false, 1);

        let key = compute_fp_identity("Dup", &["A. Author".into()]).unwrap();
        let n = __test_propagate_fp_override(
            &mut papers,
            &mut ref_states,
            0,
            0,
            &key,
            Some(FpReason::KnownGood),
        );
        assert_eq!(n, 1, "one sibling updated");
        assert_eq!(ref_states[0][1].fp_reason, Some(FpReason::KnownGood));
        assert_eq!(papers[0].stats.not_found, 0);
        assert_eq!(papers[0].stats.verified, 2);
    }

    #[test]
    fn propagate_some_to_some_does_not_shift_counts() {
        let (mut papers, mut ref_states) = fixture(2, "Shared", &["A. Author"], Status::NotFound);
        for i in 0..2 {
            ref_states[i][0].fp_reason = Some(FpReason::KnownGood);
            papers[i].apply_fp_delta(&Status::NotFound, false, 1);
        }
        assert_eq!(papers[0].stats.verified, 1);
        assert_eq!(papers[1].stats.verified, 1);

        ref_states[0][0].fp_reason = Some(FpReason::NonAcademic);
        let key = compute_fp_identity("Shared", &["A. Author".into()]).unwrap();
        let n = __test_propagate_fp_override(
            &mut papers,
            &mut ref_states,
            0,
            0,
            &key,
            Some(FpReason::NonAcademic),
        );
        assert_eq!(n, 1);
        assert_eq!(ref_states[1][0].fp_reason, Some(FpReason::NonAcademic));
        assert_eq!(papers[1].stats.verified, 1);
        assert_eq!(papers[1].stats.not_found, 0);
    }

    #[test]
    fn propagate_skips_unvalidated_refs() {
        let p0 = {
            let mut p = PaperState::new("a.pdf".into());
            p.init_results(1);
            p.record_status(0, Status::NotFound, false);
            p
        };
        let p1 = PaperState::new("b.pdf".into()); // no results recorded
        let mut papers = vec![p0, p1];
        let mut ref_states = vec![
            vec![refs("Same", &["A. Author"], Some(Status::NotFound), None)],
            vec![refs("Same", &["A. Author"], None, None)], // unvalidated
        ];
        let key = compute_fp_identity("Same", &["A. Author".into()]).unwrap();
        let n = __test_propagate_fp_override(
            &mut papers,
            &mut ref_states,
            0,
            0,
            &key,
            Some(FpReason::KnownGood),
        );
        assert_eq!(n, 1);
        assert_eq!(ref_states[1][0].fp_reason, Some(FpReason::KnownGood));
        assert_eq!(papers[1].stats.not_found, 0);
        assert_eq!(papers[1].stats.verified, 0);
    }

    #[test]
    fn propagate_does_not_cross_author_boundaries_with_same_title() {
        // The fake-cite regression (from reviewer feedback on #266).
        // Two papers cite the same title but with disjoint author
        // sets — the propagation must NOT flip the fabrication to safe.
        let real_title = "Attention Is All You Need";
        let real_authors: Vec<&str> = vec!["Ashish Vaswani", "Noam Shazeer"];
        let fake_authors: Vec<&str> = vec!["Jeremy Blackburn", "Gianluca Stringhini"];

        let mut papers = vec![
            {
                let mut p = PaperState::new("real.pdf".into());
                p.init_results(1);
                p.record_status(0, Status::NotFound, false);
                p
            },
            {
                let mut p = PaperState::new("fake.pdf".into());
                p.init_results(1);
                p.record_status(0, Status::NotFound, false);
                p
            },
        ];
        let mut ref_states = vec![
            vec![refs(
                real_title,
                &real_authors,
                Some(Status::NotFound),
                None,
            )],
            vec![refs(
                real_title,
                &fake_authors,
                Some(Status::NotFound),
                None,
            )],
        ];
        let key = compute_fp_identity(
            real_title,
            &real_authors
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
        )
        .unwrap();

        let n = __test_propagate_fp_override(
            &mut papers,
            &mut ref_states,
            0,
            0,
            &key,
            Some(FpReason::KnownGood),
        );
        assert_eq!(n, 0, "fake-cite must not inherit the real ref's safe mark");
        assert!(ref_states[1][0].fp_reason.is_none());
        assert_eq!(papers[1].stats.not_found, 1);
    }

    // ── Dry-run / threshold helpers (confirmation popup, #266) ──

    #[test]
    fn collect_targets_is_empty_when_no_other_refs_match() {
        let (_papers, ref_states) = fixture(1, "Lonely Paper", &["A. Author"], Status::NotFound);
        let key = compute_fp_identity("Lonely Paper", &["A. Author".into()]).unwrap();
        let targets =
            super::collect_propagation_targets(&ref_states, 0, 0, &key, Some(FpReason::KnownGood));
        assert!(targets.is_empty());
    }

    #[test]
    fn collect_targets_finds_all_matching_refs_across_papers() {
        let (_papers, ref_states) = fixture(4, "Shared", &["A. Author"], Status::NotFound);
        let key = compute_fp_identity("Shared", &["A. Author".into()]).unwrap();
        let targets =
            super::collect_propagation_targets(&ref_states, 0, 0, &key, Some(FpReason::KnownGood));
        // 4 papers with 1 matching ref each; origin skipped → 3 matches.
        assert_eq!(targets.len(), 3);
        assert!(!targets.contains(&(0, 0)));
        assert!(targets.contains(&(1, 0)));
        assert!(targets.contains(&(2, 0)));
        assert!(targets.contains(&(3, 0)));
    }

    #[test]
    fn collect_targets_skips_refs_already_at_target_state() {
        let (_papers, mut ref_states) = fixture(3, "Shared", &["A. Author"], Status::NotFound);
        // Pre-mark paper 1 with the target reason — propagation should
        // skip it (no net change).
        ref_states[1][0].fp_reason = Some(FpReason::KnownGood);
        let key = compute_fp_identity("Shared", &["A. Author".into()]).unwrap();
        let targets =
            super::collect_propagation_targets(&ref_states, 0, 0, &key, Some(FpReason::KnownGood));
        assert_eq!(targets.len(), 1); // only paper 2 still needs flipping
        assert_eq!(targets[0], (2, 0));
    }

    #[test]
    fn summarize_groups_by_paper_and_sorts_by_filename() {
        let mut papers: Vec<PaperState> = vec![
            PaperState::new("c.pdf".into()),
            PaperState::new("a.pdf".into()),
            PaperState::new("b.pdf".into()),
        ];
        for p in &mut papers {
            p.init_results(2);
        }
        // 3 targets across 3 papers: 1, 2, 1 refs each.
        let targets = vec![(0, 0), (1, 0), (1, 1), (2, 0)];
        let summary = super::summarize_targets_by_paper(&targets, &papers);
        // Sorted by filename ascending.
        assert_eq!(
            summary,
            vec![
                ("a.pdf".into(), 2),
                ("b.pdf".into(), 1),
                ("c.pdf".into(), 1),
            ]
        );
    }

    #[test]
    fn distinct_other_papers_excludes_origin() {
        // 5 targets across 3 papers, origin is paper 0 — distinct-
        // OTHER-papers count should be 2 (paper 1 and paper 2), even
        // if paper 0 also has same-paper siblings listed.
        let targets = vec![(0, 1), (0, 2), (1, 0), (1, 1), (2, 0)];
        let n = super::distinct_other_papers(&targets, 0);
        assert_eq!(n, 2);
    }

    #[test]
    fn distinct_other_papers_returns_zero_when_only_same_paper() {
        // Same-paper siblings (origin = paper 0, targets also in
        // paper 0) should not trip the threshold — it's the same
        // paper the user is already looking at.
        let targets = vec![(0, 1), (0, 2), (0, 3)];
        let n = super::distinct_other_papers(&targets, 0);
        assert_eq!(n, 0);
    }

    #[test]
    fn propagate_skips_refs_with_empty_authors() {
        let mut papers = vec![
            {
                let mut p = PaperState::new("a.pdf".into());
                p.init_results(1);
                p.record_status(0, Status::NotFound, false);
                p
            },
            {
                let mut p = PaperState::new("b.pdf".into());
                p.init_results(1);
                p.record_status(0, Status::NotFound, false);
                p
            },
        ];
        let mut ref_states = vec![
            vec![refs("Same", &["A. Author"], Some(Status::NotFound), None)],
            vec![refs("Same", &[], Some(Status::NotFound), None)], // empty authors
        ];
        let key = compute_fp_identity("Same", &["A. Author".into()]).unwrap();
        let n = __test_propagate_fp_override(
            &mut papers,
            &mut ref_states,
            0,
            0,
            &key,
            Some(FpReason::KnownGood),
        );
        assert_eq!(n, 0, "empty-authors ref is not a match");
        assert!(ref_states[1][0].fp_reason.is_none());
    }
}
