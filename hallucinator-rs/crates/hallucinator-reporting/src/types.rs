use hallucinator_core::{CheckStats, ValidationResult};

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

}

impl std::str::FromStr for FpReason {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "broken_parse" => Ok(FpReason::BrokenParse),
            "exists_elsewhere" => Ok(FpReason::ExistsElsewhere),
            "all_timed_out" => Ok(FpReason::AllTimedOut),
            "known_good" => Ok(FpReason::KnownGood),
            "non_academic" => Ok(FpReason::NonAcademic),
            _ => Err(()),
        }
    }
}

/// User-assigned verdict for an entire paper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaperVerdict {
    Safe,
    Questionable,
}

impl PaperVerdict {
    /// Cycle: None → Safe → Questionable → None.
    pub fn cycle(current: Option<Self>) -> Option<Self> {
        match current {
            None => Some(Self::Safe),
            Some(Self::Safe) => Some(Self::Questionable),
            Some(Self::Questionable) => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Safe => "SAFE",
            Self::Questionable => "?!",
        }
    }
}

/// Export format options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Json,
    Csv,
    Markdown,
    Text,
    Html,
}

impl ExportFormat {
    pub fn all() -> &'static [ExportFormat] {
        &[
            ExportFormat::Json,
            ExportFormat::Csv,
            ExportFormat::Markdown,
            ExportFormat::Text,
            ExportFormat::Html,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Json => "JSON",
            Self::Csv => "CSV",
            Self::Markdown => "Markdown",
            Self::Text => "Plain Text",
            Self::Html => "HTML",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Csv => "csv",
            Self::Markdown => "md",
            Self::Text => "txt",
            Self::Html => "html",
        }
    }
}

/// Lightweight input struct for a paper's results, used by the export module.
/// Consumers (TUI, CLI) build this from their internal state types.
pub struct ReportPaper<'a> {
    pub filename: &'a str,
    pub stats: &'a CheckStats,
    pub results: &'a [Option<ValidationResult>],
    pub verdict: Option<PaperVerdict>,
}

/// Lightweight input struct for a single reference, used by the export module.
pub struct ReportRef {
    pub index: usize,
    pub title: String,
    pub skip_info: Option<SkipInfo>,
    pub fp_reason: Option<FpReason>,
}

/// Information about why a reference was skipped.
pub struct SkipInfo {
    pub reason: String,
}
