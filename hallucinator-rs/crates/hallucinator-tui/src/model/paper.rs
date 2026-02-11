use hallucinator_core::{Status, ValidationResult};

/// Processing phase of a single reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefPhase {
    Pending,
    Checking,
    #[allow(dead_code)] // used in verdict_label display, constructed when retry tracking is wired
    Retrying,
    Done,
}

/// State of a single reference within a paper.
#[derive(Debug, Clone)]
pub struct RefState {
    pub index: usize,
    pub title: String,
    pub phase: RefPhase,
    pub result: Option<ValidationResult>,
    /// User has marked this reference as safe (false positive override).
    pub marked_safe: bool,
}

impl RefState {
    pub fn verdict_label(&self) -> &str {
        if self.marked_safe {
            return "\u{2713} Safe (FP)";
        }
        match &self.result {
            None => match self.phase {
                RefPhase::Pending => "\u{2014}",
                RefPhase::Checking => "...",
                RefPhase::Retrying => "retrying...",
                RefPhase::Done => "\u{2014}",
            },
            Some(r) => match r.status {
                Status::Verified => {
                    if r.retraction_info.as_ref().is_some_and(|ri| ri.is_retracted) {
                        "\u{2620} RETRACTED"
                    } else {
                        "\u{2713} Verified"
                    }
                }
                Status::NotFound => "\u{2717} Not Found",
                Status::AuthorMismatch => "\u{26A0} Mismatch",
            },
        }
    }

    pub fn source_label(&self) -> &str {
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
