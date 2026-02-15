use std::path::PathBuf;

use hallucinator_core::{ProgressEvent, Reference, ValidationResult};
use hallucinator_pdf::SkipStats;

/// Commands sent from the TUI to the backend.
pub enum BackendCommand {
    /// Start processing files. `starting_index` is the offset into the app's paper list.
    ProcessFiles {
        files: Vec<PathBuf>,
        starting_index: usize,
        max_concurrent_papers: usize,
        config: Box<hallucinator_core::Config>,
    },
    /// Retry specific references for a paper.
    /// Each tuple is (ref_index, Reference, failed_dbs). If failed_dbs is empty,
    /// the reference is re-checked against all databases.
    RetryReferences {
        paper_index: usize,
        refs_to_retry: Vec<(usize, Reference, Vec<String>)>,
        config: Box<hallucinator_core::Config>,
    },
    /// Cancel the current batch.
    CancelProcessing,
    /// Build/update the offline DBLP database.
    BuildDblp { db_path: PathBuf },
    /// Build/update the offline ACL Anthology database.
    BuildAcl { db_path: PathBuf },
}

/// Events flowing from the backend processing task to the TUI.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum BackendEvent {
    /// PDF text extraction started for paper at queue index.
    ExtractionStarted { paper_index: usize },
    /// PDF extraction completed — references parsed.
    ExtractionComplete {
        paper_index: usize,
        ref_count: usize,
        ref_titles: Vec<String>,
        references: Vec<Reference>,
        skip_stats: SkipStats,
    },
    /// PDF extraction failed.
    ExtractionFailed { paper_index: usize, error: String },
    /// Progress event from check_references (checking/result/warning/retry).
    Progress {
        paper_index: usize,
        event: Box<ProgressEvent>,
    },
    /// All references for a paper have been checked.
    PaperComplete {
        paper_index: usize,
        results: Vec<ValidationResult>,
    },
    /// All papers have been processed.
    BatchComplete,
    /// Progress from a DBLP database build.
    DblpBuildProgress {
        event: hallucinator_dblp::BuildProgress,
    },
    /// DBLP database build completed.
    DblpBuildComplete {
        success: bool,
        error: Option<String>,
        db_path: PathBuf,
    },
    /// Progress from an ACL database build.
    AclBuildProgress {
        event: hallucinator_acl::BuildProgress,
    },
    /// ACL database build completed.
    AclBuildComplete {
        success: bool,
        error: Option<String>,
        db_path: PathBuf,
    },
}
