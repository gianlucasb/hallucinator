use std::io::Write;
use std::path::PathBuf;

use crate::model::paper::{RefPhase, RefState};
use crate::model::queue::PaperState;

/// Get the run directory for persisting results.
/// Creates `~/.cache/hallucinator/runs/<timestamp>/` if it doesn't exist.
pub fn run_dir() -> Option<PathBuf> {
    let cache = dirs::cache_dir()?;
    let now = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let dir = cache
        .join("hallucinator")
        .join("runs")
        .join(now.to_string());
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Persist results for a single paper to the run directory.
///
/// Uses the same rich JSON format as the export module so that saved results
/// can be loaded back via `--load` or the file picker.
pub fn save_paper_results(
    run_dir: &std::path::Path,
    paper_index: usize,
    paper: &PaperState,
    ref_states: &[RefState],
) {
    let out_path = run_dir.join(format!("paper_{}.json", paper_index));
    let results_from_refs: Vec<Option<hallucinator_core::ValidationResult>> =
        ref_states.iter().map(|rs| rs.result.clone()).collect();
    let report_paper = hallucinator_reporting::ReportPaper {
        filename: &paper.filename,
        stats: &paper.stats,
        results: &results_from_refs,
        verdict: paper.verdict,
    };
    let report_refs: Vec<hallucinator_reporting::ReportRef> = ref_states
        .iter()
        .map(|rs| hallucinator_reporting::ReportRef {
            index: rs.index,
            title: rs.title.clone(),
            skip_info: if let RefPhase::Skipped(reason) = &rs.phase {
                Some(hallucinator_reporting::SkipInfo {
                    reason: reason.clone(),
                })
            } else {
                None
            },
            fp_reason: rs.fp_reason,
        })
        .collect();
    let json = hallucinator_reporting::export_json(&[report_paper], &[&report_refs]);

    if let Ok(mut file) = std::fs::File::create(&out_path) {
        let _ = file.write_all(json.as_bytes());
    }
}
