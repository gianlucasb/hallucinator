mod backend;
mod processing;
mod update;
mod update_config;
mod update_file_picker;
mod util;
use util::*;

use std::path::PathBuf;
use std::time::Instant;

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use tokio::sync::mpsc;

use hallucinator_ingest::archive::ArchiveItem;

use crate::model::activity::ActivityState;
use crate::model::config::ConfigState;
use crate::model::paper::{PaperFilter, PaperSortOrder, RefState};
use crate::model::queue::{PaperState, QueueFilter, SortOrder, filtered_indices};
use crate::theme::Theme;
use crate::tui_event::BackendCommand;
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
                    let is_archive = hallucinator_ingest::is_archive_path(&path);
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
    pub queue_cursor: usize,
    pub paper_cursor: usize,
    pub sort_order: SortOrder,
    pub sort_reversed: bool,
    /// Maps visual row index → paper index (recomputed on sort/tick).
    pub queue_sorted: Vec<usize>,
    pub tick: usize,
    pub theme: Theme,
    pub should_quit: bool,
    pub confirm_quit: bool,
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

    /// Wall-clock instant when the banner was first shown.
    pub banner_start: Option<Instant>,
    /// Whether to emit terminal bell (set on batch complete, consumed on next view).
    pub pending_bell: bool,

    /// Current pro-tip index (rotated by a timer in the main loop).
    pub tip_index: usize,
    /// Tick when the tip last changed (for typewriter reset).
    pub tip_change_tick: usize,

    /// Persistent T-800 seeking crosshair state (only when T-800 theme active).
    pub t800_splash: Option<crate::view::banner::T800Splash>,

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
    pub(super) throughput_since_last: u16,
    /// Tick count of last throughput push.
    pub(super) last_throughput_tick: usize,
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
    pub(super) archive_rx: Option<std::sync::mpsc::Receiver<ArchiveItem>>,
    /// Name of the archive being streamed (for display name prefix).
    pub(super) archive_streaming_name: Option<String>,
    /// Number of PDFs extracted so far from the current archive.
    pub extracted_count: usize,
    /// Number of `run_batch_with_offset` tasks still running.
    pub(super) inflight_batches: usize,
    /// Rate limiters for the current run (shared with backend for backoff state).
    pub current_rate_limiters: Option<std::sync::Arc<hallucinator_core::RateLimiters>>,
    /// Query cache for the current run (shared with backend for cache stats).
    pub current_query_cache: Option<std::sync::Arc<hallucinator_core::QueryCache>>,
    /// Cache path corresponding to the current_query_cache (for change detection).
    pub(super) current_query_cache_path: Option<std::path::PathBuf>,
    /// Frame counter for FPS measurement.
    pub(super) frame_count: u32,
    /// Last time FPS was sampled.
    pub(super) last_fps_instant: Instant,
    /// Measured FPS for display.
    pub measured_fps: f32,
    /// Measured process RSS in bytes (updated once per second).
    pub measured_rss_bytes: usize,
}

impl App {
    pub fn new(filenames: Vec<String>, theme: Theme) -> Self {
        let papers: Vec<PaperState> = filenames.into_iter().map(PaperState::new).collect();
        let paper_count = papers.len();
        let ref_states = vec![Vec::new(); paper_count];
        let queue_sorted: Vec<usize> = (0..papers.len()).collect();

        Self {
            screen: Screen::Banner,
            papers,
            ref_states,
            queue_cursor: 0,
            paper_cursor: 0,
            sort_order: SortOrder::ProblematicPct,
            sort_reversed: false,
            queue_sorted,
            tick: 0,
            theme,
            should_quit: false,
            confirm_quit: false,
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
            banner_start: None, // set in main.rs after config is applied
            pending_bell: false,
            tip_index: 0,
            tip_change_tick: 0,
            t800_splash: None,
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
            inflight_batches: 0,
            current_rate_limiters: None,
            current_query_cache: None,
            current_query_cache_path: None,
            frame_count: 0,
            last_fps_instant: Instant::now(),
            measured_fps: 0.0,
            measured_rss_bytes: get_rss_bytes().unwrap_or(0),
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
            SortOrder::NotFound => {
                indices.sort_by(|&a, &b| {
                    self.papers[b]
                        .stats
                        .not_found
                        .cmp(&self.papers[a].stats.not_found)
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
            SortOrder::Status => {
                indices.sort_by(|&a, &b| {
                    self.papers[a]
                        .phase
                        .sort_key()
                        .cmp(&self.papers[b].phase.sort_key())
                        .then_with(|| a.cmp(&b))
                });
            }
        }
        if self.sort_reversed {
            indices.reverse();
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
            crate::view::export::ExportScope::AllPapers
            | crate::view::export::ExportScope::ProblematicPapers => {
                "hallucinator-results".to_string()
            }
        }
    }

    // update() is in update.rs

    // start_processing(), build_config(), get_or_build_query_cache(),
    // add_files_from_picker(), start_next_archive_extraction(),
    // drain_archive_channel(), get_copyable_text(), handle_retry_single(),
    // handle_retry_all() are in processing.rs

    // handle_click(), config_section_item_count(), handle_config_enter(),
    // handle_config_space(), confirm_config_edit(), clear_query_cache(),
    // clear_not_found_cache(), save_config(), handle_build_database()
    // are in update_config.rs

    // handle_backend_event() and handle_progress() are in backend.rs

    /// Record that a frame was drawn; updates the measured FPS counter.
    pub fn record_frame(&mut self) {
        self.frame_count += 1;
        let elapsed = self.last_fps_instant.elapsed();
        if elapsed.as_secs_f32() >= 1.0 {
            self.measured_fps = self.frame_count as f32 / elapsed.as_secs_f32();
            self.frame_count = 0;
            self.last_fps_instant = Instant::now();
            self.measured_rss_bytes = get_rss_bytes().unwrap_or(self.measured_rss_bytes);
        }
    }

    pub fn elapsed(&self) -> std::time::Duration {
        if let Some(frozen) = self.frozen_elapsed {
            frozen
        } else if let Some(start) = self.start_time {
            start.elapsed()
        } else {
            std::time::Duration::ZERO
        }
    }

    /// Build the global stats line shown in the logo bar.
    fn build_stats_line(&self) -> Line<'static> {
        let theme = &self.theme;
        let total = self.papers.len();
        let done = self.papers.iter().filter(|p| p.phase.is_terminal()).count();
        let total_refs: usize = self.papers.iter().map(|p| p.stats.total).sum();
        let total_verified: usize = self.papers.iter().map(|p| p.stats.verified).sum();
        let total_not_found: usize = self.papers.iter().map(|p| p.stats.not_found).sum();
        let total_mismatch: usize = self.papers.iter().map(|p| p.stats.mismatch).sum();

        let spans = vec![
            Span::styled(
                format!("{}/{} papers ", done, total),
                Style::default().fg(theme.text),
            ),
            Span::styled(
                format!("Refs:{} ", total_refs),
                Style::default().fg(theme.dim),
            ),
            Span::styled(
                format!("V:{} ", total_verified),
                Style::default().fg(theme.verified),
            ),
            Span::styled(
                format!("M:{} ", total_mismatch),
                Style::default().fg(theme.mismatch),
            ),
            Span::styled(
                format!("NF:{} ", total_not_found),
                Style::default().fg(theme.not_found),
            ),
        ];

        Line::from(spans)
    }

    /// Build a right-aligned FPS/RSS line for the footer bar.
    fn build_footer_right(&self) -> Line<'static> {
        Line::from(vec![
            Span::styled(
                format!(
                    "RSS:{:.0}MB ",
                    self.measured_rss_bytes as f64 / (1024.0 * 1024.0)
                ),
                Style::default().fg(self.theme.dim),
            ),
            Span::styled(
                format!("FPS:{:.2}", self.measured_fps),
                Style::default().fg(self.theme.dim),
            ),
        ])
        .alignment(Alignment::Right)
    }

    /// Cycle theme: hacker → modern → gnr → hacker.
    fn cycle_theme(&mut self) {
        let (name, theme) = match self.config_state.theme_name.as_str() {
            "hacker" => ("modern", Theme::modern()),
            "modern" => ("gnr", Theme::t800()),
            _ => ("hacker", Theme::hacker()),
        };
        self.config_state.theme_name = name.to_string();
        self.theme = theme;
        self.config_state.dirty = true;
    }

    /// Dismiss the banner and navigate to the appropriate first screen.
    fn dismiss_banner(&mut self) {
        self.banner_start = None;
        if self.single_paper_mode {
            self.screen = Screen::Paper(0);
        } else if self.papers.is_empty() {
            self.screen = Screen::FilePicker;
        } else {
            self.screen = Screen::Queue;
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
            let elapsed = self
                .banner_start
                .map(|s| s.elapsed())
                .unwrap_or(std::time::Duration::ZERO);
            crate::view::banner::render(
                f,
                &self.theme,
                self.tick,
                elapsed,
                self.t800_splash.as_mut(),
            );
            return;
        }

        // File picker renders without activity panel
        if self.screen == Screen::FilePicker {
            crate::view::file_picker::render_in(f, self, area);
            if self.confirm_quit {
                crate::view::quit_confirm::render(f, &self.theme);
            }
            return;
        }

        // Build global stats line for the logo bar
        let stats_line = self.build_stats_line();

        // Persistent logo bar at top of every content screen
        let content_area = crate::view::banner::render_logo_bar(
            f,
            area,
            &self.theme,
            self.tip_index,
            self.tick,
            self.tip_change_tick,
            Some(stats_line),
        );

        // Split footer row out first so it spans the full terminal width.
        // Use direct Rect arithmetic instead of Layout to avoid constraint solver overhead.
        let footer_area = Rect {
            x: content_area.x,
            y: content_area.y + content_area.height.saturating_sub(1),
            width: content_area.width,
            height: 1.min(content_area.height),
        };
        let body_area = Rect {
            height: content_area.height.saturating_sub(1),
            ..content_area
        };

        // Activity panel split (on body_area, so it doesn't cover the footer)
        let main_area = if self.activity_panel_visible {
            let panel_width = if body_area.width > 120 {
                45
            } else {
                (body_area.width / 3).max(30)
            };
            let chunks = Layout::horizontal([Constraint::Min(40), Constraint::Length(panel_width)])
                .split(body_area);

            crate::view::activity::render(f, chunks[1], self);
            chunks[0]
        } else {
            body_area
        };

        // Clone screen to avoid borrow conflict with &mut self
        let screen = self.screen.clone();
        match screen {
            Screen::Queue => crate::view::queue::render_in(f, self, main_area, footer_area),
            Screen::Paper(idx) => {
                crate::view::paper::render_in(f, self, idx, main_area, footer_area)
            }
            Screen::RefDetail(paper_idx, ref_idx) => {
                crate::view::detail::render_in(f, self, paper_idx, ref_idx, main_area, footer_area)
            }
            Screen::Config => crate::view::config::render_in(f, self, main_area, footer_area),
            Screen::Banner | Screen::FilePicker => unreachable!(),
        }

        // Render FPS/RSS right-aligned in the footer (painter's order: overwrites right side)
        let footer_right = self.build_footer_right();
        f.render_widget(Paragraph::new(vec![footer_right]), footer_area);

        if self.config_state.confirm_exit {
            crate::view::config_confirm::render(f, &self.theme);
        }

        if self.export_state.active {
            crate::view::export::render(f, self);
        }

        if self.show_help {
            crate::view::help::render(f, &self.theme);
        }

        if self.confirm_quit {
            crate::view::quit_confirm::render(f, &self.theme);
        }
    }
}

#[cfg(test)]
mod tests;
