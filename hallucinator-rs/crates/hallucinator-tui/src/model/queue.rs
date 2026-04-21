use hallucinator_core::{CheckStats, MismatchKind, Status};

pub use hallucinator_reporting::PaperVerdict;

/// Lightweight summary of a validation result, stored in PaperState.
/// The full ValidationResult is kept only in RefState.result.
#[derive(Debug, Clone)]
pub struct ResultSummary {
    pub status: Status,
    pub is_retracted: bool,
}

/// Processing phase of a paper in the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaperPhase {
    Queued,
    Extracting,
    ExtractionFailed,
    Checking,
    Retrying,
    Complete,
}

impl PaperPhase {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Extracting => "Extracting...",
            Self::ExtractionFailed => "Failed",
            Self::Checking => "Checking...",
            Self::Retrying => "Retrying...",
            Self::Complete => "Done",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Complete | Self::ExtractionFailed)
    }
}

/// State of a single paper in the queue.
#[derive(Debug, Clone)]
pub struct PaperState {
    pub filename: String,
    pub phase: PaperPhase,
    pub total_refs: usize,
    pub stats: CheckStats,
    /// Indexed by reference position; `None` = not yet completed.
    pub results: Vec<Option<ResultSummary>>,
    pub error: Option<String>,
    /// Total refs to retry in the retry pass.
    pub retry_total: usize,
    /// Completed retry count.
    pub retry_done: usize,
    /// User-assigned verdict for the entire paper.
    pub verdict: Option<PaperVerdict>,
}

impl PaperState {
    pub fn new(filename: String) -> Self {
        Self {
            filename,
            phase: PaperPhase::Queued,
            total_refs: 0,
            stats: CheckStats::default(),
            results: Vec::new(),
            error: None,
            retry_total: 0,
            retry_done: 0,
            verdict: None,
        }
    }

    /// Pre-allocate result slots once the reference count is known.
    pub fn init_results(&mut self, count: usize) {
        self.results = vec![None; count];
    }

    /// Record (or replace) a validation result summary at the given index.
    ///
    /// If the slot already contains a result (retry pass), the old status
    /// counters are decremented before the new ones are incremented, preventing
    /// double-counting.
    pub fn record_status(&mut self, index: usize, status: Status, is_retracted: bool) {
        // Grow if needed (shouldn't happen after init_results, but be safe)
        if index >= self.results.len() {
            self.results.resize(index + 1, None);
        }

        // Decrement old counters if replacing
        if let Some(old) = &self.results[index] {
            match &old.status {
                Status::Verified => self.stats.verified = self.stats.verified.saturating_sub(1),
                Status::NotFound => self.stats.not_found = self.stats.not_found.saturating_sub(1),
                Status::Mismatch(kind) => {
                    self.stats.mismatch = self.stats.mismatch.saturating_sub(1);
                    if kind.contains(MismatchKind::AUTHOR) {
                        self.stats.author_mismatch = self.stats.author_mismatch.saturating_sub(1);
                    }
                    if kind.contains(MismatchKind::DOI) {
                        self.stats.doi_mismatch = self.stats.doi_mismatch.saturating_sub(1);
                    }
                    if kind.contains(MismatchKind::ARXIV_ID) {
                        self.stats.arxiv_mismatch = self.stats.arxiv_mismatch.saturating_sub(1);
                    }
                }
            }
            if old.is_retracted {
                self.stats.retracted = self.stats.retracted.saturating_sub(1);
            }
        }

        // Increment new counters
        match &status {
            Status::Verified => self.stats.verified += 1,
            Status::NotFound => self.stats.not_found += 1,
            Status::Mismatch(kind) => {
                self.stats.mismatch += 1;
                if kind.contains(MismatchKind::AUTHOR) {
                    self.stats.author_mismatch += 1;
                }
                if kind.contains(MismatchKind::DOI) {
                    self.stats.doi_mismatch += 1;
                }
                if kind.contains(MismatchKind::ARXIV_ID) {
                    self.stats.arxiv_mismatch += 1;
                }
            }
        }
        if is_retracted {
            self.stats.retracted += 1;
        }

        self.results[index] = Some(ResultSummary {
            status,
            is_retracted,
        });
    }

    /// Adjust the status-bucket counters for a false-positive override
    /// on a single reference.
    ///
    /// `dir = +1` means "user just marked this ref safe" — move it out
    /// of its current status bucket (not_found / mismatch / retracted)
    /// into `verified`.  `dir = -1` undoes that (user un-marked it).
    ///
    /// Mirrors the bucket structure of `record_status`: Status::Verified
    /// is unchanged (already counted in `verified`); Status::Mismatch
    /// decrements the overall `mismatch` counter plus each matching
    /// sub-flag (`author_mismatch`, `doi_mismatch`, `arxiv_mismatch`);
    /// `is_retracted` is an independent counter and is always toggled.
    ///
    /// Called from three places:
    ///   * `app/update.rs` when the user cycles the fp reason with Space,
    ///   * `app/backend.rs` when a ProgressEvent::Result arrives for a
    ///     ref whose fp override was already restored from the query
    ///     cache during extraction,
    ///   * `load.rs` after loading a JSON export whose refs carry
    ///     persisted fp_reason fields.
    pub fn apply_fp_delta(&mut self, status: &Status, is_retracted: bool, dir: i32) {
        debug_assert!(dir == 1 || dir == -1, "dir must be +1 or -1");
        let add = |n: &mut usize, delta: i32| {
            if delta >= 0 {
                *n = n.saturating_add(delta as usize);
            } else {
                *n = n.saturating_sub(delta.unsigned_abs() as usize);
            }
        };
        match status {
            Status::Verified => {}
            Status::NotFound => {
                add(&mut self.stats.not_found, -dir);
                add(&mut self.stats.verified, dir);
            }
            Status::Mismatch(kind) => {
                add(&mut self.stats.mismatch, -dir);
                if kind.contains(MismatchKind::AUTHOR) {
                    add(&mut self.stats.author_mismatch, -dir);
                }
                if kind.contains(MismatchKind::DOI) {
                    add(&mut self.stats.doi_mismatch, -dir);
                }
                if kind.contains(MismatchKind::ARXIV_ID) {
                    add(&mut self.stats.arxiv_mismatch, -dir);
                }
                add(&mut self.stats.verified, dir);
            }
        }
        if is_retracted {
            add(&mut self.stats.retracted, -dir);
        }
    }

    /// Number of completed results.
    pub fn completed_count(&self) -> usize {
        self.results.iter().filter(|r| r.is_some()).count()
    }

    /// Number of problems (not_found + mismatch + retracted).
    pub fn problems(&self) -> usize {
        self.stats.not_found + self.stats.mismatch + self.stats.retracted
    }

    /// Percentage of references that are problematic (0.0 - 100.0).
    ///
    /// Uses checkable refs (total minus skipped) as the denominator so the
    /// percentage reflects only refs that actually entered the validation pipeline.
    pub fn problematic_pct(&self) -> f64 {
        let checkable = self.total_refs.saturating_sub(self.stats.skipped);
        if checkable == 0 {
            0.0
        } else {
            (self.problems() as f64 / checkable as f64) * 100.0
        }
    }
}

/// Sort order for the queue table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    Original,
    Problems,
    NotFound,
    ProblematicPct,
    Name,
    Status,
}

impl SortOrder {
    pub fn next(self) -> Self {
        match self {
            Self::Original => Self::Problems,
            Self::Problems => Self::NotFound,
            Self::NotFound => Self::ProblematicPct,
            Self::ProblematicPct => Self::Name,
            Self::Name => Self::Status,
            Self::Status => Self::Original,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Original => "order",
            Self::Problems => "problems",
            Self::NotFound => "not found",
            Self::ProblematicPct => "% flagged",
            Self::Name => "name",
            Self::Status => "status",
        }
    }
}

impl PaperPhase {
    /// Sort key for status ordering: active phases first, then completed, then queued.
    pub fn sort_key(&self) -> u8 {
        match self {
            Self::Checking => 0,
            Self::Extracting => 1,
            Self::Retrying => 2,
            Self::Complete => 3,
            Self::ExtractionFailed => 4,
            Self::Queued => 5,
        }
    }
}

/// Filter for the queue table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueFilter {
    All,
    HasProblems,
    Done,
    Running,
    Queued,
}

impl QueueFilter {
    pub fn next(self) -> Self {
        match self {
            Self::All => Self::HasProblems,
            Self::HasProblems => Self::Done,
            Self::Done => Self::Running,
            Self::Running => Self::Queued,
            Self::Queued => Self::All,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::HasProblems => "problems",
            Self::Done => "done",
            Self::Running => "running",
            Self::Queued => "queued",
        }
    }

    pub fn matches(self, paper: &PaperState) -> bool {
        match self {
            Self::All => true,
            Self::HasProblems => paper.problems() > 0,
            Self::Done => paper.phase.is_terminal(),
            Self::Running => matches!(
                paper.phase,
                PaperPhase::Extracting | PaperPhase::Checking | PaperPhase::Retrying
            ),
            Self::Queued => paper.phase == PaperPhase::Queued,
        }
    }
}

/// Compute filtered indices from the papers list, applying filter and optional search.
pub fn filtered_indices(
    papers: &[PaperState],
    filter: QueueFilter,
    search_query: &str,
) -> Vec<usize> {
    let query_lower = search_query.to_lowercase();
    papers
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            filter.matches(p)
                && (search_query.is_empty() || p.filename.to_lowercase().contains(&query_lower))
        })
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paper_with_recorded(statuses: &[(Status, bool)]) -> PaperState {
        // Helper: build a paper and run record_status for each entry so
        // the raw counters land in the exact shape the live event loop
        // would produce. `.1` is `is_retracted`.
        let mut p = PaperState::new("t".into());
        p.init_results(statuses.len());
        for (i, (status, is_retracted)) in statuses.iter().enumerate() {
            p.record_status(i, status.clone(), *is_retracted);
        }
        p.stats.total = statuses.len();
        p.total_refs = statuses.len();
        p
    }

    #[test]
    fn apply_fp_delta_not_found_marks_safe() {
        // Single not_found ref → raw stats: not_found=1, verified=0.
        // Mark safe → not_found=0, verified=1.
        let mut p = paper_with_recorded(&[(Status::NotFound, false)]);
        assert_eq!(p.stats.not_found, 1);
        assert_eq!(p.stats.verified, 0);
        p.apply_fp_delta(&Status::NotFound, false, 1);
        assert_eq!(p.stats.not_found, 0);
        assert_eq!(p.stats.verified, 1);
    }

    #[test]
    fn apply_fp_delta_not_found_is_reversible() {
        // Mark safe then un-mark → back to original raw counts.
        let mut p = paper_with_recorded(&[(Status::NotFound, false)]);
        p.apply_fp_delta(&Status::NotFound, false, 1);
        p.apply_fp_delta(&Status::NotFound, false, -1);
        assert_eq!(p.stats.not_found, 1);
        assert_eq!(p.stats.verified, 0);
    }

    #[test]
    fn apply_fp_delta_mismatch_decrements_all_matching_subflags() {
        let kind = MismatchKind::AUTHOR | MismatchKind::DOI;
        let mut p = paper_with_recorded(&[(Status::Mismatch(kind), false)]);
        assert_eq!(p.stats.mismatch, 1);
        assert_eq!(p.stats.author_mismatch, 1);
        assert_eq!(p.stats.doi_mismatch, 1);
        assert_eq!(p.stats.arxiv_mismatch, 0);
        p.apply_fp_delta(&Status::Mismatch(kind), false, 1);
        assert_eq!(p.stats.mismatch, 0);
        assert_eq!(p.stats.author_mismatch, 0);
        assert_eq!(p.stats.doi_mismatch, 0);
        assert_eq!(p.stats.arxiv_mismatch, 0);
        assert_eq!(p.stats.verified, 1);
    }

    #[test]
    fn apply_fp_delta_retracted_toggles_independently_of_status() {
        // A retracted ref can coexist with any status; the `retracted`
        // counter is separate and must be toggled on apply_fp_delta.
        let mut p = paper_with_recorded(&[(Status::Verified, true)]);
        assert_eq!(p.stats.verified, 1);
        assert_eq!(p.stats.retracted, 1);
        p.apply_fp_delta(&Status::Verified, true, 1);
        // Status::Verified: verified unchanged. Retracted decrements.
        assert_eq!(p.stats.verified, 1);
        assert_eq!(p.stats.retracted, 0);
    }

    #[test]
    fn apply_fp_delta_verified_status_only_retracted_flips() {
        let mut p = paper_with_recorded(&[(Status::Verified, false)]);
        let before = p.stats.clone();
        p.apply_fp_delta(&Status::Verified, false, 1);
        // Status::Verified + not_retracted → nothing to move.
        assert_eq!(p.stats.verified, before.verified);
        assert_eq!(p.stats.not_found, before.not_found);
        assert_eq!(p.stats.mismatch, before.mismatch);
    }

    #[test]
    fn problems_count_drops_when_all_refs_marked_safe() {
        // End-to-end: a paper with 2 not_found + 1 verified refs. After
        // marking both not_found refs safe, problems() must read 0.
        let mut p = paper_with_recorded(&[
            (Status::NotFound, false),
            (Status::NotFound, false),
            (Status::Verified, false),
        ]);
        assert_eq!(p.problems(), 2);
        p.apply_fp_delta(&Status::NotFound, false, 1);
        p.apply_fp_delta(&Status::NotFound, false, 1);
        assert_eq!(p.problems(), 0);
        assert_eq!(p.stats.verified, 3);
    }
}
