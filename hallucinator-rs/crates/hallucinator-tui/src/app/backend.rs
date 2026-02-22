use std::time::Instant;

use hallucinator_core::{DbStatus, ProgressEvent};

use hallucinator_reporting::FpReason;

use super::App;
use crate::model::activity::ActiveQuery;
use crate::model::paper::{RefPhase, RefState};
use crate::model::queue::PaperPhase;
use crate::tui_event::BackendEvent;

impl App {
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
                references,
                skip_stats: _,
            } => {
                if let Some(paper) = self.papers.get_mut(paper_index) {
                    paper.total_refs = ref_count;
                    let skipped = references
                        .iter()
                        .filter(|r| r.skip_reason.is_some())
                        .count();
                    paper.stats.total = references.len();
                    paper.stats.skipped = skipped;
                    // Allocate result slots for ALL refs (including skipped) so
                    // that remapped indices from the backend fit.
                    paper.init_results(references.len());
                    paper.phase = PaperPhase::Checking;
                }
                if paper_index < self.ref_states.len() {
                    self.ref_states[paper_index] = references
                        .into_iter()
                        .map(|r| {
                            let phase = if let Some(reason) = &r.skip_reason {
                                RefPhase::Skipped(reason.clone())
                            } else {
                                RefPhase::Pending
                            };
                            RefState {
                                index: r.original_number.saturating_sub(1),
                                title: r.title.clone().unwrap_or_default(),
                                phase,
                                result: None,
                                fp_reason: None,
                                raw_citation: r.raw_citation,
                                authors: r.authors,
                                doi: r.doi,
                                arxiv_id: r.arxiv_id,
                            }
                        })
                        .collect();

                    // Restore persisted FP overrides from cache
                    if let Some(cache) = &self.current_query_cache {
                        for rs in &mut self.ref_states[paper_index] {
                            if let Some(reason_str) = cache.get_fp_override(&rs.title) {
                                rs.fp_reason = reason_str.parse::<FpReason>().ok();
                            }
                        }
                    }
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
            BackendEvent::PaperComplete { paper_index } => {
                if let Some(paper) = self.papers.get_mut(paper_index)
                    && paper.phase != PaperPhase::ExtractionFailed
                {
                    paper.phase = PaperPhase::Complete;
                }
            }
            BackendEvent::BatchComplete => {
                self.inflight_batches = self.inflight_batches.saturating_sub(1);
                // Only mark the whole run as complete (and ring the bell) when
                // all sub-batches have finished AND no archives are still extracting.
                let all_done = self.inflight_batches == 0
                    && self.pending_archive_extractions.is_empty()
                    && self.archive_rx.is_none();
                if all_done {
                    self.frozen_elapsed = Some(self.elapsed());
                    self.batch_complete = true;
                    self.pending_bell = true;
                }
            }
            BackendEvent::DblpBuildProgress { event } => {
                // Track parse phase start for records/s calculation
                if matches!(event, hallucinator_dblp::BuildProgress::Parsing { .. })
                    && self.config_state.dblp_parse_started.is_none()
                {
                    self.config_state.dblp_parse_started = Some(Instant::now());
                }
                self.config_state.dblp_build_status = Some(super::format_dblp_progress(
                    &event,
                    self.config_state.dblp_build_started,
                    self.config_state.dblp_parse_started,
                ));
            }
            BackendEvent::DblpBuildComplete {
                success,
                error,
                db_path,
            } => {
                self.config_state.dblp_building = false;
                if success {
                    let elapsed = self
                        .config_state
                        .dblp_build_started
                        .map(|s| s.elapsed())
                        .unwrap_or_default();
                    self.config_state.dblp_build_status =
                        Some(format!("Build complete! (total {:.0?})", elapsed));
                    self.config_state.dblp_offline_path = db_path.display().to_string();
                    self.activity
                        .log(format!("DBLP database built: {}", db_path.display()));
                } else {
                    let msg = error.unwrap_or_else(|| "unknown error".to_string());
                    self.config_state.dblp_build_status = Some(format!("Failed: {}", msg));
                    self.activity
                        .log_warn(format!("DBLP build failed: {}", msg));
                }
            }
            BackendEvent::AclBuildProgress { event } => {
                // Track parse phase start for records/s calculation
                if matches!(event, hallucinator_acl::BuildProgress::Parsing { .. })
                    && self.config_state.acl_parse_started.is_none()
                {
                    self.config_state.acl_parse_started = Some(Instant::now());
                }
                self.config_state.acl_build_status = Some(super::format_acl_progress(
                    &event,
                    self.config_state.acl_build_started,
                    self.config_state.acl_parse_started,
                ));
            }
            BackendEvent::AclBuildComplete {
                success,
                error,
                db_path,
            } => {
                self.config_state.acl_building = false;
                if success {
                    let elapsed = self
                        .config_state
                        .acl_build_started
                        .map(|s| s.elapsed())
                        .unwrap_or_default();
                    self.config_state.acl_build_status =
                        Some(format!("Build complete! (total {:.0?})", elapsed));
                    self.config_state.acl_offline_path = db_path.display().to_string();
                    self.activity
                        .log(format!("ACL database built: {}", db_path.display()));
                } else {
                    let msg = error.unwrap_or_else(|| "unknown error".to_string());
                    self.config_state.acl_build_status = Some(format!("Failed: {}", msg));
                    self.activity.log_warn(format!("ACL build failed: {}", msg));
                }
            }
            BackendEvent::OpenAlexBuildProgress { event } => {
                if let Some(s) = super::format_openalex_progress(
                    &event,
                    self.config_state.openalex_build_started,
                ) {
                    self.config_state.openalex_build_status = Some(s);
                }
            }
            BackendEvent::OpenAlexBuildComplete {
                success,
                error,
                db_path,
            } => {
                self.config_state.openalex_building = false;
                if success {
                    let elapsed = self
                        .config_state
                        .openalex_build_started
                        .map(|s| s.elapsed())
                        .unwrap_or_default();
                    self.config_state.openalex_build_status =
                        Some(format!("Build complete! (total {:.0?})", elapsed));
                    self.config_state.openalex_offline_path = db_path.display().to_string();
                    self.activity
                        .log(format!("OpenAlex index built: {}", db_path.display()));
                } else {
                    let msg = error.unwrap_or_else(|| "unknown error".to_string());
                    self.config_state.openalex_build_status = Some(format!("Failed: {}", msg));
                    self.activity
                        .log_warn(format!("OpenAlex build failed: {}", msg));
                }
            }
        }
    }

    pub(super) fn handle_progress(&mut self, paper_index: usize, event: ProgressEvent) {
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
                    is_retry: false,
                });
                // Increment in-flight for all enabled DBs
                let enabled: Vec<String> = self
                    .config_state
                    .disabled_dbs
                    .iter()
                    .filter(|(_, enabled)| *enabled)
                    .map(|(name, _)| name.clone())
                    .collect();
                self.activity.increment_in_flight(&enabled);
            }
            ProgressEvent::Result { index, result, .. } => {
                let result = *result;
                if let Some(paper) = self.papers.get_mut(paper_index) {
                    // Track retry progress
                    if paper.phase == PaperPhase::Retrying {
                        paper.retry_done += 1;
                    }
                    let is_retracted = result
                        .retraction_info
                        .as_ref()
                        .is_some_and(|r| r.is_retracted);
                    paper.record_status(index, result.status.clone(), is_retracted);
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
            ProgressEvent::Warning { .. } => {
                // Per-reference warnings are too spammy for the activity log.
                // Aggregate DB-level warnings are emitted below in DatabaseQueryComplete.
            }
            ProgressEvent::Retrying {
                title, failed_dbs, ..
            } => {
                // Mark the existing active query as a retry
                if let Some(q) = self
                    .activity
                    .active_queries
                    .iter_mut()
                    .find(|q| q.ref_title == title)
                {
                    q.is_retry = true;
                    q.db_name = format!("retry ({})", failed_dbs.len());
                }
            }
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
                if status == DbStatus::Skipped {
                    // Early-exit artifact — just decrement in-flight
                    self.activity.decrement_in_flight(&db_name);
                } else {
                    let success = matches!(
                        status,
                        DbStatus::Match | DbStatus::NoMatch | DbStatus::AuthorMismatch
                    );
                    let is_rate_limited = status == DbStatus::RateLimited;
                    let is_match = status == DbStatus::Match;
                    self.activity.record_db_complete(
                        &db_name,
                        success,
                        is_rate_limited,
                        is_match,
                        elapsed.as_secs_f64() * 1000.0,
                    );

                    // Emit a one-time warning when a DB hits 3+ failures
                    if !success
                        && !self.activity.warned_dbs.contains(&db_name)
                        && let Some(health) = self.activity.db_health.get(db_name.as_str())
                        && health.failed >= 3
                    {
                        let has_dblp_offline = !self.config_state.dblp_offline_path.is_empty();
                        let msg = super::db_failure_warning(
                            &db_name,
                            health.rate_limited,
                            has_dblp_offline,
                        );
                        self.activity.log_warn(msg);
                        self.activity.warned_dbs.insert(db_name.clone());
                    }
                }
            }
            ProgressEvent::RateLimitWait { .. } | ProgressEvent::RateLimitRetry { .. } => {
                // Rate limit events are handled internally by the pool;
                // no TUI action needed (activity panel could log these in the future).
            }
        }
    }
}
