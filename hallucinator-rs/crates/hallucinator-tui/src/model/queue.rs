use hallucinator_core::{CheckStats, Status, ValidationResult};

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
    pub results: Vec<Option<ValidationResult>>,
    pub error: Option<String>,
    /// Total refs to retry in the retry pass.
    pub retry_total: usize,
    /// Completed retry count.
    pub retry_done: usize,
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
        }
    }

    /// Pre-allocate result slots once the reference count is known.
    pub fn init_results(&mut self, count: usize) {
        self.results = vec![None; count];
    }

    /// Record (or replace) a validation result at the given index.
    ///
    /// If the slot already contains a result (retry pass), the old status
    /// counters are decremented before the new ones are incremented, preventing
    /// double-counting.
    pub fn record_result(&mut self, index: usize, result: ValidationResult) {
        // Grow if needed (shouldn't happen after init_results, but be safe)
        if index >= self.results.len() {
            self.results.resize(index + 1, None);
        }

        // Decrement old counters if replacing
        if let Some(old) = &self.results[index] {
            match old.status {
                Status::Verified => self.stats.verified = self.stats.verified.saturating_sub(1),
                Status::NotFound => self.stats.not_found = self.stats.not_found.saturating_sub(1),
                Status::AuthorMismatch => {
                    self.stats.author_mismatch = self.stats.author_mismatch.saturating_sub(1)
                }
            }
            if old.retraction_info.as_ref().is_some_and(|r| r.is_retracted) {
                self.stats.retracted = self.stats.retracted.saturating_sub(1);
            }
        }

        // Increment new counters
        match result.status {
            Status::Verified => self.stats.verified += 1,
            Status::NotFound => self.stats.not_found += 1,
            Status::AuthorMismatch => self.stats.author_mismatch += 1,
        }
        if result
            .retraction_info
            .as_ref()
            .is_some_and(|r| r.is_retracted)
        {
            self.stats.retracted += 1;
        }

        self.results[index] = Some(result);
    }

    /// Number of completed results.
    pub fn completed_count(&self) -> usize {
        self.results.iter().filter(|r| r.is_some()).count()
    }

    /// Number of problems (not_found + author_mismatch + retracted).
    pub fn problems(&self) -> usize {
        self.stats.not_found + self.stats.author_mismatch + self.stats.retracted
    }

    /// Percentage of checked references that are problematic (0.0 - 100.0).
    pub fn problematic_pct(&self) -> f64 {
        let checked = self.total_refs.saturating_sub(self.stats.skipped);
        if checked == 0 {
            0.0
        } else {
            (self.problems() as f64 / checked as f64) * 100.0
        }
    }
}

/// Sort order for the queue table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    Original,
    Problems,
    ProblematicPct,
    Name,
}

impl SortOrder {
    pub fn next(self) -> Self {
        match self {
            Self::Original => Self::Problems,
            Self::Problems => Self::ProblematicPct,
            Self::ProblematicPct => Self::Name,
            Self::Name => Self::Original,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Original => "order",
            Self::Problems => "problems",
            Self::ProblematicPct => "% flagged",
            Self::Name => "name",
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
