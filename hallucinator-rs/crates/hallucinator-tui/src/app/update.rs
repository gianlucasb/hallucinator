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

        // Export modal intercepts
        if self.export_state.active {
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
                                (0..self.papers.len())
                                    .filter(|&i| {
                                        self.papers.get(i).is_some_and(|p| {
                                            p.stats.not_found > 0
                                                || p.stats.author_mismatch > 0
                                                || p.stats.retracted > 0
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
                            if let Some(refs) = self.ref_states.get_mut(idx)
                                && let Some(rs) = refs.get_mut(ref_idx)
                            {
                                rs.fp_reason = FpReason::cycle(rs.fp_reason);
                                if let Some(cache) = &self.current_query_cache {
                                    cache.set_fp_override(
                                        &rs.title,
                                        rs.fp_reason.map(|r| r.as_str()),
                                    );
                                }
                            }
                        }
                    }
                    Screen::RefDetail(paper_idx, ref_idx) => {
                        // Space on detail: cycle FP reason
                        let paper_idx = *paper_idx;
                        let ref_idx = *ref_idx;
                        if let Some(refs) = self.ref_states.get_mut(paper_idx)
                            && let Some(rs) = refs.get_mut(ref_idx)
                        {
                            rs.fp_reason = FpReason::cycle(rs.fp_reason);
                            if let Some(cache) = &self.current_query_cache {
                                cache.set_fp_override(&rs.title, rs.fp_reason.map(|r| r.as_str()));
                            }
                        }
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
            Action::None => {}
        }
        false
    }
}
