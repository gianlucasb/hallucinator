use std::io::Write;
use std::path::PathBuf;

use hallucinator_core::ValidationResult;

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
pub fn save_paper_results(
    run_dir: &std::path::Path,
    paper_index: usize,
    filename: &str,
    results: &[Option<ValidationResult>],
) {
    let out_path = run_dir.join(format!("paper_{}.json", paper_index));

    let mut out = String::from("{\n");
    out.push_str(&format!("  \"filename\": {:?},\n", filename));
    out.push_str("  \"references\": [\n");

    for (i, result) in results.iter().enumerate() {
        if let Some(r) = result {
            let status = match r.status {
                hallucinator_core::Status::Verified => "verified",
                hallucinator_core::Status::NotFound => "not_found",
                hallucinator_core::Status::AuthorMismatch => "author_mismatch",
            };
            let retracted = r.retraction_info.as_ref().is_some_and(|ri| ri.is_retracted);
            out.push_str(&format!(
                "    {{\"index\": {}, \"title\": {:?}, \"status\": {:?}, \"source\": {:?}, \"retracted\": {}}}",
                i,
                r.title,
                status,
                r.source.as_deref().unwrap_or(""),
                retracted,
            ));
        } else {
            out.push_str(&format!("    {{\"index\": {}, \"title\": null, \"status\": \"pending\", \"source\": \"\", \"retracted\": false}}", i));
        }

        if i + 1 < results.len() {
            out.push(',');
        }
        out.push('\n');
    }

    out.push_str("  ]\n}\n");

    if let Ok(mut file) = std::fs::File::create(&out_path) {
        let _ = file.write_all(out.as_bytes());
    }
}
