use std::path::PathBuf;
use std::time::Instant;

use hallucinator_ingest::archive::ArchiveItem;

use super::App;
use crate::model::paper::RefPhase;
use crate::model::queue::{PaperPhase, PaperState};
use crate::tui_event::BackendCommand;

impl App {
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
        self.inflight_batches = 0;
        self.start_time = Some(Instant::now());
        self.frozen_elapsed = None;
        self.activity = crate::model::activity::ActivityState::default();
        // Pre-seed "Web Search" in activity panel if SearxNG is configured
        if self.config_state.searxng_url.is_some() {
            self.activity.db_health.insert(
                "Web Search".to_string(),
                crate::model::activity::DbHealth::new(),
            );
        }
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

        let config = self.build_config();
        // Keep references to rate limiters and cache for the activity panel
        self.current_rate_limiters = Some(config.rate_limiters.clone());
        if let Some(tx) = &self.backend_cmd_tx {
            let _ = tx.send(BackendCommand::ProcessFiles {
                files: real_files,
                starting_index: 0,
                config: Box::new(config),
            });
            self.inflight_batches += 1;
        }
    }

    /// Return the existing query cache if the path hasn't changed, or build a new one.
    pub(crate) fn get_or_build_query_cache(
        &mut self,
    ) -> std::sync::Arc<hallucinator_core::QueryCache> {
        let current_path = if self.config_state.cache_path.is_empty() {
            None
        } else {
            Some(std::path::PathBuf::from(&self.config_state.cache_path))
        };

        // Reuse existing cache if path matches and we have a live handle
        if let Some(ref existing) = self.current_query_cache {
            let prev_path = self
                .current_query_cache_path
                .as_ref()
                .map(std::path::PathBuf::from);
            if prev_path == current_path {
                return existing.clone();
            }
        }

        // Path changed or no cache yet — build fresh
        let cache = hallucinator_core::build_query_cache(
            current_path.as_deref(),
            hallucinator_core::DEFAULT_POSITIVE_TTL.as_secs(),
            hallucinator_core::DEFAULT_NEGATIVE_TTL.as_secs(),
        );

        // Log cache info
        if cache.has_persistence() {
            let (found, nf) = cache.l2_counts();
            let total = found + nf;
            if let Some(ref p) = current_path {
                self.activity.log(format!(
                    "Cache opened: {} ({} entries: {} found, {} not-found)",
                    p.display(),
                    total,
                    found,
                    nf,
                ));
            }
        } else if current_path.is_some() {
            self.activity
                .log_warn("Cache: failed to open SQLite, using in-memory only".to_string());
        }

        self.current_query_cache = Some(cache.clone());
        self.current_query_cache_path = current_path;
        cache
    }

    /// Build a `hallucinator_core::Config` from the current ConfigState.
    pub(super) fn build_config(&mut self) -> hallucinator_core::Config {
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
            openalex_offline_path: if self.config_state.openalex_offline_path.is_empty() {
                None
            } else {
                Some(std::path::PathBuf::from(
                    &self.config_state.openalex_offline_path,
                ))
            },
            openalex_offline_db: None, // Populated from main.rs
            num_workers: self.config_state.num_workers,
            max_rate_limit_retries: self.config_state.max_rate_limit_retries,
            rate_limiters: std::sync::Arc::new(hallucinator_core::RateLimiters::new(
                !self.config_state.crossref_mailto.is_empty(),
                !self.config_state.s2_api_key.is_empty(),
            )),
            db_timeout_secs: self.config_state.db_timeout_secs,
            db_timeout_short_secs: self.config_state.db_timeout_short_secs,
            disabled_dbs,
            check_openalex_authors: false,
            crossref_mailto: if self.config_state.crossref_mailto.is_empty() {
                None
            } else {
                Some(self.config_state.crossref_mailto.clone())
            },
            searxng_url: self.config_state.searxng_url.clone(),
            cache_path: if self.config_state.cache_path.is_empty() {
                None
            } else {
                Some(std::path::PathBuf::from(&self.config_state.cache_path))
            },
            cache_positive_ttl_secs: hallucinator_core::DEFAULT_POSITIVE_TTL.as_secs(),
            cache_negative_ttl_secs: hallucinator_core::DEFAULT_NEGATIVE_TTL.as_secs(),
            query_cache: Some(self.get_or_build_query_cache()),
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
                        for (paper, refs) in loaded {
                            self.papers.push(paper);
                            self.ref_states.push(refs);
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
            } else if hallucinator_ingest::is_archive_path(&path) {
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
                self.file_paths.push(path);
            }
        }
        self.recompute_sorted_indices();
    }

    /// Start streaming extraction for the next pending archive.
    /// Spawns a background thread that extracts PDFs one-by-one,
    /// sending them through a channel that the tick handler drains.
    pub(super) fn start_next_archive_extraction(&mut self) {
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
                hallucinator_ingest::extract_archive_streaming(&path, &dir, max_size, &tx)
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
    pub(super) fn drain_archive_channel(&mut self) -> bool {
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
        if self.processing_started && got_new {
            let starting_index = self.file_paths.len() - new_pdfs.len();
            let config = self.build_config();
            if let Some(tx) = &self.backend_cmd_tx {
                let _ = tx.send(BackendCommand::ProcessFiles {
                    files: new_pdfs,
                    starting_index,
                    config: Box::new(config),
                });
                self.inflight_batches += 1;
            }
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
                // All archives extracted. If all sub-batches already completed,
                // mark the entire run as done now.
                if self.inflight_batches == 0 && self.processing_started && !self.batch_complete {
                    self.frozen_elapsed = Some(self.elapsed());
                    self.batch_complete = true;
                    self.pending_bell = true;
                }
            }
        }

        finished
    }

    /// Get text to copy for the current screen context.
    pub(super) fn get_copyable_text(&self) -> Option<String> {
        match &self.screen {
            super::Screen::RefDetail(paper_idx, ref_idx) => {
                let rs = self.ref_states.get(*paper_idx)?.get(*ref_idx)?;
                if let Some(result) = &rs.result
                    && !result.raw_citation.is_empty()
                {
                    return Some(result.raw_citation.clone());
                }
                Some(rs.title.clone())
            }
            super::Screen::Paper(idx) => {
                let indices = self.paper_ref_indices(*idx);
                let ref_idx = indices.get(self.paper_cursor)?;
                let rs = self.ref_states.get(*idx)?.get(*ref_idx)?;
                Some(rs.title.clone())
            }
            _ => None,
        }
    }

    /// Handle Ctrl+r: retry the currently selected reference.
    pub(super) fn handle_retry_single(&mut self) {
        let (paper_idx, ref_idx) = match &self.screen {
            super::Screen::Paper(idx) => {
                let idx = *idx;
                let indices = self.paper_ref_indices(idx);
                if self.paper_cursor >= indices.len() {
                    return;
                }
                (idx, indices[self.paper_cursor])
            }
            super::Screen::RefDetail(paper_idx, ref_idx) => (*paper_idx, *ref_idx),
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

        let reference = match self.ref_states.get(paper_idx).and_then(|r| r.get(ref_idx)) {
            Some(rs) => rs.to_reference(),
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

        let config = self.build_config();
        if let Some(tx) = &self.backend_cmd_tx {
            let _ = tx.send(BackendCommand::RetryReferences {
                paper_index: paper_idx,
                refs_to_retry: vec![(ref_idx, reference, failed_dbs)],
                config: Box::new(config),
            });
        }
    }

    /// Handle R: retry all failed/not-found references for the current paper.
    pub(super) fn handle_retry_all(&mut self) {
        let paper_idx = match &self.screen {
            super::Screen::Paper(idx) => *idx,
            super::Screen::RefDetail(idx, _) => *idx,
            super::Screen::Queue => {
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

        // Collect retryable refs: NotFound with failed_dbs, or NotFound for full re-check
        let mut to_retry: Vec<(usize, hallucinator_core::Reference, Vec<String>)> = Vec::new();
        for (i, rs) in refs.iter().enumerate() {
            if let Some(result) = &rs.result
                && result.status == hallucinator_core::Status::NotFound
            {
                to_retry.push((i, rs.to_reference(), result.failed_dbs.clone()));
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

        let config = self.build_config();
        if let Some(tx) = &self.backend_cmd_tx {
            let _ = tx.send(BackendCommand::RetryReferences {
                paper_index: paper_idx,
                refs_to_retry: to_retry,
                config: Box::new(config),
            });
        }
    }
}
