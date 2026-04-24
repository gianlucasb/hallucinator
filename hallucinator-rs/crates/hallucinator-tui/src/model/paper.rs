use hallucinator_core::{MismatchKind, Reference, Status, ValidationResult};

pub use hallucinator_reporting::FpReason;

/// Processing phase of a single reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefPhase {
    Pending,
    Checking,
    #[allow(dead_code)] // used in verdict_label display, constructed when retry tracking is wired
    Retrying,
    Done,
    /// Reference was skipped during extraction (URL-only, short title, etc.).
    Skipped(String),
}

/// State of a single reference within a paper.
#[derive(Debug, Clone)]
pub struct RefState {
    pub index: usize,
    pub title: String,
    pub phase: RefPhase,
    pub result: Option<ValidationResult>,
    /// Why the user marked this reference as a false positive, or None if not overridden.
    pub fp_reason: Option<FpReason>,
    /// Raw citation text from extraction (always available, even for skipped refs).
    pub raw_citation: String,
    /// Authors parsed during extraction.
    pub authors: Vec<String>,
    /// DOI extracted during parsing.
    pub doi: Option<String>,
    /// arXiv ID extracted during parsing.
    pub arxiv_id: Option<String>,
    /// URLs extracted from the reference (for URL liveness check fallback).
    pub urls: Vec<String>,
}

impl RefState {
    /// Reconstruct a `Reference` from this ref state (for retry support).
    pub fn to_reference(&self) -> Reference {
        let title = if self.title.is_empty() {
            None
        } else {
            Some(self.title.clone())
        };
        let skip_reason = if let RefPhase::Skipped(reason) = &self.phase {
            Some(reason.clone())
        } else {
            None
        };
        Reference {
            raw_citation: self.raw_citation.clone(),
            title,
            authors: self.authors.clone(),
            doi: self.doi.clone(),
            arxiv_id: self.arxiv_id.clone(),
            urls: self.urls.clone(),
            original_number: self.index + 1,
            skip_reason,
        }
    }

    /// Whether the user has marked this reference as safe (any FP reason).
    pub fn is_marked_safe(&self) -> bool {
        self.fp_reason.is_some()
    }

    /// Whether this reference is an unresolved problem — i.e., validation
    /// flagged it as not-found / author-mismatch / retracted AND the user
    /// has not yet marked it safe.
    ///
    /// Matches the problem definition used by the paper-level stats
    /// counters (`stats.not_found`, `stats.author_mismatch`,
    /// `stats.retracted`): a DOI-only or arXiv-only mismatch without an
    /// author mismatch does not count here (consistent with the paper
    /// stats, which track author mismatches separately from DOI/arXiv
    /// ID mismatches).
    pub fn is_unresolved_problem(&self) -> bool {
        if self.fp_reason.is_some() {
            return false;
        }
        let Some(result) = &self.result else {
            return false;
        };
        // A `url_check_skipped` NotFound is intentionally not an
        // unresolved problem — the user opted not to URL-verify that
        // ref, so it's bucketed under "skipped" and must not inflate
        // the paper-level problem count.
        let is_real_not_found =
            matches!(result.status, Status::NotFound) && !result.url_check_skipped;
        let status_problem = is_real_not_found
            || matches!(result.status, Status::Mismatch(kind) if kind.contains(MismatchKind::AUTHOR));
        let retracted = result
            .retraction_info
            .as_ref()
            .is_some_and(|ri| ri.is_retracted);
        status_problem || retracted
    }

    pub fn verdict_label(&self) -> String {
        if let Some(reason) = self.fp_reason {
            return format!("\u{2713} Safe ({})", reason.short_label());
        }
        if let RefPhase::Skipped(reason) = &self.phase {
            return match reason.as_str() {
                "url_only" => "(skipped: URL-only)".to_string(),
                "short_title" => "(skipped: short title)".to_string(),
                "no_title" => "(skipped: no title)".to_string(),
                other => format!("(skipped: {})", other),
            };
        }
        match &self.result {
            None => match self.phase {
                RefPhase::Pending => "\u{2014}".to_string(),
                RefPhase::Checking => "...".to_string(),
                RefPhase::Retrying => "retrying...".to_string(),
                RefPhase::Done => "\u{2014}".to_string(),
                RefPhase::Skipped(_) => unreachable!(),
            },
            Some(r) => match r.status {
                Status::Verified => {
                    if r.retraction_info.as_ref().is_some_and(|ri| ri.is_retracted) {
                        "\u{2620} RETRACTED".to_string()
                    } else {
                        "\u{2713} Verified".to_string()
                    }
                }
                // URL-gated NotFound renders as "skipped (URL check
                // disabled)" so the ref visually sits alongside the
                // parse-time skips and doesn't look like a
                // hallucination candidate.
                Status::NotFound if r.url_check_skipped => {
                    "(skipped: URL check disabled)".to_string()
                }
                Status::NotFound => "\u{2717} Not Found".to_string(),
                Status::Mismatch(_) => "\u{26A0} Mismatch".to_string(),
            },
        }
    }

    pub fn source_label(&self) -> &str {
        if matches!(self.phase, RefPhase::Skipped(_)) {
            return "\u{2014}";
        }
        match &self.result {
            Some(r) => r.source.as_deref().unwrap_or("\u{2014}"),
            None => "\u{2014}",
        }
    }
}

/// Whether a paper has at least one reference that is still an unresolved
/// problem. Used by the export "Problematic papers" scope filter: a
/// paper where every problematic ref has been marked safe by the user
/// is excluded, because there is nothing left to show on the report.
///
/// This is the ref-level truth corresponding to
/// `hallucinator_reporting::adjusted_stats`'s post-adjustment buckets —
/// we can't call that helper directly from the TUI (it lives inside a
/// private export module and operates on ReportRef/ReportPaper rather
/// than RefState), so the predicate is duplicated here against the TUI
/// model.
pub fn has_unresolved_problems(ref_states: &[RefState]) -> bool {
    ref_states.iter().any(RefState::is_unresolved_problem)
}

/// Sort order for references in the paper view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaperSortOrder {
    RefNumber,
    Verdict,
    Source,
}

impl PaperSortOrder {
    pub fn next(self) -> Self {
        match self {
            Self::RefNumber => Self::Verdict,
            Self::Verdict => Self::Source,
            Self::Source => Self::RefNumber,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::RefNumber => "ref#",
            Self::Verdict => "verdict",
            Self::Source => "source",
        }
    }
}

/// Filter for references in the paper view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaperFilter {
    All,
    ProblemsOnly,
}

impl PaperFilter {
    pub fn next(self) -> Self {
        match self {
            Self::All => Self::ProblemsOnly,
            Self::ProblemsOnly => Self::All,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::ProblemsOnly => "problems",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hallucinator_core::{MismatchKind, RetractionInfo, Status, ValidationResult};

    fn empty_val_result(status: Status) -> ValidationResult {
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
            url_check_skipped: false,
        }
    }

    fn ref_state(
        phase: RefPhase,
        result: Option<ValidationResult>,
        fp_reason: Option<FpReason>,
    ) -> RefState {
        RefState {
            index: 0,
            title: "some title".into(),
            phase,
            result,
            fp_reason,
            raw_citation: String::new(),
            authors: Vec::new(),
            doi: None,
            arxiv_id: None,
            urls: Vec::new(),
        }
    }

    #[test]
    fn unresolved_problem_not_found_with_no_fp_reason() {
        let rs = ref_state(
            RefPhase::Done,
            Some(empty_val_result(Status::NotFound)),
            None,
        );
        assert!(rs.is_unresolved_problem());
    }

    #[test]
    fn unresolved_problem_goes_false_once_marked_safe() {
        let rs = ref_state(
            RefPhase::Done,
            Some(empty_val_result(Status::NotFound)),
            Some(FpReason::KnownGood),
        );
        assert!(!rs.is_unresolved_problem());
    }

    #[test]
    fn unresolved_problem_author_mismatch_counts() {
        let rs = ref_state(
            RefPhase::Done,
            Some(empty_val_result(Status::Mismatch(MismatchKind::AUTHOR))),
            None,
        );
        assert!(rs.is_unresolved_problem());
    }

    #[test]
    fn unresolved_problem_doi_only_mismatch_does_not_count() {
        // Matches paper stats semantics: stats.author_mismatch is the
        // TUI's problematic-mismatch counter. A DOI-only or arXiv-ID-only
        // mismatch is a separate bucket and not flagged as problematic
        // by the existing export filter, so keep that behavior here.
        let rs = ref_state(
            RefPhase::Done,
            Some(empty_val_result(Status::Mismatch(MismatchKind::DOI))),
            None,
        );
        assert!(!rs.is_unresolved_problem());
    }

    #[test]
    fn unresolved_problem_retracted_counts_even_if_verified() {
        let mut result = empty_val_result(Status::Verified);
        result.retraction_info = Some(RetractionInfo {
            is_retracted: true,
            retraction_doi: Some("10.9/retr".into()),
            retraction_source: None,
        });
        let rs = ref_state(RefPhase::Done, Some(result), None);
        assert!(rs.is_unresolved_problem());
    }

    #[test]
    fn unresolved_problem_retracted_goes_false_when_marked_safe() {
        let mut result = empty_val_result(Status::Verified);
        result.retraction_info = Some(RetractionInfo {
            is_retracted: true,
            retraction_doi: None,
            retraction_source: None,
        });
        let rs = ref_state(RefPhase::Done, Some(result), Some(FpReason::KnownGood));
        assert!(!rs.is_unresolved_problem());
    }

    #[test]
    fn unresolved_problem_false_for_verified_ref() {
        let rs = ref_state(
            RefPhase::Done,
            Some(empty_val_result(Status::Verified)),
            None,
        );
        assert!(!rs.is_unresolved_problem());
    }

    #[test]
    fn unresolved_problem_false_for_pending_ref() {
        // No validation result yet — not problematic (and not verified
        // either). A paper that is still processing should not be
        // flagged as problematic by the export filter.
        let rs = ref_state(RefPhase::Pending, None, None);
        assert!(!rs.is_unresolved_problem());
    }

    #[test]
    fn unresolved_problem_false_for_skipped_ref() {
        // Skipped references have no validation result and are not
        // problematic for export purposes.
        let rs = ref_state(RefPhase::Skipped("short_title".into()), None, None);
        assert!(!rs.is_unresolved_problem());
    }

    // ── has_unresolved_problems (paper-level) ────────────────────────

    #[test]
    fn paper_clean_when_all_refs_verified() {
        let refs = vec![
            ref_state(
                RefPhase::Done,
                Some(empty_val_result(Status::Verified)),
                None,
            ),
            ref_state(
                RefPhase::Done,
                Some(empty_val_result(Status::Verified)),
                None,
            ),
        ];
        assert!(!has_unresolved_problems(&refs));
    }

    #[test]
    fn paper_clean_when_all_problematic_refs_marked_safe() {
        // This is the bug the user reported: a paper with previously
        // problematic references that have all been marked safe should
        // be excluded from the "Problematic papers" export scope. The
        // raw paper stats would still show positive counters, but this
        // predicate correctly looks at the overridden ref states.
        let refs = vec![
            ref_state(
                RefPhase::Done,
                Some(empty_val_result(Status::NotFound)),
                Some(FpReason::KnownGood),
            ),
            ref_state(
                RefPhase::Done,
                Some(empty_val_result(Status::Mismatch(MismatchKind::AUTHOR))),
                Some(FpReason::ExistsElsewhere),
            ),
            ref_state(
                RefPhase::Done,
                Some(empty_val_result(Status::Verified)),
                None,
            ),
        ];
        assert!(!has_unresolved_problems(&refs));
    }

    #[test]
    fn paper_problematic_when_one_unmarked_problem_remains() {
        let refs = vec![
            // Marked safe — shouldn't count.
            ref_state(
                RefPhase::Done,
                Some(empty_val_result(Status::NotFound)),
                Some(FpReason::KnownGood),
            ),
            // Still problematic — should tip the paper into
            // "has unresolved problems".
            ref_state(
                RefPhase::Done,
                Some(empty_val_result(Status::NotFound)),
                None,
            ),
        ];
        assert!(has_unresolved_problems(&refs));
    }

    #[test]
    fn paper_problematic_when_retracted_ref_is_not_marked_safe() {
        let mut retracted = empty_val_result(Status::Verified);
        retracted.retraction_info = Some(RetractionInfo {
            is_retracted: true,
            retraction_doi: None,
            retraction_source: None,
        });
        let refs = vec![ref_state(RefPhase::Done, Some(retracted), None)];
        assert!(has_unresolved_problems(&refs));
    }
}
