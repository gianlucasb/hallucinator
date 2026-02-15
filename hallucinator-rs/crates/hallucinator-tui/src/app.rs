use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

use ratatui::layout::{Constraint, Layout, Rect};
use tokio::sync::mpsc;

use hallucinator_pdf::archive::ArchiveItem;

use hallucinator_core::{DbStatus, ProgressEvent, Reference};

use crate::action::Action;
use crate::model::activity::{ActiveQuery, ActivityState};
use crate::model::config::ConfigState;
use crate::model::paper::{FpReason, PaperFilter, PaperSortOrder, RefPhase, RefState};
use crate::model::queue::{
    PaperPhase, PaperState, PaperVerdict, QueueFilter, SortOrder, filtered_indices,
};
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

/// Context for the file picker — determines what kind of file we're picking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilePickerContext {
    /// Normal mode: selecting PDFs, .bbl, archives, etc.
    AddFiles,
    /// Selecting a .db/.sqlite file for a config database path.
    SelectDatabase {
        /// 0 = DBLP offline path, 1 = ACL offline path
        config_item: usize,
    },
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
    pub is_bbl: bool,
    pub is_bib: bool,
    pub is_archive: bool,
    pub is_json: bool,
    pub is_db: bool,
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
                is_bbl: false,
                is_bib: false,
                is_archive: false,
                is_json: false,
                is_db: false,
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
                        is_bbl: false,
                        is_bib: false,
                        is_archive: false,
                        is_json: false,
                        is_db: false,
                    });
                } else {
                    let ext = path.extension().and_then(|e| e.to_str());
                    let is_pdf = ext.map(|e| e.eq_ignore_ascii_case("pdf")).unwrap_or(false);
                    let is_bbl = ext.map(|e| e.eq_ignore_ascii_case("bbl")).unwrap_or(false);
                    let is_bib = ext.map(|e| e.eq_ignore_ascii_case("bib")).unwrap_or(false);
                    let is_archive = hallucinator_pdf::archive::is_archive_path(&path);
                    let is_json = ext.map(|e| e.eq_ignore_ascii_case("json")).unwrap_or(false);
                    let is_db = ext
                        .map(|e| e.eq_ignore_ascii_case("db") || e.eq_ignore_ascii_case("sqlite"))
                        .unwrap_or(false);
                    files.push(FileEntry {
                        name,
                        path,
                        is_dir: false,
                        is_pdf,
                        is_bbl,
                        is_bib,
                        is_archive,
                        is_json,
                        is_db,
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

    /// Toggle selection of the current entry (PDFs, .bbl, .bib files, archives, .json results, and .db/.sqlite).
    pub fn toggle_selected(&mut self) {
        if let Some(entry) = self.entries.get(self.cursor)
            && (entry.is_pdf
                || entry.is_bbl
                || entry.is_bib
                || entry.is_archive
                || entry.is_json
                || entry.is_db)
        {
            if let Some(pos) = self.selected.iter().position(|p| p == &entry.path) {
                self.selected.remove(pos);
            } else {
                self.selected.push(entry.path.clone());
            }
        }
    }

    /// Enter the directory at cursor, or return false if not a directory.
    pub fn enter_directory(&mut self) -> bool {
        if let Some(entry) = self.entries.get(self.cursor)
            && entry.is_dir
        {
            self.current_dir = entry.path.clone();
            self.refresh_entries();
            return true;
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

    /// Tick at which the banner should auto-transition.
    pub banner_dismiss_tick: Option<usize>,
    /// Whether to emit terminal bell (set on batch complete, consumed on next view).
    pub pending_bell: bool,

    // Phase 4 state
    /// Whether backend processing has been started (manual start).
    pub processing_started: bool,
    /// Channel to send commands to the backend listener.
    pub backend_cmd_tx: Option<mpsc::UnboundedSender<BackendCommand>>,
    /// Input file paths — PDF or .bbl (kept for deferred processing).
    pub file_paths: Vec<PathBuf>,
    /// Last table area rendered (for mouse click → row mapping).
    pub last_table_area: Option<Rect>,
    /// Throughput counter: refs completed since last throughput bucket push.
    throughput_since_last: u16,
    /// Tick count of last throughput push.
    last_throughput_tick: usize,
    /// File picker state.
    pub file_picker: FilePickerState,
    /// Context for the file picker (add files vs select database).
    pub file_picker_context: FilePickerContext,
    /// Temp directory for extracted archive PDFs (auto-cleanup on drop).
    pub temp_dir: Option<tempfile::TempDir>,
    /// Archives waiting to be extracted (processed one per tick for UI responsiveness).
    pub pending_archive_extractions: Vec<PathBuf>,
    /// Name of the archive currently being extracted (shown in UI).
    pub extracting_archive: Option<String>,
    /// Receiver for streaming archive extraction (PDFs arrive one at a time).
    archive_rx: Option<std::sync::mpsc::Receiver<ArchiveItem>>,
    /// Name of the archive being streamed (for display name prefix).
    archive_streaming_name: Option<String>,
    /// Number of PDFs extracted so far from the current archive.
    pub extracted_count: usize,
    /// Frame counter for FPS measurement.
    frame_count: u32,
    /// Last time FPS was sampled.
    last_fps_instant: Instant,
    /// Measured FPS for display.
    pub measured_fps: f32,
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
            sort_order: SortOrder::ProblematicPct,
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
            paper_sort: PaperSortOrder::Verdict,
            activity_panel_visible: true,
            start_time: None,
            frozen_elapsed: None,
            single_paper_mode: false,
            activity: ActivityState::default(),
            config_state: ConfigState::default(),
            export_state: ExportState::default(),
            banner_dismiss_tick: None, // set after config_state is applied
            pending_bell: false,
            processing_started: false,
            backend_cmd_tx: None,
            file_paths: Vec::new(),
            last_table_area: None,
            throughput_since_last: 0,
            last_throughput_tick: 0,
            file_picker: FilePickerState::new(),
            file_picker_context: FilePickerContext::AddFiles,
            temp_dir: None,
            pending_archive_extractions: Vec::new(),
            extracting_archive: None,
            archive_rx: None,
            archive_streaming_name: None,
            extracted_count: 0,
            frame_count: 0,
            last_fps_instant: Instant::now(),
            measured_fps: 0.0,
        }
    }

    /// Recompute `queue_sorted` based on the current `sort_order`, filter, and search.
    ///
    /// Stabilises the cursor: if the paper previously under the cursor is still
    /// present after filtering/sorting, the cursor follows it to its new row.
    pub fn recompute_sorted_indices(&mut self) {
        // Remember which paper the cursor is currently on.
        let prev_paper = self.queue_sorted.get(self.queue_cursor).copied();

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
            SortOrder::ProblematicPct => {
                indices.sort_by(|&a, &b| {
                    self.papers[b]
                        .problematic_pct()
                        .partial_cmp(&self.papers[a].problematic_pct())
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| a.cmp(&b))
                });
            }
            SortOrder::Name => {
                indices.sort_by(|&a, &b| self.papers[a].filename.cmp(&self.papers[b].filename));
            }
        }
        self.queue_sorted = indices;

        // Restore cursor to the same paper if it's still in the list.
        if let Some(paper_idx) = prev_paper {
            if let Some(new_pos) = self.queue_sorted.iter().position(|&i| i == paper_idx) {
                self.queue_cursor = new_pos;
            } else {
                // Paper was filtered out — clamp cursor.
                self.queue_cursor = self
                    .queue_cursor
                    .min(self.queue_sorted.len().saturating_sub(1));
            }
        }
    }

    /// Get sorted/filtered reference indices for the paper view.
    pub fn paper_ref_indices(&self, paper_index: usize) -> Vec<usize> {
        let refs = &self.ref_states[paper_index];
        let mut indices: Vec<usize> = (0..refs.len()).collect();

        if self.paper_filter == PaperFilter::ProblemsOnly {
            indices.retain(|&i| {
                refs[i].result.as_ref().is_some_and(|r| {
                    r.status != hallucinator_core::Status::Verified
                        || r.retraction_info.as_ref().is_some_and(|ri| ri.is_retracted)
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

    /// Get the paper index for the currently viewed paper (if any).
    fn current_paper_index(&self) -> Option<usize> {
        match self.screen {
            Screen::Paper(i) | Screen::RefDetail(i, _) => Some(i),
            Screen::Queue => self.queue_sorted.get(self.queue_cursor).copied(),
            _ => None,
        }
    }

    /// Derive an export filename stem from a paper's filename (strip extension).
    fn paper_export_stem(&self, paper_index: usize) -> String {
        if let Some(paper) = self.papers.get(paper_index) {
            std::path::Path::new(&paper.filename)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "hallucinator-results".to_string())
        } else {
            "hallucinator-results".to_string()
        }
    }

    /// Compute the default export filename based on scope and current paper.
    fn export_default_path(&self, scope: crate::view::export::ExportScope) -> String {
        match scope {
            crate::view::export::ExportScope::ThisPaper => self
                .current_paper_index()
                .map(|i| self.paper_export_stem(i))
                .unwrap_or_else(|| "hallucinator-results".to_string()),
            crate::view::export::ExportScope::AllPapers => "hallucinator-results".to_string(),
        }
    }

    /// Send a start command to the backend if not already started.
    pub fn start_processing(&mut self) {
        if self.processing_started {
            return;
        }

        // Filter out placeholder paths (from loaded results)
        let real_files: Vec<PathBuf> = self
            .file_paths
            .iter()
            .filter(|p| p.as_os_str() != "")
            .cloned()
            .collect();

        if real_files.is_empty() {
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
                files: real_files,
                starting_index: 0,
                max_concurrent_papers: self.config_state.max_concurrent_papers,
                config: Box::new(config),
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
            acl_offline_path: if self.config_state.acl_offline_path.is_empty() {
                None
            } else {
                Some(std::path::PathBuf::from(
                    &self.config_state.acl_offline_path,
                ))
            },
            acl_offline_db: None, // Populated from main.rs
            max_concurrent_refs: self.config_state.max_concurrent_refs,
            db_timeout_secs: self.config_state.db_timeout_secs,
            db_timeout_short_secs: self.config_state.db_timeout_short_secs,
            disabled_dbs,
            check_openalex_authors: false,
            crossref_mailto: if self.config_state.crossref_mailto.is_empty() {
                None
            } else {
                Some(self.config_state.crossref_mailto.clone())
            },
        }
    }

    /// Add files from file picker to the paper queue.
    /// PDFs are added directly. Archives are queued for deferred extraction
    /// (one per tick) so the UI can show progress. JSON result files are loaded
    /// and their papers added as already-complete entries.
    pub fn add_files_from_picker(&mut self) {
        let new_files: Vec<PathBuf> = self.file_picker.selected.drain(..).collect();
        if new_files.is_empty() {
            return;
        }

        for path in new_files {
            let is_json = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("json"))
                .unwrap_or(false);

            if is_json {
                match crate::load::load_results_file(&path) {
                    Ok(loaded) => {
                        let count = loaded.len();
                        for (paper, refs, paper_refs) in loaded {
                            self.papers.push(paper);
                            self.ref_states.push(refs);
                            self.paper_refs.push(paper_refs);
                            self.file_paths.push(PathBuf::new()); // placeholder
                        }
                        self.batch_complete = true;
                        self.processing_started = true;
                        self.activity.log(format!(
                            "Loaded {} paper{} from {}",
                            count,
                            if count == 1 { "" } else { "s" },
                            path.file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| path.display().to_string()),
                        ));
                    }
                    Err(e) => {
                        self.activity
                            .log_warn(format!("Failed to load {}: {}", path.display(), e));
                    }
                }
            } else if hallucinator_pdf::archive::is_archive_path(&path) {
                // Set extracting indicator for the first archive so it shows immediately
                if self.extracting_archive.is_none() {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    self.extracting_archive = Some(name);
                }
                self.pending_archive_extractions.push(path);
            } else {
                let filename = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.display().to_string());
                self.papers.push(PaperState::new(filename));
                self.ref_states.push(Vec::new());
                self.paper_refs.push(Vec::new());
                self.file_paths.push(path);
            }
        }
        self.recompute_sorted_indices();
    }

    /// Start streaming extraction for the next pending archive.
    /// Spawns a background thread that extracts PDFs one-by-one,
    /// sending them through a channel that the tick handler drains.
    fn start_next_archive_extraction(&mut self) {
        let path = match self.pending_archive_extractions.first() {
            Some(p) => p.clone(),
            None => {
                self.extracting_archive = None;
                return;
            }
        };

        let archive_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        self.extracting_archive = Some(archive_name.clone());
        self.archive_streaming_name = Some(archive_name.clone());
        self.extracted_count = 0;

        // Ensure temp_dir exists
        if self.temp_dir.is_none() {
            match tempfile::tempdir() {
                Ok(td) => self.temp_dir = Some(td),
                Err(e) => {
                    self.activity
                        .log(format!("Failed to create temp dir: {}", e));
                    self.pending_archive_extractions.remove(0);
                    self.extracting_archive = None;
                    return;
                }
            }
        }
        let dir = self.temp_dir.as_ref().unwrap().path().to_path_buf();

        let max_size = self.config_state.max_archive_size_mb as u64 * 1024 * 1024;

        let (tx, rx) = std::sync::mpsc::channel();
        self.archive_rx = Some(rx);

        // Spawn blocking extraction in a background thread
        tokio::task::spawn_blocking(move || {
            if let Err(e) =
                hallucinator_pdf::archive::extract_archive_streaming(&path, &dir, max_size, &tx)
            {
                // Send the error as a warning so the UI can display it;
                // Done{0} signals no PDFs were found.
                let _ = tx.send(ArchiveItem::Warning(e));
                let _ = tx.send(ArchiveItem::Done { total: 0 });
            }
        });
    }

    /// Drain the archive streaming channel, adding extracted PDFs to the queue.
    /// Returns true if the current archive finished (Done received or channel closed).
    fn drain_archive_channel(&mut self) -> bool {
        let rx = match &self.archive_rx {
            Some(rx) => rx,
            None => return false,
        };

        let archive_name = self.archive_streaming_name.clone().unwrap_or_default();
        let mut finished = false;
        let mut new_pdfs: Vec<PathBuf> = Vec::new();

        loop {
            match rx.try_recv() {
                Ok(ArchiveItem::Pdf(pdf)) => {
                    self.extracted_count += 1;
                    let display_name = format!("{}/{}", archive_name, pdf.filename);
                    self.papers.push(PaperState::new(display_name));
                    self.ref_states.push(Vec::new());
                    self.paper_refs.push(Vec::new());
                    new_pdfs.push(pdf.path.clone());
                    self.file_paths.push(pdf.path);
                }
                Ok(ArchiveItem::Warning(msg)) => {
                    self.activity.log_warn(msg);
                }
                Ok(ArchiveItem::Done { total }) => {
                    self.activity.log(format!(
                        "Extracted {} file{} from {}",
                        total,
                        if total == 1 { "" } else { "s" },
                        archive_name,
                    ));
                    finished = true;
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    // Sender dropped without Done — extraction thread panicked or errored
                    if self.extracted_count == 0 {
                        self.activity.log(format!(
                            "Archive error ({}): extraction failed",
                            archive_name
                        ));
                    }
                    finished = true;
                    break;
                }
            }
        }

        let got_new = !new_pdfs.is_empty();

        // If processing is already started, send newly extracted PDFs to backend
        if self.processing_started
            && got_new
            && let Some(tx) = &self.backend_cmd_tx
        {
            let starting_index = self.file_paths.len() - new_pdfs.len();
            let config = self.build_config();
            let _ = tx.send(BackendCommand::ProcessFiles {
                files: new_pdfs,
                starting_index,
                max_concurrent_papers: self.config_state.max_concurrent_papers,
                config: Box::new(config),
            });
        }

        if got_new {
            self.recompute_sorted_indices();
        }

        if finished {
            self.archive_rx = None;
            self.archive_streaming_name = None;
            self.pending_archive_extractions.remove(0);
            if self.pending_archive_extractions.is_empty() {
                self.extracting_archive = None;
            }
        }

        finished
    }

    /// Process a user action and update state. Returns true if the app should quit.
    pub fn update(&mut self, action: Action) -> bool {
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
                            self.export_state.edit_buffer.pop();
                        } else {
                            self.export_state.edit_buffer.push(ch);
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
                        self.export_state.output_path =
                            self.export_default_path(self.export_state.scope);
                    }
                    2 => {
                        // Start editing the output path
                        self.export_state.editing_path = true;
                        self.export_state.edit_buffer = self.export_state.output_path.clone();
                        self.input_mode = InputMode::TextInput;
                    }
                    3 => {
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
                        };
                        let papers: Vec<&crate::model::queue::PaperState> = paper_indices
                            .iter()
                            .filter_map(|&i| self.papers.get(i))
                            .collect();
                        let ref_states: Vec<&[RefState]> = paper_indices
                            .iter()
                            .filter_map(|&i| self.ref_states.get(i).map(|v| v.as_slice()))
                            .collect();
                        match crate::export::export_results(
                            &papers,
                            &ref_states,
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

        // Banner auto-dismiss on any key
        if self.screen == Screen::Banner {
            match action {
                Action::Quit => {
                    self.should_quit = true;
                    return true;
                }
                Action::Tick => {
                    self.tick = self.tick.wrapping_add(1);
                    if let Some(dismiss) = self.banner_dismiss_tick
                        && self.tick >= dismiss
                    {
                        self.dismiss_banner();
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
                    match &self.file_picker_context {
                        FilePickerContext::SelectDatabase { config_item } => {
                            // In db mode: if a file was selected, write it to config
                            let config_item = *config_item;
                            if let Some(path) = self.file_picker.selected.first() {
                                let canonical = path
                                    .canonicalize()
                                    .unwrap_or_else(|_| path.clone())
                                    .display()
                                    .to_string();
                                if config_item == 0 {
                                    self.config_state.dblp_offline_path = canonical;
                                } else {
                                    self.config_state.acl_offline_path = canonical;
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
                    if matches!(
                        self.file_picker_context,
                        FilePickerContext::SelectDatabase { .. }
                    ) {
                        // In db mode: single-select, only .db files
                        if let Some(entry) = self.file_picker.entries.get(self.file_picker.cursor)
                            && entry.is_db
                        {
                            self.file_picker.selected.clear();
                            self.file_picker.selected.push(entry.path.clone());
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
                                let canonical = entry
                                    .path
                                    .canonicalize()
                                    .unwrap_or_else(|_| entry.path.clone())
                                    .display()
                                    .to_string();
                                if let FilePickerContext::SelectDatabase { config_item } =
                                    self.file_picker_context
                                {
                                    if config_item == 0 {
                                        self.config_state.dblp_offline_path = canonical;
                                    } else {
                                        self.config_state.acl_offline_path = canonical;
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
                    && self.config_state.item_cursor <= 1
                {
                    // Open file picker in database selection mode
                    let config_item = self.config_state.item_cursor;
                    self.file_picker_context = FilePickerContext::SelectDatabase { config_item };
                    self.file_picker.selected.clear();

                    // Navigate to the current path's parent if set
                    let current_path = if config_item == 0 {
                        &self.config_state.dblp_offline_path
                    } else {
                        &self.config_state.acl_offline_path
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
                    osc52_copy(&text);
                    self.activity.log("Copied to clipboard".to_string());
                }
            }
            Action::SaveConfig => {
                self.save_config();
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
                self.frame_count += 1;

                // Measure FPS every second
                let elapsed = self.last_fps_instant.elapsed();
                if elapsed.as_secs_f32() >= 1.0 {
                    self.measured_fps = self.frame_count as f32 / elapsed.as_secs_f32();
                    self.frame_count = 0;
                    self.last_fps_instant = Instant::now();
                }

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

    /// Handle mouse click → row selection.
    fn handle_click(&mut self, _x: u16, y: u16) {
        if let Some(table_area) = self.last_table_area
            && y >= table_area.y
            && y < table_area.y + table_area.height
        {
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

    /// Get the number of items in the current config section.
    fn config_section_item_count(&self) -> usize {
        use crate::model::config::ConfigSection;
        match self.config_state.section {
            ConfigSection::ApiKeys => 3,
            ConfigSection::Databases => 2 + self.config_state.disabled_dbs.len(),
            ConfigSection::Concurrency => 5,
            ConfigSection::Display => 2, // theme + fps
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
                    2 => self.config_state.crossref_mailto.clone(),
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
                    4 => self.config_state.max_archive_size_mb.to_string(),
                    _ => return,
                };
                self.config_state.editing = true;
                self.config_state.edit_buffer = value;
                self.input_mode = InputMode::TextInput;
            }
            ConfigSection::Display => match self.config_state.item_cursor {
                0 => {
                    // Cycle theme — update both the name and the live theme struct
                    if self.config_state.theme_name == "hacker" {
                        self.config_state.theme_name = "modern".to_string();
                        self.theme = Theme::modern();
                    } else {
                        self.config_state.theme_name = "hacker".to_string();
                        self.theme = Theme::hacker();
                    }
                    self.config_state.dirty = true;
                }
                1 => {
                    // Edit FPS
                    self.config_state.editing = true;
                    self.config_state.edit_buffer = self.config_state.fps.to_string();
                    self.input_mode = InputMode::TextInput;
                }
                _ => {}
            },
            ConfigSection::Databases => {
                if self.config_state.item_cursor == 0 {
                    // Item 0: edit DBLP offline path
                    self.config_state.editing = true;
                    self.config_state.edit_buffer = self.config_state.dblp_offline_path.clone();
                    self.input_mode = InputMode::TextInput;
                } else if self.config_state.item_cursor == 1 {
                    // Item 1: edit ACL offline path
                    self.config_state.editing = true;
                    self.config_state.edit_buffer = self.config_state.acl_offline_path.clone();
                    self.input_mode = InputMode::TextInput;
                } else {
                    // Items 2+: toggle DB (same as space)
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
                // Items 2+ are DB toggles (items 0-1 are DBLP/ACL paths)
                if self.config_state.item_cursor >= 2 {
                    let toggle_idx = self.config_state.item_cursor - 2;
                    if let Some((_, enabled)) = self.config_state.disabled_dbs.get_mut(toggle_idx) {
                        *enabled = !*enabled;
                        self.config_state.dirty = true;
                    }
                }
            }
            ConfigSection::Display => {
                if self.config_state.item_cursor == 0 {
                    // Space on theme cycles it
                    if self.config_state.theme_name == "hacker" {
                        self.config_state.theme_name = "modern".to_string();
                        self.theme = Theme::modern();
                    } else {
                        self.config_state.theme_name = "hacker".to_string();
                        self.theme = Theme::hacker();
                    }
                    self.config_state.dirty = true;
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
                2 => self.config_state.crossref_mailto = buf,
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
                4 => {
                    if let Ok(v) = buf.parse::<u32>() {
                        self.config_state.max_archive_size_mb = v;
                    }
                }
                _ => {}
            },
            ConfigSection::Databases => match self.config_state.item_cursor {
                0 => {
                    self.config_state.dblp_offline_path = if buf.is_empty() {
                        buf
                    } else {
                        PathBuf::from(&buf)
                            .canonicalize()
                            .unwrap_or_else(|_| PathBuf::from(&buf))
                            .display()
                            .to_string()
                    };
                }
                1 => {
                    self.config_state.acl_offline_path = if buf.is_empty() {
                        buf
                    } else {
                        PathBuf::from(&buf)
                            .canonicalize()
                            .unwrap_or_else(|_| PathBuf::from(&buf))
                            .display()
                            .to_string()
                    };
                }
                _ => {}
            },
            ConfigSection::Display => {
                if self.config_state.item_cursor == 1
                    && let Ok(v) = buf.parse::<u32>()
                {
                    self.config_state.fps = v.clamp(1, 120);
                }
            }
        }
        self.config_state.dirty = true;
        self.config_state.editing = false;
        self.config_state.edit_buffer.clear();
        self.input_mode = InputMode::Normal;
    }

    /// Save config to disk and clear the dirty flag.
    fn save_config(&mut self) {
        let file_cfg = crate::config_file::from_config_state(&self.config_state);
        match crate::config_file::save_config(&file_cfg) {
            Ok(path) => {
                self.config_state.dirty = false;
                self.activity
                    .log(format!("Config saved to {}", path.display()));
            }
            Err(e) => {
                self.activity.log(format!("Config save failed: {}", e));
            }
        }
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
                    // Allocate result slots for ALL refs (including skipped) so
                    // that remapped indices from the backend fit.
                    paper.init_results(references.len());
                    paper.phase = PaperPhase::Checking;
                }
                if paper_index < self.ref_states.len() {
                    self.ref_states[paper_index] = ref_titles
                        .into_iter()
                        .zip(references.iter())
                        .map(|(title, r)| {
                            let phase = if let Some(reason) = &r.skip_reason {
                                RefPhase::Skipped(reason.clone())
                            } else {
                                RefPhase::Pending
                            };
                            RefState {
                                index: r.original_number.saturating_sub(1),
                                title,
                                phase,
                                result: None,
                                fp_reason: None,
                                raw_citation: r.raw_citation.clone(),
                                authors: r.authors.clone(),
                                doi: r.doi.clone(),
                                arxiv_id: r.arxiv_id.clone(),
                            }
                        })
                        .collect();
                }
                if paper_index < self.paper_refs.len() {
                    self.paper_refs[paper_index] = references;
                }
            }
            BackendEvent::ExtractionFailed { paper_index, error } => {
                if let Some(paper) = self.papers.get_mut(paper_index) {
                    paper.phase = PaperPhase::ExtractionFailed;
                    paper.error = Some(error);
                }
            }
            BackendEvent::Progress { paper_index, event } => {
                self.handle_progress(paper_index, *event);
            }
            BackendEvent::PaperComplete {
                paper_index,
                results: _,
            } => {
                if let Some(paper) = self.papers.get_mut(paper_index)
                    && paper.phase != PaperPhase::ExtractionFailed
                {
                    paper.phase = PaperPhase::Complete;
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
                if let Some(refs) = self.ref_states.get_mut(paper_index)
                    && let Some(rs) = refs.get_mut(index)
                {
                    rs.phase = RefPhase::Checking;
                }
                // Track active query
                self.activity.active_queries.push(ActiveQuery {
                    db_name: format!("ref #{}", index + 1),
                    ref_title: title,
                });
            }
            ProgressEvent::Result { index, result, .. } => {
                let result = *result;
                if let Some(paper) = self.papers.get_mut(paper_index) {
                    // Track retry progress
                    if paper.phase == PaperPhase::Retrying {
                        paper.retry_done += 1;
                    }
                    paper.record_result(index, result.clone());
                }
                if let Some(refs) = self.ref_states.get_mut(paper_index)
                    && let Some(rs) = refs.get_mut(index)
                {
                    rs.phase = RefPhase::Done;
                    // Remove matching active query
                    let title = rs.title.clone();
                    self.activity
                        .active_queries
                        .retain(|q| q.ref_title != title);
                    rs.result = Some(result);
                }
                self.activity.total_completed += 1;
                self.throughput_since_last += 1;
            }
            ProgressEvent::Warning { .. } => {}
            ProgressEvent::RetryPass { count, .. } => {
                if let Some(paper) = self.papers.get_mut(paper_index) {
                    paper.phase = PaperPhase::Retrying;
                    paper.retry_total = count;
                    paper.retry_done = 0;
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
                    let is_match = status == DbStatus::Match;
                    self.activity.record_db_complete(
                        &db_name,
                        success,
                        is_match,
                        elapsed.as_secs_f64() * 1000.0,
                    );

                    // Warn when DBLP online is repeatedly timing out and no offline DB is configured
                    if db_name == "DBLP"
                        && !success
                        && !self.activity.dblp_timeout_warned
                        && self.config_state.dblp_offline_path.is_empty()
                        && let Some(health) = self.activity.db_health.get("DBLP")
                        && health.failed >= 3
                    {
                        self.activity.log_warn(
                                    "DBLP online timing out repeatedly. Build an offline database: hallucinator-tui update-dblp".to_string()
                                );
                        self.activity.dblp_timeout_warned = true;
                    }
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
                if let Some(result) = &rs.result
                    && !result.raw_citation.is_empty()
                {
                    return Some(result.raw_citation.clone());
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

    /// Handle Ctrl+r: retry the currently selected reference.
    fn handle_retry_single(&mut self) {
        let (paper_idx, ref_idx) = match &self.screen {
            Screen::Paper(idx) => {
                let idx = *idx;
                let indices = self.paper_ref_indices(idx);
                if self.paper_cursor >= indices.len() {
                    return;
                }
                (idx, indices[self.paper_cursor])
            }
            Screen::RefDetail(paper_idx, ref_idx) => (*paper_idx, *ref_idx),
            _ => return,
        };

        let rs = match self.ref_states.get(paper_idx).and_then(|r| r.get(ref_idx)) {
            Some(rs) => rs,
            None => return,
        };

        // Determine what to retry
        let failed_dbs = match &rs.result {
            Some(r) => {
                if r.status == hallucinator_core::Status::Verified && r.failed_dbs.is_empty() {
                    self.activity.log("Already verified".to_string());
                    return;
                }
                r.failed_dbs.clone()
            }
            None => {
                self.activity.log("No result to retry".to_string());
                return;
            }
        };

        let reference = match self.paper_refs.get(paper_idx).and_then(|r| r.get(ref_idx)) {
            Some(r) => r.clone(),
            None => return,
        };

        // Mark as retrying
        if let Some(refs) = self.ref_states.get_mut(paper_idx)
            && let Some(rs) = refs.get_mut(ref_idx)
        {
            rs.phase = RefPhase::Retrying;
        }

        self.activity
            .log(format!("Retrying ref #{}...", ref_idx + 1));

        if let Some(tx) = &self.backend_cmd_tx {
            let config = self.build_config();
            let _ = tx.send(BackendCommand::RetryReferences {
                paper_index: paper_idx,
                refs_to_retry: vec![(ref_idx, reference, failed_dbs)],
                config: Box::new(config),
            });
        }
    }

    /// Handle R: retry all failed/not-found references for the current paper.
    fn handle_retry_all(&mut self) {
        let paper_idx = match &self.screen {
            Screen::Paper(idx) => *idx,
            Screen::RefDetail(idx, _) => *idx,
            Screen::Queue => {
                if self.queue_cursor < self.queue_sorted.len() {
                    self.queue_sorted[self.queue_cursor]
                } else {
                    return;
                }
            }
            _ => return,
        };

        let refs = match self.ref_states.get(paper_idx) {
            Some(r) => r,
            None => return,
        };
        let paper_refs = match self.paper_refs.get(paper_idx) {
            Some(r) => r,
            None => return,
        };

        // Collect retryable refs: NotFound with failed_dbs, or NotFound for full re-check
        let mut to_retry: Vec<(usize, hallucinator_core::Reference, Vec<String>)> = Vec::new();
        for (i, rs) in refs.iter().enumerate() {
            if let Some(result) = &rs.result
                && result.status == hallucinator_core::Status::NotFound
                && let Some(reference) = paper_refs.get(i)
            {
                to_retry.push((i, reference.clone(), result.failed_dbs.clone()));
            }
        }

        if to_retry.is_empty() {
            self.activity.log("No references to retry".to_string());
            return;
        }

        let count = to_retry.len();

        // Mark all as retrying
        if let Some(refs) = self.ref_states.get_mut(paper_idx) {
            for &(ref_idx, _, _) in &to_retry {
                if let Some(rs) = refs.get_mut(ref_idx) {
                    rs.phase = RefPhase::Retrying;
                }
            }
        }

        self.activity
            .log(format!("Retrying {} references...", count));

        if let Some(tx) = &self.backend_cmd_tx {
            let config = self.build_config();
            let _ = tx.send(BackendCommand::RetryReferences {
                paper_index: paper_idx,
                refs_to_retry: to_retry,
                config: Box::new(config),
            });
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
        let content_area = crate::view::banner::render_logo_bar(
            f,
            area,
            &self.theme,
            self.tick,
            self.config_state.fps,
        );

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
    if matches!(rs.phase, RefPhase::Skipped(_)) {
        return 5; // sort skipped refs last
    }
    match &rs.result {
        Some(r) => {
            if r.retraction_info.as_ref().is_some_and(|ri| ri.is_retracted) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::Action;
    use crate::model::config::ConfigSection;

    /// Create a minimal App for testing (no backend, no files).
    fn test_app() -> App {
        App::new(vec![], Theme::hacker())
    }

    /// Navigate from Banner to Queue (dismiss banner).
    fn dismiss_banner(app: &mut App) {
        app.screen = Screen::Queue;
    }

    // ── FilePickerContext defaults ──────────────────────────────────

    #[test]
    fn file_picker_context_defaults_to_add_files() {
        let app = test_app();
        assert_eq!(app.file_picker_context, FilePickerContext::AddFiles);
    }

    // ── AddFiles from Queue opens picker in AddFiles mode ──────────

    #[test]
    fn add_files_from_queue_opens_picker() {
        let mut app = test_app();
        dismiss_banner(&mut app);
        app.update(Action::AddFiles);
        assert_eq!(app.screen, Screen::FilePicker);
        assert_eq!(app.file_picker_context, FilePickerContext::AddFiles);
    }

    // ── AddFiles from Config > Databases item 0 opens db picker ────

    #[test]
    fn add_files_from_config_databases_item0_opens_db_picker() {
        let mut app = test_app();
        app.screen = Screen::Config;
        app.config_state.section = ConfigSection::Databases;
        app.config_state.item_cursor = 0;

        app.update(Action::AddFiles);

        assert_eq!(app.screen, Screen::FilePicker);
        assert_eq!(
            app.file_picker_context,
            FilePickerContext::SelectDatabase { config_item: 0 }
        );
    }

    #[test]
    fn add_files_from_config_databases_item1_opens_db_picker() {
        let mut app = test_app();
        app.screen = Screen::Config;
        app.config_state.section = ConfigSection::Databases;
        app.config_state.item_cursor = 1;

        app.update(Action::AddFiles);

        assert_eq!(app.screen, Screen::FilePicker);
        assert_eq!(
            app.file_picker_context,
            FilePickerContext::SelectDatabase { config_item: 1 }
        );
    }

    // ── AddFiles from Config > Databases item 2+ is a no-op ────────

    #[test]
    fn add_files_from_config_databases_toggle_item_is_noop() {
        let mut app = test_app();
        app.screen = Screen::Config;
        app.config_state.section = ConfigSection::Databases;
        app.config_state.item_cursor = 3; // a DB toggle item

        app.update(Action::AddFiles);

        // Should stay on Config, not open picker
        assert_eq!(app.screen, Screen::Config);
    }

    // ── AddFiles from Config > non-Databases section is a no-op ────

    #[test]
    fn add_files_from_config_api_keys_is_noop() {
        let mut app = test_app();
        app.screen = Screen::Config;
        app.config_state.section = ConfigSection::ApiKeys;
        app.config_state.item_cursor = 0;

        app.update(Action::AddFiles);

        assert_eq!(app.screen, Screen::Config);
    }

    // ── Esc in db picker with no selection returns to Config unchanged ──

    #[test]
    fn esc_in_db_picker_no_selection_returns_to_config() {
        let mut app = test_app();
        app.screen = Screen::FilePicker;
        app.file_picker_context = FilePickerContext::SelectDatabase { config_item: 0 };
        app.file_picker.selected.clear();
        app.config_state.dblp_offline_path = String::new();

        app.update(Action::NavigateBack);

        assert_eq!(app.screen, Screen::Config);
        assert_eq!(app.file_picker_context, FilePickerContext::AddFiles);
        assert!(app.config_state.dblp_offline_path.is_empty());
    }

    // ── Esc in db picker with selection writes canonicalized path ────

    #[test]
    fn esc_in_db_picker_with_selection_writes_path() {
        let mut app = test_app();
        app.screen = Screen::FilePicker;
        app.file_picker_context = FilePickerContext::SelectDatabase { config_item: 1 };

        // Use a path that definitely exists so canonicalize succeeds
        let existing = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        app.file_picker.selected = vec![existing.clone()];

        app.update(Action::NavigateBack);

        assert_eq!(app.screen, Screen::Config);
        assert_eq!(app.file_picker_context, FilePickerContext::AddFiles);
        // Should be an absolute, canonicalized path
        let result = &app.config_state.acl_offline_path;
        assert!(!result.is_empty());
        assert!(PathBuf::from(result).is_absolute());
    }

    // ── Esc in normal picker returns to Queue ───────────────────────

    #[test]
    fn esc_in_normal_picker_returns_to_queue() {
        let mut app = test_app();
        app.screen = Screen::FilePicker;
        app.file_picker_context = FilePickerContext::AddFiles;

        app.update(Action::NavigateBack);

        assert_eq!(app.screen, Screen::Queue);
    }

    // ── Space in db picker ignores non-db files ─────────────────────

    #[test]
    fn space_in_db_picker_ignores_non_db_entry() {
        let mut app = test_app();
        app.screen = Screen::FilePicker;
        app.file_picker_context = FilePickerContext::SelectDatabase { config_item: 0 };

        // Inject a PDF entry at cursor
        app.file_picker.entries = vec![FileEntry {
            name: "paper.pdf".to_string(),
            path: PathBuf::from("/tmp/paper.pdf"),
            is_dir: false,
            is_pdf: true,
            is_bbl: false,
            is_bib: false,
            is_archive: false,
            is_json: false,
            is_db: false,
        }];
        app.file_picker.cursor = 0;

        app.update(Action::ToggleSafe);

        assert!(app.file_picker.selected.is_empty());
    }

    // ── Space in db picker selects db file (single-select) ──────────

    #[test]
    fn space_in_db_picker_selects_db_file() {
        let mut app = test_app();
        app.screen = Screen::FilePicker;
        app.file_picker_context = FilePickerContext::SelectDatabase { config_item: 0 };

        app.file_picker.entries = vec![FileEntry {
            name: "dblp.db".to_string(),
            path: PathBuf::from("/tmp/dblp.db"),
            is_dir: false,
            is_pdf: false,
            is_bbl: false,
            is_bib: false,
            is_archive: false,
            is_json: false,
            is_db: true,
        }];
        app.file_picker.cursor = 0;

        app.update(Action::ToggleSafe);

        assert_eq!(app.file_picker.selected.len(), 1);
        assert_eq!(app.file_picker.selected[0], PathBuf::from("/tmp/dblp.db"));
    }

    // ── Space in db picker replaces previous selection ──────────────

    #[test]
    fn space_in_db_picker_single_select_replaces() {
        let mut app = test_app();
        app.screen = Screen::FilePicker;
        app.file_picker_context = FilePickerContext::SelectDatabase { config_item: 0 };

        app.file_picker.selected = vec![PathBuf::from("/tmp/old.db")];
        app.file_picker.entries = vec![FileEntry {
            name: "new.db".to_string(),
            path: PathBuf::from("/tmp/new.db"),
            is_dir: false,
            is_pdf: false,
            is_bbl: false,
            is_bib: false,
            is_archive: false,
            is_json: false,
            is_db: true,
        }];
        app.file_picker.cursor = 0;

        app.update(Action::ToggleSafe);

        assert_eq!(app.file_picker.selected.len(), 1);
        assert_eq!(app.file_picker.selected[0], PathBuf::from("/tmp/new.db"));
    }

    // ── Enter on .db file in db picker confirms and returns to Config ──

    #[test]
    fn enter_on_db_file_in_db_picker_confirms() {
        let mut app = test_app();
        app.screen = Screen::FilePicker;
        app.file_picker_context = FilePickerContext::SelectDatabase { config_item: 0 };

        // Use CARGO_MANIFEST_DIR as a known-existing path for canonicalize
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let cargo_toml = manifest.join("Cargo.toml");

        // Create a fake .db entry pointing to a real file (so canonicalize works)
        app.file_picker.entries = vec![FileEntry {
            name: "Cargo.toml".to_string(), // reuse existing file
            path: cargo_toml.clone(),
            is_dir: false,
            is_pdf: false,
            is_bbl: false,
            is_bib: false,
            is_archive: false,
            is_json: false,
            is_db: true, // pretend it's a db
        }];
        app.file_picker.cursor = 0;

        app.update(Action::DrillIn);

        assert_eq!(app.screen, Screen::Config);
        assert_eq!(app.file_picker_context, FilePickerContext::AddFiles);
        let result = &app.config_state.dblp_offline_path;
        assert!(!result.is_empty());
        assert!(PathBuf::from(result).is_absolute());
    }

    // ── Enter on directory in db picker navigates into it ───────────

    #[test]
    fn enter_on_dir_in_db_picker_navigates() {
        let mut app = test_app();
        app.screen = Screen::FilePicker;
        app.file_picker_context = FilePickerContext::SelectDatabase { config_item: 0 };

        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        app.file_picker.entries = vec![FileEntry {
            name: "src".to_string(),
            path: manifest.join("src"),
            is_dir: true,
            is_pdf: false,
            is_bbl: false,
            is_bib: false,
            is_archive: false,
            is_json: false,
            is_db: false,
        }];
        app.file_picker.cursor = 0;

        app.update(Action::DrillIn);

        // Should still be in file picker, navigated into the dir
        assert_eq!(app.screen, Screen::FilePicker);
        assert!(app.file_picker_context == FilePickerContext::SelectDatabase { config_item: 0 });
    }

    // ── Enter on non-db file in db picker is a no-op ────────────────

    #[test]
    fn enter_on_non_db_file_in_db_picker_is_noop() {
        let mut app = test_app();
        app.screen = Screen::FilePicker;
        app.file_picker_context = FilePickerContext::SelectDatabase { config_item: 0 };

        app.file_picker.entries = vec![FileEntry {
            name: "paper.pdf".to_string(),
            path: PathBuf::from("/tmp/paper.pdf"),
            is_dir: false,
            is_pdf: true,
            is_bbl: false,
            is_bib: false,
            is_archive: false,
            is_json: false,
            is_db: false,
        }];
        app.file_picker.cursor = 0;

        app.update(Action::DrillIn);

        // Should remain on file picker, nothing selected
        assert_eq!(app.screen, Screen::FilePicker);
        assert!(app.file_picker.selected.is_empty());
    }

    // ── Canonicalize on manual config edit ───────────────────────────

    #[test]
    fn confirm_config_edit_canonicalizes_dblp_path() {
        let mut app = test_app();
        app.screen = Screen::Config;
        app.config_state.section = ConfigSection::Databases;
        app.config_state.item_cursor = 0;

        // Start editing
        app.update(Action::DrillIn); // triggers handle_config_enter
        assert!(app.config_state.editing);

        // Clear buffer and type a known-existing path
        app.config_state.edit_buffer = env!("CARGO_MANIFEST_DIR").to_string();

        // Confirm
        app.update(Action::SearchConfirm);
        assert!(!app.config_state.editing);

        let result = &app.config_state.dblp_offline_path;
        assert!(!result.is_empty());
        assert!(PathBuf::from(result).is_absolute());
    }

    #[test]
    fn confirm_config_edit_empty_path_stays_empty() {
        let mut app = test_app();
        app.screen = Screen::Config;
        app.config_state.section = ConfigSection::Databases;
        app.config_state.item_cursor = 1;

        app.update(Action::DrillIn);
        app.config_state.edit_buffer.clear();
        app.update(Action::SearchConfirm);

        assert!(app.config_state.acl_offline_path.is_empty());
    }

    // ── is_db detection in FileEntry ────────────────────────────────

    #[test]
    fn refresh_entries_detects_db_extension() {
        // We can't easily control the filesystem, but we can test the
        // detection logic directly on a FileEntry constructed in refresh_entries.
        let ext_db = std::path::Path::new("test.db")
            .extension()
            .and_then(|e| e.to_str());
        assert!(
            ext_db
                .map(|e| e.eq_ignore_ascii_case("db") || e.eq_ignore_ascii_case("sqlite"))
                .unwrap_or(false)
        );

        let ext_sqlite = std::path::Path::new("test.sqlite")
            .extension()
            .and_then(|e| e.to_str());
        assert!(
            ext_sqlite
                .map(|e| e.eq_ignore_ascii_case("db") || e.eq_ignore_ascii_case("sqlite"))
                .unwrap_or(false)
        );

        let ext_pdf = std::path::Path::new("test.pdf")
            .extension()
            .and_then(|e| e.to_str());
        assert!(
            !ext_pdf
                .map(|e| e.eq_ignore_ascii_case("db") || e.eq_ignore_ascii_case("sqlite"))
                .unwrap_or(false)
        );
    }

    // ── toggle_selected allows .db files ────────────────────────────

    #[test]
    fn toggle_selected_allows_db_files() {
        let mut picker = FilePickerState::new();
        picker.entries = vec![FileEntry {
            name: "test.db".to_string(),
            path: PathBuf::from("/tmp/test.db"),
            is_dir: false,
            is_pdf: false,
            is_bbl: false,
            is_bib: false,
            is_archive: false,
            is_json: false,
            is_db: true,
        }];
        picker.cursor = 0;

        picker.toggle_selected();
        assert_eq!(picker.selected.len(), 1);

        // Toggle off
        picker.toggle_selected();
        assert!(picker.selected.is_empty());
    }

    // ── Normal picker behavior unchanged ────────────────────────────

    #[test]
    fn normal_picker_enter_toggles_pdf() {
        let mut app = test_app();
        app.screen = Screen::FilePicker;
        app.file_picker_context = FilePickerContext::AddFiles;

        app.file_picker.entries = vec![FileEntry {
            name: "paper.pdf".to_string(),
            path: PathBuf::from("/tmp/paper.pdf"),
            is_dir: false,
            is_pdf: true,
            is_bbl: false,
            is_bib: false,
            is_archive: false,
            is_json: false,
            is_db: false,
        }];
        app.file_picker.cursor = 0;

        app.update(Action::DrillIn);

        // In normal mode, Enter on PDF toggles selection (stays in picker)
        assert_eq!(app.screen, Screen::FilePicker);
        assert_eq!(app.file_picker.selected.len(), 1);
    }

    // ── Dirty flag tracking ─────────────────────────────────────────

    #[test]
    fn config_starts_not_dirty() {
        let app = test_app();
        assert!(!app.config_state.dirty);
    }

    #[test]
    fn confirm_config_edit_sets_dirty() {
        let mut app = test_app();
        app.screen = Screen::Config;
        app.config_state.section = ConfigSection::ApiKeys;
        app.config_state.item_cursor = 0;

        // Start editing, type something, confirm
        app.update(Action::DrillIn);
        app.config_state.edit_buffer = "test-key".to_string();
        app.update(Action::SearchConfirm);

        assert!(app.config_state.dirty);
    }

    #[test]
    fn config_space_toggle_db_sets_dirty() {
        let mut app = test_app();
        app.screen = Screen::Config;
        app.config_state.section = ConfigSection::Databases;
        app.config_state.item_cursor = 2; // first DB toggle

        app.update(Action::ToggleSafe);

        assert!(app.config_state.dirty);
    }

    #[test]
    fn config_theme_cycle_sets_dirty() {
        let mut app = test_app();
        app.screen = Screen::Config;
        app.config_state.section = ConfigSection::Display;
        app.config_state.item_cursor = 0;

        app.update(Action::DrillIn); // Enter cycles theme

        assert!(app.config_state.dirty);
    }

    // ── Confirm exit prompt ─────────────────────────────────────────

    #[test]
    fn esc_on_dirty_config_shows_confirm_prompt() {
        let mut app = test_app();
        app.screen = Screen::Config;
        app.config_state.dirty = true;

        app.update(Action::NavigateBack);

        // Should stay on Config with confirm_exit active
        assert_eq!(app.screen, Screen::Config);
        assert!(app.config_state.confirm_exit);
    }

    #[test]
    fn esc_on_clean_config_exits_immediately() {
        let mut app = test_app();
        app.screen = Screen::Config;
        app.config_state.dirty = false;

        app.update(Action::NavigateBack);

        assert_eq!(app.screen, Screen::Queue);
        assert!(!app.config_state.confirm_exit);
    }

    #[test]
    fn confirm_prompt_n_discards_and_exits() {
        let mut app = test_app();
        app.screen = Screen::Config;
        app.config_state.dirty = true;
        app.config_state.confirm_exit = true;

        // n = NextMatch in normal mode
        app.update(Action::NextMatch);

        assert_eq!(app.screen, Screen::Queue);
        assert!(!app.config_state.confirm_exit);
        assert!(!app.config_state.dirty);
    }

    #[test]
    fn confirm_prompt_esc_cancels_back_to_config() {
        let mut app = test_app();
        app.screen = Screen::Config;
        app.config_state.dirty = true;
        app.config_state.confirm_exit = true;

        app.update(Action::NavigateBack);

        // Should stay on Config, prompt dismissed
        assert_eq!(app.screen, Screen::Config);
        assert!(!app.config_state.confirm_exit);
        assert!(app.config_state.dirty); // still dirty
    }

    #[test]
    fn confirm_prompt_ignores_other_actions() {
        let mut app = test_app();
        app.screen = Screen::Config;
        app.config_state.dirty = true;
        app.config_state.confirm_exit = true;

        app.update(Action::MoveDown);

        // Should still be showing prompt, nothing changed
        assert_eq!(app.screen, Screen::Config);
        assert!(app.config_state.confirm_exit);
    }

    #[test]
    fn db_picker_enter_on_db_sets_dirty() {
        let mut app = test_app();
        app.screen = Screen::FilePicker;
        app.file_picker_context = FilePickerContext::SelectDatabase { config_item: 0 };

        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let cargo_toml = manifest.join("Cargo.toml");
        app.file_picker.entries = vec![FileEntry {
            name: "Cargo.toml".to_string(),
            path: cargo_toml,
            is_dir: false,
            is_pdf: false,
            is_bbl: false,
            is_bib: false,
            is_archive: false,
            is_json: false,
            is_db: true,
        }];
        app.file_picker.cursor = 0;

        app.update(Action::DrillIn);

        assert_eq!(app.screen, Screen::Config);
        assert!(app.config_state.dirty);
    }
}
