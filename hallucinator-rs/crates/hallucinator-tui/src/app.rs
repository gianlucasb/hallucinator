use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

use ratatui::layout::{Constraint, Layout, Rect};
use tokio::sync::mpsc;

use hallucinator_core::{DbStatus, ProgressEvent, Reference};

use crate::action::Action;
use crate::model::activity::{ActiveQuery, ActivityState};
use crate::model::config::ConfigState;
use crate::model::paper::{PaperFilter, PaperSortOrder, RefPhase, RefState};
use crate::model::queue::{filtered_indices, PaperPhase, PaperState, QueueFilter, SortOrder};
use crate::theme::Theme;
use crate::tui_event::{BackendCommand, BackendEvent};
use crate::view::export::ExportState;

/// Which screen is currently displayed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    Banner,
    Queue,
    Paper(usize),            // index into papers vec
    RefDetail(usize, usize), // (paper_index, ref_index)
    Config,
    FilePicker,
}

/// Input mode determines how keyboard input is interpreted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Search,
    TextInput,
}

/// State for the file picker screen.
#[derive(Debug, Clone)]
pub struct FilePickerState {
    /// Current directory being browsed.
    pub current_dir: PathBuf,
    /// Entries in the current directory (dirs first, then files).
    pub entries: Vec<FileEntry>,
    /// Cursor position in the entries list.
    pub cursor: usize,
    /// Selected PDF files for processing.
    pub selected: Vec<PathBuf>,
    /// Scroll offset for the entries list.
    pub scroll_offset: usize,
}

/// A single entry in the file picker.
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_pdf: bool,
    pub is_archive: bool,
}

impl FilePickerState {
    pub fn new() -> Self {
        let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut state = Self {
            current_dir: current_dir.clone(),
            entries: Vec::new(),
            cursor: 0,
            selected: Vec::new(),
            scroll_offset: 0,
        };
        state.refresh_entries();
        state
    }

    /// Refresh the entries list from the current directory.
    pub fn refresh_entries(&mut self) {
        let mut entries = Vec::new();

        // Parent directory entry
        if let Some(parent) = self.current_dir.parent() {
            entries.push(FileEntry {
                name: "..".to_string(),
                path: parent.to_path_buf(),
                is_dir: true,
                is_pdf: false,
                is_archive: false,
            });
        }

        if let Ok(read_dir) = std::fs::read_dir(&self.current_dir) {
            let mut dirs = Vec::new();
            let mut files = Vec::new();

            for entry in read_dir.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();

                // Skip hidden files/dirs
                if name.starts_with('.') {
                    continue;
                }

                if path.is_dir() {
                    dirs.push(FileEntry {
                        name,
                        path,
                        is_dir: true,
                        is_pdf: false,
                        is_archive: false,
                    });
                } else {
                    let is_pdf = path
                        .extension()
                        .map(|ext| ext.eq_ignore_ascii_case("pdf"))
                        .unwrap_or(false);
                    let is_archive = hallucinator_pdf::archive::is_archive_path(&path);
                    files.push(FileEntry {
                        name,
                        path,
                        is_dir: false,
                        is_pdf,
                        is_archive,
                    });
                }
            }

            dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

            entries.extend(dirs);
            entries.extend(files);
        }

        self.entries = entries;
        self.cursor = 0;
        self.scroll_offset = 0;
    }

    /// Toggle selection of the current entry (PDFs and archives).
    pub fn toggle_selected(&mut self) {
        if let Some(entry) = self.entries.get(self.cursor) {
            if entry.is_pdf || entry.is_archive {
                if let Some(pos) = self.selected.iter().position(|p| p == &entry.path) {
                    self.selected.remove(pos);
                } else {
                    self.selected.push(entry.path.clone());
                }
            }
        }
    }

    /// Enter the directory at cursor, or return false if not a directory.
    pub fn enter_directory(&mut self) -> bool {
        if let Some(entry) = self.entries.get(self.cursor) {
            if entry.is_dir {
                self.current_dir = entry.path.clone();
                self.refresh_entries();
                return true;
            }
        }
        false
    }

    pub fn is_selected(&self, path: &PathBuf) -> bool {
        self.selected.contains(path)
    }
}

/// Main application state.
pub struct App {
    pub screen: Screen,
    pub papers: Vec<PaperState>,
    /// Per-paper reference states, indexed in parallel with `papers`.
    pub ref_states: Vec<Vec<RefState>>,
    /// Per-paper extracted references (for retry support).
    pub paper_refs: Vec<Vec<Reference>>,
    pub queue_cursor: usize,
    pub paper_cursor: usize,
    pub sort_order: SortOrder,
    /// Maps visual row index → paper index (recomputed on sort/tick).
    pub queue_sorted: Vec<usize>,
    pub tick: usize,
    pub theme: Theme,
    pub should_quit: bool,
    pub batch_complete: bool,
    pub show_help: bool,
    pub detail_scroll: u16,
    /// Height of the visible table area (set on resize, used for page up/down).
    pub visible_rows: usize,

    // Phase 3 state
    pub input_mode: InputMode,
    pub search_query: String,
    pub queue_filter: QueueFilter,
    pub paper_filter: PaperFilter,
    pub paper_sort: PaperSortOrder,
    pub activity_panel_visible: bool,
    pub start_time: Option<Instant>,
    /// Frozen elapsed time (set on cancel or batch complete).
    pub frozen_elapsed: Option<std::time::Duration>,
    pub single_paper_mode: bool,

    // New screens/features
    pub activity: ActivityState,
    pub config_state: ConfigState,
    pub export_state: ExportState,

    /// Tick at which the banner should auto-transition (10 ticks = ~1 second).
    banner_dismiss_tick: Option<usize>,
    /// Whether to emit terminal bell (set on batch complete, consumed on next view).
    pub pending_bell: bool,

    // Phase 4 state
    /// Whether backend processing has been started (manual start).
    pub processing_started: bool,
    /// Channel to send commands to the backend listener.
    pub backend_cmd_tx: Option<mpsc::UnboundedSender<BackendCommand>>,
    /// PDF file paths (kept for deferred processing).
    pub pdf_paths: Vec<PathBuf>,
    /// Last table area rendered (for mouse click → row mapping).
    pub last_table_area: Option<Rect>,
    /// Throughput counter: refs completed since last throughput bucket push.
    throughput_since_last: u16,
    /// Tick count of last throughput push.
    last_throughput_tick: usize,
    /// File picker state.
    pub file_picker: FilePickerState,
    /// Temp directory for extracted archive PDFs (auto-cleanup on drop).
    pub temp_dir: Option<tempfile::TempDir>,
}

impl App {
    pub fn new(filenames: Vec<String>, theme: Theme) -> Self {
        let papers: Vec<PaperState> = filenames.into_iter().map(PaperState::new).collect();
        let paper_count = papers.len();
        let ref_states = vec![Vec::new(); paper_count];
        let paper_refs = vec![Vec::new(); paper_count];
        let queue_sorted: Vec<usize> = (0..papers.len()).collect();

        Self {
            screen: Screen::Banner,
            papers,
            ref_states,
            paper_refs,
            queue_cursor: 0,
            paper_cursor: 0,
            sort_order: SortOrder::Original,
            queue_sorted,
            tick: 0,
            theme,
            should_quit: false,
            batch_complete: false,
            show_help: false,
            detail_scroll: 0,
            visible_rows: 20,
            input_mode: InputMode::Normal,
            search_query: String::new(),
            queue_filter: QueueFilter::All,
            paper_filter: PaperFilter::All,
            paper_sort: PaperSortOrder::RefNumber,
            activity_panel_visible: true,
            start_time: None,
            frozen_elapsed: None,
            single_paper_mode: false,
            activity: ActivityState::default(),
            config_state: ConfigState::default(),
            export_state: ExportState::default(),
            banner_dismiss_tick: Some(30),
            pending_bell: false,
            processing_started: false,
            backend_cmd_tx: None,
            pdf_paths: Vec::new(),
            last_table_area: None,
            throughput_since_last: 0,
            last_throughput_tick: 0,
            file_picker: FilePickerState::new(),
            temp_dir: None,
        }
    }

    /// Recompute `queue_sorted` based on the current `sort_order`, filter, and search.
    pub fn recompute_sorted_indices(&mut self) {
        let mut indices = filtered_indices(&self.papers, self.queue_filter, &self.search_query);
        match self.sort_order {
            SortOrder::Original => {}
            SortOrder::Problems => {
                indices.sort_by(|&a, &b| {
                    self.papers[b]
                        .problems()
                        .cmp(&self.papers[a].problems())
                        .then_with(|| a.cmp(&b))
                });
            }
            SortOrder::Name => {
                indices.sort_by(|&a, &b| self.papers[a].filename.cmp(&self.papers[b].filename));
            }
        }
        self.queue_sorted = indices;
    }

    /// Get sorted/filtered reference indices for the paper view.
    pub fn paper_ref_indices(&self, paper_index: usize) -> Vec<usize> {
        let refs = &self.ref_states[paper_index];
        let mut indices: Vec<usize> = (0..refs.len()).collect();

        if self.paper_filter == PaperFilter::ProblemsOnly {
            indices.retain(|&i| {
                refs[i].result.as_ref().map_or(false, |r| {
                    r.status != hallucinator_core::Status::Verified
                        || r.retraction_info
                            .as_ref()
                            .map_or(false, |ri| ri.is_retracted)
                })
            });
        }

        if !self.search_query.is_empty() {
            let query = self.search_query.to_lowercase();
            indices.retain(|&i| refs[i].title.to_lowercase().contains(&query));
        }

        match self.paper_sort {
            PaperSortOrder::RefNumber => {}
            PaperSortOrder::Verdict => {
                indices.sort_by(|&a, &b| {
                    let va = verdict_sort_key(&refs[a]);
                    let vb = verdict_sort_key(&refs[b]);
                    va.cmp(&vb).then_with(|| a.cmp(&b))
                });
            }
            PaperSortOrder::Source => {
                indices.sort_by(|&a, &b| {
                    let sa = refs[a].source_label();
                    let sb = refs[b].source_label();
                    sa.cmp(sb).then_with(|| a.cmp(&b))
                });
            }
        }

        indices
    }

    /// Send a start command to the backend if not already started.
    pub fn start_processing(&mut self) {
        if self.processing_started || self.pdf_paths.is_empty() {
            return;
        }
        self.processing_started = true;
        self.batch_complete = false;
        self.start_time = Some(Instant::now());
        self.frozen_elapsed = None;
        self.activity = ActivityState::default();
        self.throughput_since_last = 0;
        self.last_throughput_tick = self.tick;

        // Reset all paper/ref state to avoid double-counting on restart
        for paper in &mut self.papers {
            paper.phase = PaperPhase::Queued;
            paper.total_refs = 0;
            paper.stats = hallucinator_core::CheckStats::default();
            paper.results.clear();
            paper.error = None;
        }
        for rs in &mut self.ref_states {
            rs.clear();
        }
        for pr in &mut self.paper_refs {
            pr.clear();
        }

        if let Some(tx) = &self.backend_cmd_tx {
            let config = self.build_config();
            let _ = tx.send(BackendCommand::ProcessFiles {
                files: self.pdf_paths.clone(),
                starting_index: 0,
                max_concurrent_papers: self.config_state.max_concurrent_papers,
                config,
            });
        }
    }

    /// Build a `hallucinator_core::Config` from the current ConfigState.
    fn build_config(&self) -> hallucinator_core::Config {
        let disabled_dbs: Vec<String> = self
            .config_state
            .disabled_dbs
            .iter()
            .filter(|(_, enabled)| !enabled)
            .map(|(name, _)| name.clone())
            .collect();

        hallucinator_core::Config {
            openalex_key: if self.config_state.openalex_key.is_empty() {
                None
            } else {
                Some(self.config_state.openalex_key.clone())
            },
            s2_api_key: if self.config_state.s2_api_key.is_empty() {
                None
            } else {
                Some(self.config_state.s2_api_key.clone())
            },
            dblp_offline_path: if self.config_state.dblp_offline_path.is_empty() {
                None
            } else {
                Some(std::path::PathBuf::from(
                    &self.config_state.dblp_offline_path,
                ))
            },
            dblp_offline_db: None, // Populated from main.rs
            max_concurrent_refs: self.config_state.max_concurrent_refs,
            db_timeout_secs: self.config_state.db_timeout_secs,
            db_timeout_short_secs: self.config_state.db_timeout_short_secs,
            disabled_dbs,
            check_openalex_authors: false,
        }
    }

    /// Add files from file picker to the paper queue.
    /// PDFs are added directly. Archives are extracted and each PDF inside is added.
    pub fn add_files_from_picker(&mut self) {
        let new_files: Vec<PathBuf> = self.file_picker.selected.drain(..).collect();
        if new_files.is_empty() {
            return;
        }

        for path in &new_files {
            if hallucinator_pdf::archive::is_archive_path(path) {
                // Ensure temp_dir exists for extracted PDFs
                if self.temp_dir.is_none() {
                    match tempfile::tempdir() {
                        Ok(td) => self.temp_dir = Some(td),
                        Err(e) => {
                            self.activity.log(format!(
                                "Failed to create temp dir: {}",
                                e
                            ));
                            continue;
                        }
                    }
                }
                let dir = self.temp_dir.as_ref().unwrap().path();

                match hallucinator_pdf::archive::extract_archive(path, dir) {
                    Ok(extracted) => {
                        let archive_name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        let count = extracted.len();
                        for pdf in extracted {
                            let display_name =
                                format!("{}/{}", archive_name, pdf.filename);
                            self.papers.push(PaperState::new(display_name));
                            self.ref_states.push(Vec::new());
                            self.paper_refs.push(Vec::new());
                            self.pdf_paths.push(pdf.path);
                        }
                        self.activity.log(format!(
                            "Extracted {} PDF{} from {}",
                            count,
                            if count == 1 { "" } else { "s" },
                            archive_name,
                        ));
                    }
                    Err(e) => {
                        self.activity.log(format!(
                            "Archive error ({}): {}",
                            path.file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_default(),
                            e
                        ));
                    }
                }
            } else {
                let filename = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.display().to_string());
                self.papers.push(PaperState::new(filename));
                self.ref_states.push(Vec::new());
                self.paper_refs.push(Vec::new());
                self.pdf_paths.push(path.clone());
            }
        }
        self.recompute_sorted_indices();
    }

    /// Process a user action and update state. Returns true if the app should quit.
    pub fn update(&mut self, action: Action) -> bool {
        // Export modal intercepts
        if self.export_state.active {
            match action {
                Action::Quit => {
                    self.should_quit = true;
                    return true;
                }
                Action::NavigateBack => {
                    self.export_state.active = false;
                }
                Action::MoveDown => {
                    self.export_state.cursor = (self.export_state.cursor + 1).min(3);
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
                        self.export_state.scope = match self.export_state.scope {
                            crate::view::export::ExportScope::ThisPaper => {
                                crate::view::export::ExportScope::AllPapers
                            }
                            crate::view::export::ExportScope::AllPapers => {
                                crate::view::export::ExportScope::ThisPaper
                            }
                        };
                    }
                    3 => {
                        let path = format!(
                            "{}.{}",
                            self.export_state.output_path,
                            self.export_state.format.extension()
                        );
                        let data = crate::export::collect_paper_data(&self.papers);
                        let refs: Vec<(String, &[Option<hallucinator_core::ValidationResult>])> =
                            data.iter()
                                .map(|(name, results)| (name.clone(), results.as_slice()))
                                .collect();
                        match crate::export::export_results(
                            &refs,
                            self.export_state.format,
                            std::path::Path::new(&path),
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
                    self.should_quit = true;
                    return true;
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

        // Banner auto-dismiss on any key
        if self.screen == Screen::Banner {
            match action {
                Action::Quit => {
                    self.should_quit = true;
                    return true;
                }
                Action::Tick => {
                    self.tick = self.tick.wrapping_add(1);
                    if let Some(dismiss) = self.banner_dismiss_tick {
                        if self.tick >= dismiss {
                            self.dismiss_banner();
                        }
                    }
                }
                Action::Resize(_w, h) => {
                    self.visible_rows = (h as usize).saturating_sub(11);
                }
                _ => {
                    self.dismiss_banner();
                }
            }
            return false;
        }

        // File picker screen
        if self.screen == Screen::FilePicker {
            match action {
                Action::Quit => {
                    self.should_quit = true;
                    return true;
                }
                Action::NavigateBack => {
                    // Esc: add any selected files, go back to queue
                    if !self.file_picker.selected.is_empty() {
                        self.add_files_from_picker();
                    }
                    self.screen = Screen::Queue;
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
                Action::StartProcessing => {
                    // Space: toggle selection of current entry
                    self.file_picker.toggle_selected();
                }
                Action::DrillIn => {
                    // Enter on directory: open it. Enter on PDF: toggle selection.
                    if !self.file_picker.enter_directory() {
                        self.file_picker.toggle_selected();
                    }
                }
                Action::OpenConfig => {
                    self.config_state.prev_screen = Some(self.screen.clone());
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
            return false;
        }

        match action {
            Action::Quit => {
                self.should_quit = true;
                return true;
            }
            Action::ToggleHelp => {
                self.show_help = true;
            }
            Action::NavigateBack => match &self.screen {
                Screen::RefDetail(paper_idx, _) => {
                    let paper_idx = *paper_idx;
                    self.screen = Screen::Paper(paper_idx);
                }
                Screen::Paper(_) => {
                    if !self.single_paper_mode {
                        self.screen = Screen::Queue;
                        self.paper_cursor = 0;
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
                    self.input_mode = InputMode::Normal;

                    if let Some(prev) = self.config_state.prev_screen.clone() {
                        self.screen = prev;
                    } else {
                        self.screen = Screen::Queue;
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
                    self.recompute_sorted_indices();
                }
                Screen::Paper(_) => {
                    self.paper_sort = self.paper_sort.next();
                }
                _ => {}
            },
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
            Action::SearchInput(c) => {
                if self.config_state.editing {
                    // Route to config text editing
                    if c == '\x08' {
                        self.config_state.edit_buffer.pop();
                    } else {
                        self.config_state.edit_buffer.push(c);
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
                self.config_state.prev_screen = Some(self.screen.clone());
                self.screen = Screen::Config;
            }
            Action::Export => {
                self.export_state.active = true;
                self.export_state.cursor = 0;
                self.export_state.message = None;
            }
            Action::StartProcessing => {
                match &self.screen {
                    Screen::Queue => {
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
                        }
                    }
                    Screen::Paper(idx) => {
                        // Space on paper: toggle marked_safe on current reference
                        let idx = *idx;
                        let indices = self.paper_ref_indices(idx);
                        if self.paper_cursor < indices.len() {
                            let ref_idx = indices[self.paper_cursor];
                            if let Some(refs) = self.ref_states.get_mut(idx) {
                                if let Some(rs) = refs.get_mut(ref_idx) {
                                    rs.marked_safe = !rs.marked_safe;
                                }
                            }
                        }
                    }
                    Screen::RefDetail(paper_idx, ref_idx) => {
                        // Space on detail: toggle marked_safe
                        let paper_idx = *paper_idx;
                        let ref_idx = *ref_idx;
                        if let Some(refs) = self.ref_states.get_mut(paper_idx) {
                            if let Some(rs) = refs.get_mut(ref_idx) {
                                rs.marked_safe = !rs.marked_safe;
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
                self.screen = Screen::FilePicker;
            }
            Action::CopyToClipboard => {
                if let Some(text) = self.get_copyable_text() {
                    osc52_copy(&text);
                    self.activity.log("Copied to clipboard".to_string());
                }
            }
            Action::Retry
            | Action::RetryAll
            | Action::RemovePaper
            | Action::SaveConfig => {
                // Placeholder for future implementation
            }
            Action::Tick => {
                self.tick = self.tick.wrapping_add(1);
                if self.screen == Screen::Queue {
                    self.recompute_sorted_indices();
                }
                // Throughput tracking: push a bucket every 10 ticks (~1 second)
                if self.tick.wrapping_sub(self.last_throughput_tick) >= 10 {
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

    /// Handle mouse click → row selection.
    fn handle_click(&mut self, _x: u16, y: u16) {
        if let Some(table_area) = self.last_table_area {
            if y >= table_area.y && y < table_area.y + table_area.height {
                // Account for border (1) + header row (1) = offset 2 from table_area.y
                let row_offset = 2u16;
                if y >= table_area.y + row_offset {
                    let clicked_row = (y - table_area.y - row_offset) as usize;
                    match &self.screen {
                        Screen::Queue => {
                            if clicked_row < self.queue_sorted.len() {
                                self.queue_cursor = clicked_row;
                            }
                        }
                        Screen::Paper(idx) => {
                            let indices = self.paper_ref_indices(*idx);
                            if clicked_row < indices.len() {
                                self.paper_cursor = clicked_row;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// Get the number of items in the current config section.
    fn config_section_item_count(&self) -> usize {
        use crate::model::config::ConfigSection;
        match self.config_state.section {
            ConfigSection::ApiKeys => 2,
            ConfigSection::Databases => 1 + self.config_state.disabled_dbs.len(),
            ConfigSection::Concurrency => 4,
            ConfigSection::Display => 1,
        }
    }

    /// Handle Enter on Config screen (start editing a field).
    fn handle_config_enter(&mut self) {
        use crate::model::config::ConfigSection;
        match self.config_state.section {
            ConfigSection::ApiKeys => {
                let value = match self.config_state.item_cursor {
                    0 => self.config_state.openalex_key.clone(),
                    1 => self.config_state.s2_api_key.clone(),
                    _ => return,
                };
                self.config_state.editing = true;
                self.config_state.edit_buffer = value;
                self.input_mode = InputMode::TextInput;
            }
            ConfigSection::Concurrency => {
                let value = match self.config_state.item_cursor {
                    0 => self.config_state.max_concurrent_papers.to_string(),
                    1 => self.config_state.max_concurrent_refs.to_string(),
                    2 => self.config_state.db_timeout_secs.to_string(),
                    3 => self.config_state.db_timeout_short_secs.to_string(),
                    _ => return,
                };
                self.config_state.editing = true;
                self.config_state.edit_buffer = value;
                self.input_mode = InputMode::TextInput;
            }
            ConfigSection::Display => {
                // Cycle theme — update both the name and the live theme struct
                if self.config_state.theme_name == "hacker" {
                    self.config_state.theme_name = "modern".to_string();
                    self.theme = Theme::modern();
                } else {
                    self.config_state.theme_name = "hacker".to_string();
                    self.theme = Theme::hacker();
                }
            }
            ConfigSection::Databases => {
                if self.config_state.item_cursor == 0 {
                    // Item 0: edit DBLP offline path
                    self.config_state.editing = true;
                    self.config_state.edit_buffer = self.config_state.dblp_offline_path.clone();
                    self.input_mode = InputMode::TextInput;
                } else {
                    // Items 1+: toggle DB (same as space)
                    self.handle_config_space();
                }
            }
        }
    }

    /// Handle Space on Config screen (toggle database or cycle theme).
    fn handle_config_space(&mut self) {
        use crate::model::config::ConfigSection;
        match self.config_state.section {
            ConfigSection::Databases => {
                // Items 1+ are DB toggles (item 0 is DBLP path)
                if self.config_state.item_cursor >= 1 {
                    let toggle_idx = self.config_state.item_cursor - 1;
                    if let Some((_, enabled)) = self.config_state.disabled_dbs.get_mut(toggle_idx) {
                        *enabled = !*enabled;
                    }
                }
            }
            ConfigSection::Display => {
                // Space on Display also cycles theme (same as Enter)
                if self.config_state.theme_name == "hacker" {
                    self.config_state.theme_name = "modern".to_string();
                    self.theme = Theme::modern();
                } else {
                    self.config_state.theme_name = "hacker".to_string();
                    self.theme = Theme::hacker();
                }
            }
            _ => {}
        }
    }

    /// Confirm a config text edit.
    fn confirm_config_edit(&mut self) {
        use crate::model::config::ConfigSection;
        let buf = self.config_state.edit_buffer.clone();
        match self.config_state.section {
            ConfigSection::ApiKeys => match self.config_state.item_cursor {
                0 => self.config_state.openalex_key = buf,
                1 => self.config_state.s2_api_key = buf,
                _ => {}
            },
            ConfigSection::Concurrency => match self.config_state.item_cursor {
                0 => {
                    if let Ok(v) = buf.parse::<usize>() {
                        self.config_state.max_concurrent_papers = v.max(1);
                    }
                }
                1 => {
                    if let Ok(v) = buf.parse::<usize>() {
                        self.config_state.max_concurrent_refs = v.max(1);
                    }
                }
                2 => {
                    if let Ok(v) = buf.parse::<u64>() {
                        self.config_state.db_timeout_secs = v.max(1);
                    }
                }
                3 => {
                    if let Ok(v) = buf.parse::<u64>() {
                        self.config_state.db_timeout_short_secs = v.max(1);
                    }
                }
                _ => {}
            },
            ConfigSection::Databases => {
                if self.config_state.item_cursor == 0 {
                    self.config_state.dblp_offline_path = buf;
                }
            }
            _ => {}
        }
        self.config_state.editing = false;
        self.config_state.edit_buffer.clear();
        self.input_mode = InputMode::Normal;
    }

    /// Process a backend event and update model state.
    pub fn handle_backend_event(&mut self, event: BackendEvent) {
        match event {
            BackendEvent::ExtractionStarted { paper_index } => {
                if let Some(paper) = self.papers.get_mut(paper_index) {
                    paper.phase = PaperPhase::Extracting;
                }
            }
            BackendEvent::ExtractionComplete {
                paper_index,
                ref_count,
                ref_titles,
                references,
                skip_stats: _,
            } => {
                if let Some(paper) = self.papers.get_mut(paper_index) {
                    paper.total_refs = ref_count;
                    paper.init_results(ref_count);
                    paper.phase = PaperPhase::Checking;
                }
                if paper_index < self.paper_refs.len() {
                    self.paper_refs[paper_index] = references;
                }
                if paper_index < self.ref_states.len() {
                    self.ref_states[paper_index] = ref_titles
                        .into_iter()
                        .enumerate()
                        .map(|(i, title)| RefState {
                            index: i,
                            title,
                            phase: RefPhase::Pending,
                            result: None,
                            marked_safe: false,
                        })
                        .collect();
                }
            }
            BackendEvent::ExtractionFailed { paper_index, error } => {
                if let Some(paper) = self.papers.get_mut(paper_index) {
                    paper.phase = PaperPhase::ExtractionFailed;
                    paper.error = Some(error);
                }
            }
            BackendEvent::Progress { paper_index, event } => {
                self.handle_progress(paper_index, event);
            }
            BackendEvent::PaperComplete {
                paper_index,
                results: _,
            } => {
                if let Some(paper) = self.papers.get_mut(paper_index) {
                    if paper.phase != PaperPhase::ExtractionFailed {
                        paper.phase = PaperPhase::Complete;
                    }
                }
            }
            BackendEvent::BatchComplete => {
                self.frozen_elapsed = Some(self.elapsed());
                self.batch_complete = true;
                self.pending_bell = true;
            }
        }
    }

    fn handle_progress(&mut self, paper_index: usize, event: ProgressEvent) {
        match event {
            ProgressEvent::Checking { index, title, .. } => {
                if let Some(refs) = self.ref_states.get_mut(paper_index) {
                    if let Some(rs) = refs.get_mut(index) {
                        rs.phase = RefPhase::Checking;
                    }
                }
                // Track active query
                self.activity.active_queries.push(ActiveQuery {
                    db_name: format!("ref #{}", index + 1),
                    ref_title: title,
                    started_tick: self.tick,
                });
            }
            ProgressEvent::Result { index, result, .. } => {
                if let Some(paper) = self.papers.get_mut(paper_index) {
                    paper.record_result(index, result.clone());
                }
                if let Some(refs) = self.ref_states.get_mut(paper_index) {
                    if let Some(rs) = refs.get_mut(index) {
                        rs.phase = RefPhase::Done;
                        // Remove matching active query
                        let title = rs.title.clone();
                        self.activity
                            .active_queries
                            .retain(|q| q.ref_title != title);
                        rs.result = Some(result);
                    }
                }
                self.activity.total_completed += 1;
                self.throughput_since_last += 1;
            }
            ProgressEvent::Warning { .. } => {}
            ProgressEvent::RetryPass { .. } => {
                if let Some(paper) = self.papers.get_mut(paper_index) {
                    paper.phase = PaperPhase::Retrying;
                }
            }
            ProgressEvent::DatabaseQueryComplete {
                ref_index: _,
                db_name,
                status,
                elapsed,
                ..
            } => {
                // Skip recording Skipped status — those are early-exit artifacts, not real queries
                if status != DbStatus::Skipped {
                    let success = matches!(
                        status,
                        DbStatus::Match | DbStatus::NoMatch | DbStatus::AuthorMismatch
                    );
                    self.activity.record_db_complete(
                        &db_name,
                        success,
                        elapsed.as_secs_f64() * 1000.0,
                    );
                }
            }
        }
    }

    /// Elapsed processing time. Returns zero before processing starts,
    /// frozen value after cancel/complete, or live value during processing.
    pub fn elapsed(&self) -> std::time::Duration {
        if let Some(frozen) = self.frozen_elapsed {
            frozen
        } else if let Some(start) = self.start_time {
            start.elapsed()
        } else {
            std::time::Duration::ZERO
        }
    }

    /// Dismiss the banner and navigate to the appropriate first screen.
    fn dismiss_banner(&mut self) {
        self.banner_dismiss_tick = None;
        if self.single_paper_mode {
            self.screen = Screen::Paper(0);
        } else if self.papers.is_empty() {
            self.screen = Screen::FilePicker;
        } else {
            self.screen = Screen::Queue;
        }
    }

    /// Get text to copy for the current screen context.
    fn get_copyable_text(&self) -> Option<String> {
        match &self.screen {
            Screen::RefDetail(paper_idx, ref_idx) => {
                let rs = self.ref_states.get(*paper_idx)?.get(*ref_idx)?;
                if let Some(result) = &rs.result {
                    if !result.raw_citation.is_empty() {
                        return Some(result.raw_citation.clone());
                    }
                }
                Some(rs.title.clone())
            }
            Screen::Paper(idx) => {
                let indices = self.paper_ref_indices(*idx);
                let ref_idx = indices.get(self.paper_cursor)?;
                let rs = self.ref_states.get(*idx)?.get(*ref_idx)?;
                Some(rs.title.clone())
            }
            _ => None,
        }
    }

    /// Render the current screen.
    pub fn view(&mut self, f: &mut ratatui::Frame) {
        // Emit terminal bell if pending
        if self.pending_bell {
            self.pending_bell = false;
            eprint!("\x07");
        }

        let area = f.area();

        // Banner screen renders as centered overlay with no activity panel
        if self.screen == Screen::Banner {
            crate::view::banner::render(f, &self.theme, self.tick);
            return;
        }

        // File picker renders without activity panel
        if self.screen == Screen::FilePicker {
            crate::view::file_picker::render_in(f, self, area);
            return;
        }

        // Persistent logo bar at top of every content screen
        let content_area = crate::view::banner::render_logo_bar(f, area, &self.theme, self.tick);

        // Activity panel split
        let main_area = if self.activity_panel_visible {
            let panel_width = if content_area.width > 120 {
                45
            } else {
                (content_area.width / 3).max(30)
            };
            let chunks = Layout::horizontal([Constraint::Min(40), Constraint::Length(panel_width)])
                .split(content_area);

            crate::view::activity::render(f, chunks[1], self);
            chunks[0]
        } else {
            content_area
        };

        // Clone screen to avoid borrow conflict with &mut self
        let screen = self.screen.clone();
        match screen {
            Screen::Queue => crate::view::queue::render_in(f, self, main_area),
            Screen::Paper(idx) => crate::view::paper::render_in(f, self, idx, main_area),
            Screen::RefDetail(paper_idx, ref_idx) => {
                crate::view::detail::render_in(f, self, paper_idx, ref_idx, main_area)
            }
            Screen::Config => crate::view::config::render_in(f, self, main_area),
            Screen::Banner | Screen::FilePicker => unreachable!(),
        }

        if self.export_state.active {
            crate::view::export::render(f, self);
        }

        if self.show_help {
            crate::view::help::render(f, &self.theme);
        }
    }
}

/// Copy text to the system clipboard via OSC 52 escape sequence.
/// Works in Ghostty, iTerm2, kitty, WezTerm, and most modern terminals.
fn osc52_copy(text: &str) {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    // Write directly to stdout, bypassing the terminal backend buffer
    let _ = std::io::stdout().write_all(format!("\x1b]52;c;{}\x07", encoded).as_bytes());
    let _ = std::io::stdout().flush();
}

fn verdict_sort_key(rs: &RefState) -> u8 {
    match &rs.result {
        Some(r) => {
            if r.retraction_info
                .as_ref()
                .map_or(false, |ri| ri.is_retracted)
            {
                0
            } else {
                match r.status {
                    hallucinator_core::Status::NotFound => 1,
                    hallucinator_core::Status::AuthorMismatch => 2,
                    hallucinator_core::Status::Verified => 3,
                }
            }
        }
        None => 4,
    }
}
