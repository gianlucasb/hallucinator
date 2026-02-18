use hallucinator_core::{Reference, Status, ValidationResult};

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
            original_number: self.index + 1,
            skip_reason,
        }
    }

    /// Whether the user has marked this reference as safe (any FP reason).
    pub fn is_marked_safe(&self) -> bool {
        self.fp_reason.is_some()
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
                Status::NotFound => "\u{2717} Not Found".to_string(),
                Status::AuthorMismatch => "\u{26A0} Mismatch".to_string(),
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
