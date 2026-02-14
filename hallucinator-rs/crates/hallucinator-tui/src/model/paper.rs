use hallucinator_core::{Status, ValidationResult};

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

/// Reason a user marked a reference as a false positive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FpReason {
    /// Citation parsing failed, title garbled.
    BrokenParse,
    /// Found on Google Scholar or another source not checked by the tool.
    ExistsElsewhere,
    /// All databases timed out; reference likely exists.
    AllTimedOut,
    /// User personally knows this reference is real.
    KnownGood,
    /// Non-academic source (RFC, legal document, news article, etc.).
    NonAcademic,
}

impl FpReason {
    /// Cycle: None → BrokenParse → ExistsElsewhere → AllTimedOut → KnownGood → NonAcademic → None.
    pub fn cycle(current: Option<FpReason>) -> Option<FpReason> {
        match current {
            None => Some(FpReason::BrokenParse),
            Some(FpReason::BrokenParse) => Some(FpReason::ExistsElsewhere),
            Some(FpReason::ExistsElsewhere) => Some(FpReason::AllTimedOut),
            Some(FpReason::AllTimedOut) => Some(FpReason::KnownGood),
            Some(FpReason::KnownGood) => Some(FpReason::NonAcademic),
            Some(FpReason::NonAcademic) => None,
        }
    }

    /// Short label for the verdict column (e.g. "parse", "GS").
    pub fn short_label(self) -> &'static str {
        match self {
            FpReason::BrokenParse => "parse",
            FpReason::ExistsElsewhere => "GS",
            FpReason::AllTimedOut => "timeout",
            FpReason::KnownGood => "known",
            FpReason::NonAcademic => "N/A",
        }
    }

    /// Human-readable description for the detail banner.
    pub fn description(self) -> &'static str {
        match self {
            FpReason::BrokenParse => "Broken citation parse",
            FpReason::ExistsElsewhere => "Found on Google Scholar / other source",
            FpReason::AllTimedOut => "All databases timed out",
            FpReason::KnownGood => "User verified as real",
            FpReason::NonAcademic => "Non-academic source (RFC, legal, news, etc.)",
        }
    }

    /// JSON-serializable string key.
    pub fn as_str(self) -> &'static str {
        match self {
            FpReason::BrokenParse => "broken_parse",
            FpReason::ExistsElsewhere => "exists_elsewhere",
            FpReason::AllTimedOut => "all_timed_out",
            FpReason::KnownGood => "known_good",
            FpReason::NonAcademic => "non_academic",
        }
    }

    /// Parse from a JSON string key.
    pub fn from_str(s: &str) -> Option<FpReason> {
        match s {
            "broken_parse" => Some(FpReason::BrokenParse),
            "exists_elsewhere" => Some(FpReason::ExistsElsewhere),
            "all_timed_out" => Some(FpReason::AllTimedOut),
            "known_good" => Some(FpReason::KnownGood),
            "non_academic" => Some(FpReason::NonAcademic),
            _ => None,
        }
    }
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
