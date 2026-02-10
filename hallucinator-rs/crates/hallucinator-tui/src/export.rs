use std::io::Write;
use std::path::Path;

use hallucinator_core::{Status, ValidationResult};

use crate::model::queue::PaperState;
use crate::view::export::ExportFormat;

/// Export results for a set of papers to the given path.
pub fn export_results(
    papers: &[(String, &[Option<ValidationResult>])],
    format: ExportFormat,
    path: &Path,
) -> Result<(), String> {
    let content = match format {
        ExportFormat::Json => export_json(papers),
        ExportFormat::Csv => export_csv(papers),
        ExportFormat::Markdown => export_markdown(papers),
        ExportFormat::Text => export_text(papers),
    };

    let mut file =
        std::fs::File::create(path).map_err(|e| format!("Failed to create file: {}", e))?;
    file.write_all(content.as_bytes())
        .map_err(|e| format!("Failed to write: {}", e))?;
    Ok(())
}

fn export_json(papers: &[(String, &[Option<ValidationResult>])]) -> String {
    let mut out = String::from("[\n");
    for (pi, (filename, results)) in papers.iter().enumerate() {
        out.push_str(&format!(
            "  {{\n    \"filename\": {:?},\n    \"references\": [\n",
            filename
        ));
        for (ri, result) in results.iter().enumerate() {
            if let Some(r) = result {
                let status = match r.status {
                    Status::Verified => "verified",
                    Status::NotFound => "not_found",
                    Status::AuthorMismatch => "author_mismatch",
                };
                let retracted = r
                    .retraction_info
                    .as_ref()
                    .map_or(false, |ri| ri.is_retracted);
                out.push_str(&format!(
                    "      {{\"index\": {}, \"title\": {:?}, \"status\": {:?}, \"source\": {:?}, \"retracted\": {}}}",
                    ri,
                    r.title,
                    status,
                    r.source.as_deref().unwrap_or(""),
                    retracted,
                ));
                if ri + 1 < results.len() {
                    out.push(',');
                }
                out.push('\n');
            }
        }
        out.push_str("    ]\n  }");
        if pi + 1 < papers.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("]\n");
    out
}

fn export_csv(papers: &[(String, &[Option<ValidationResult>])]) -> String {
    let mut out = String::from("Filename,Ref#,Title,Status,Source,Retracted\n");
    for (filename, results) in papers {
        for (ri, result) in results.iter().enumerate() {
            if let Some(r) = result {
                let status = match r.status {
                    Status::Verified => "verified",
                    Status::NotFound => "not_found",
                    Status::AuthorMismatch => "author_mismatch",
                };
                let retracted = r
                    .retraction_info
                    .as_ref()
                    .map_or(false, |ri| ri.is_retracted);
                out.push_str(&format!(
                    "{:?},{},{:?},{},{:?},{}\n",
                    filename,
                    ri + 1,
                    r.title,
                    status,
                    r.source.as_deref().unwrap_or(""),
                    retracted,
                ));
            }
        }
    }
    out
}

fn export_markdown(papers: &[(String, &[Option<ValidationResult>])]) -> String {
    let mut out = String::from("# Hallucinator Results\n\n");
    for (filename, results) in papers {
        out.push_str(&format!("## {}\n\n", filename));
        out.push_str("| # | Title | Status | Source |\n");
        out.push_str("|---|-------|--------|--------|\n");
        for (ri, result) in results.iter().enumerate() {
            if let Some(r) = result {
                let status = match r.status {
                    Status::Verified => {
                        if r.retraction_info
                            .as_ref()
                            .map_or(false, |ri| ri.is_retracted)
                        {
                            "\u{2620} RETRACTED"
                        } else {
                            "\u{2713} Verified"
                        }
                    }
                    Status::NotFound => "\u{2717} Not Found",
                    Status::AuthorMismatch => "\u{26A0} Mismatch",
                };
                let source = r.source.as_deref().unwrap_or("\u{2014}");
                let title_escaped = r.title.replace('|', "\\|");
                out.push_str(&format!(
                    "| {} | {} | {} | {} |\n",
                    ri + 1,
                    title_escaped,
                    status,
                    source
                ));
            }
        }
        out.push('\n');
    }
    out
}

fn export_text(papers: &[(String, &[Option<ValidationResult>])]) -> String {
    let mut out = String::from("Hallucinator Results\n");
    out.push_str(&"=".repeat(60));
    out.push('\n');
    for (filename, results) in papers {
        out.push_str(&format!("\n{}\n", filename));
        out.push_str(&"-".repeat(filename.len()));
        out.push('\n');
        for (ri, result) in results.iter().enumerate() {
            if let Some(r) = result {
                let status = match r.status {
                    Status::Verified => "Verified",
                    Status::NotFound => "NOT FOUND",
                    Status::AuthorMismatch => "Author Mismatch",
                };
                let retracted = if r
                    .retraction_info
                    .as_ref()
                    .map_or(false, |ri| ri.is_retracted)
                {
                    " [RETRACTED]"
                } else {
                    ""
                };
                let source = r.source.as_deref().unwrap_or("-");
                out.push_str(&format!(
                    "  [{}] {} - {} ({}){}\n",
                    ri + 1,
                    r.title,
                    status,
                    source,
                    retracted
                ));
            }
        }
    }
    out
}

/// Build paper data tuples from App state for export.
pub fn collect_paper_data(papers: &[PaperState]) -> Vec<(String, Vec<Option<ValidationResult>>)> {
    papers
        .iter()
        .map(|p| (p.filename.clone(), p.results.clone()))
        .collect()
}
