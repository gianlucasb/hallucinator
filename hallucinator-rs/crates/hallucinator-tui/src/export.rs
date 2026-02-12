use std::io::Write;
use std::path::Path;

use hallucinator_core::{CheckStats, DbStatus, Status, ValidationResult};

use crate::model::paper::RefState;
use crate::model::queue::{PaperState, PaperVerdict};
use crate::view::export::ExportFormat;

/// Export results for a set of papers to the given path.
///
/// `ref_states` is a parallel slice to `papers` — `ref_states[i]` are the RefStates
/// for `papers[i]`. This is used to include FP reason overrides in the output.
pub fn export_results(
    papers: &[&PaperState],
    ref_states: &[&[RefState]],
    format: ExportFormat,
    path: &Path,
) -> Result<(), String> {
    let content = match format {
        ExportFormat::Json => export_json(papers, ref_states),
        ExportFormat::Csv => export_csv(papers, ref_states),
        ExportFormat::Markdown => export_markdown(papers, ref_states),
        ExportFormat::Text => export_text(papers, ref_states),
        ExportFormat::Html => export_html(papers, ref_states),
    };

    let mut file =
        std::fs::File::create(path).map_err(|e| format!("Failed to create file: {}", e))?;
    file.write_all(content.as_bytes())
        .map_err(|e| format!("Failed to write: {}", e))?;
    Ok(())
}

fn status_str(s: &Status) -> &'static str {
    match s {
        Status::Verified => "verified",
        Status::NotFound => "not_found",
        Status::AuthorMismatch => "author_mismatch",
    }
}

fn verdict_str(v: Option<PaperVerdict>) -> &'static str {
    match v {
        Some(PaperVerdict::Safe) => "safe",
        Some(PaperVerdict::Questionable) => "questionable",
        None => "",
    }
}

fn is_retracted(r: &ValidationResult) -> bool {
    r.retraction_info.as_ref().is_some_and(|ri| ri.is_retracted)
}

fn problematic_pct(stats: &CheckStats) -> f64 {
    let checked = stats.total.saturating_sub(stats.skipped);
    if checked == 0 {
        0.0
    } else {
        let problems = stats.not_found + stats.author_mismatch + stats.retracted;
        (problems as f64 / checked as f64) * 100.0
    }
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < '\x20' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

fn json_str(s: &str) -> String {
    format!("\"{}\"", json_escape(s))
}

fn json_opt_str(s: &Option<String>) -> String {
    match s {
        Some(v) => json_str(v),
        None => "null".to_string(),
    }
}

fn json_str_array(v: &[String]) -> String {
    let items: Vec<String> = v.iter().map(|s| json_str(s)).collect();
    format!("[{}]", items.join(", "))
}

pub fn export_json(papers: &[&PaperState], ref_states: &[&[RefState]]) -> String {
    let mut out = String::from("[\n");
    for (pi, paper) in papers.iter().enumerate() {
        let s = &paper.stats;
        let verdict_json = match paper.verdict {
            Some(_) => json_str(verdict_str(paper.verdict)),
            None => "null".to_string(),
        };
        out.push_str(&format!(
            "  {{\n    \"filename\": {},\n    \"verdict\": {},\n    \"stats\": {{\n      \"total\": {},\n      \"verified\": {},\n      \"not_found\": {},\n      \"author_mismatch\": {},\n      \"retracted\": {},\n      \"skipped\": {},\n      \"problematic_pct\": {:.1}\n    }},\n    \"references\": [\n",
            json_str(&paper.filename),
            verdict_json,
            s.total, s.verified, s.not_found, s.author_mismatch, s.retracted, s.skipped,
            problematic_pct(s),
        ));
        let paper_refs = ref_states.get(pi).copied().unwrap_or(&[]);
        let ref_count = paper.results.iter().filter(|r| r.is_some()).count();
        let mut written = 0;
        for (ri, result) in paper.results.iter().enumerate() {
            if let Some(r) = result {
                written += 1;
                let fp_json = paper_refs
                    .get(ri)
                    .and_then(|rs| rs.fp_reason)
                    .map(|fp| json_str(fp.as_str()))
                    .unwrap_or_else(|| "null".to_string());
                out.push_str("      {\n");
                out.push_str(&format!("        \"index\": {},\n", ri));
                out.push_str(&format!("        \"title\": {},\n", json_str(&r.title)));
                out.push_str(&format!(
                    "        \"raw_citation\": {},\n",
                    json_str(&r.raw_citation)
                ));
                out.push_str(&format!(
                    "        \"status\": {},\n",
                    json_str(status_str(&r.status))
                ));
                out.push_str(&format!("        \"fp_reason\": {},\n", fp_json));
                out.push_str(&format!(
                    "        \"source\": {},\n",
                    json_opt_str(&r.source)
                ));
                out.push_str(&format!(
                    "        \"ref_authors\": {},\n",
                    json_str_array(&r.ref_authors)
                ));
                out.push_str(&format!(
                    "        \"found_authors\": {},\n",
                    json_str_array(&r.found_authors)
                ));
                out.push_str(&format!(
                    "        \"paper_url\": {},\n",
                    json_opt_str(&r.paper_url)
                ));
                out.push_str(&format!(
                    "        \"failed_dbs\": {},\n",
                    json_str_array(&r.failed_dbs)
                ));

                // DOI info
                if let Some(doi) = &r.doi_info {
                    out.push_str(&format!(
                        "        \"doi_info\": {{\"doi\": {}, \"valid\": {}, \"title\": {}}},\n",
                        json_str(&doi.doi),
                        doi.valid,
                        json_opt_str(&doi.title)
                    ));
                } else {
                    out.push_str("        \"doi_info\": null,\n");
                }

                // arXiv info
                if let Some(ax) = &r.arxiv_info {
                    out.push_str(&format!(
                        "        \"arxiv_info\": {{\"arxiv_id\": {}, \"valid\": {}, \"title\": {}}},\n",
                        json_str(&ax.arxiv_id),
                        ax.valid,
                        json_opt_str(&ax.title)
                    ));
                } else {
                    out.push_str("        \"arxiv_info\": null,\n");
                }

                // Retraction info
                if let Some(ret) = &r.retraction_info {
                    out.push_str(&format!(
                        "        \"retraction_info\": {{\"is_retracted\": {}, \"retraction_doi\": {}, \"retraction_source\": {}}},\n",
                        ret.is_retracted,
                        json_opt_str(&ret.retraction_doi),
                        json_opt_str(&ret.retraction_source)
                    ));
                } else {
                    out.push_str("        \"retraction_info\": null,\n");
                }

                // Per-DB results
                out.push_str("        \"db_results\": [");
                for (di, db) in r.db_results.iter().enumerate() {
                    let db_status = match db.status {
                        DbStatus::Match => "match",
                        DbStatus::NoMatch => "no_match",
                        DbStatus::AuthorMismatch => "author_mismatch",
                        DbStatus::Timeout => "timeout",
                        DbStatus::Error => "error",
                        DbStatus::Skipped => "skipped",
                    };
                    let elapsed_ms = db.elapsed.map(|d| d.as_millis()).unwrap_or(0);
                    out.push_str(&format!(
                        "{{\"db\": {}, \"status\": {}, \"elapsed_ms\": {}, \"authors\": {}, \"url\": {}}}",
                        json_str(&db.db_name),
                        json_str(db_status),
                        elapsed_ms,
                        json_str_array(&db.found_authors),
                        json_opt_str(&db.paper_url),
                    ));
                    if di + 1 < r.db_results.len() {
                        out.push_str(", ");
                    }
                }
                out.push_str("]\n");

                out.push_str("      }");
                if written < ref_count {
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

fn csv_escape(s: &str) -> String {
    if s.contains('"') || s.contains(',') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn export_csv(papers: &[&PaperState], ref_states: &[&[RefState]]) -> String {
    let mut out = String::from(
        "Filename,Verdict,Ref#,Title,Status,FpReason,Source,Retracted,Authors,FoundAuthors,PaperURL,DOI,ArxivID,FailedDBs\n",
    );
    for (pi, paper) in papers.iter().enumerate() {
        let verdict = verdict_str(paper.verdict);
        let paper_refs = ref_states.get(pi).copied().unwrap_or(&[]);
        for (ri, result) in paper.results.iter().enumerate() {
            if let Some(r) = result {
                let retracted = is_retracted(r);
                let fp = paper_refs
                    .get(ri)
                    .and_then(|rs| rs.fp_reason)
                    .map(|fp| fp.as_str())
                    .unwrap_or("");
                let authors = r.ref_authors.join("; ");
                let found = r.found_authors.join("; ");
                let url = r.paper_url.as_deref().unwrap_or("");
                let doi = r.doi_info.as_ref().map(|d| d.doi.as_str()).unwrap_or("");
                let arxiv = r
                    .arxiv_info
                    .as_ref()
                    .map(|a| a.arxiv_id.as_str())
                    .unwrap_or("");
                let failed = r.failed_dbs.join("; ");
                out.push_str(&format!(
                    "{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
                    csv_escape(&paper.filename),
                    csv_escape(verdict),
                    ri + 1,
                    csv_escape(&r.title),
                    status_str(&r.status),
                    csv_escape(fp),
                    csv_escape(r.source.as_deref().unwrap_or("")),
                    retracted,
                    csv_escape(&authors),
                    csv_escape(&found),
                    csv_escape(url),
                    csv_escape(doi),
                    csv_escape(arxiv),
                    csv_escape(&failed),
                ));
            }
        }
    }
    out
}

fn md_escape(s: &str) -> String {
    s.replace('|', "\\|")
}

fn scholar_url(title: &str) -> String {
    format!(
        "https://scholar.google.com/scholar?q={}",
        title.replace(' ', "+")
    )
}

fn export_markdown(papers: &[&PaperState], ref_states: &[&[RefState]]) -> String {
    let mut out = String::from("# Hallucinator Results\n\n");

    for (pi, paper) in papers.iter().enumerate() {
        let paper_refs = ref_states.get(pi).copied().unwrap_or(&[]);
        let s = &paper.stats;
        let verdict_badge = match paper.verdict {
            Some(PaperVerdict::Safe) => " **[SAFE]**",
            Some(PaperVerdict::Questionable) => " **[?!]**",
            None => "",
        };
        out.push_str(&format!("## {}{}\n\n", paper.filename, verdict_badge));

        // Stats summary
        out.push_str(&format!(
            "**{}** references | **{}** verified | **{}** not found | **{}** mismatch | **{}** retracted | **{}** skipped | **{:.1}%** problematic\n\n",
            s.total, s.verified, s.not_found, s.author_mismatch, s.retracted, s.skipped,
            problematic_pct(s),
        ));

        // Group: problems first, then verified
        let mut problems: Vec<(usize, &ValidationResult)> = Vec::new();
        let mut verified: Vec<(usize, &ValidationResult)> = Vec::new();
        for (ri, result) in paper.results.iter().enumerate() {
            if let Some(r) = result {
                if r.status != Status::Verified || is_retracted(r) {
                    problems.push((ri, r));
                } else {
                    verified.push((ri, r));
                }
            }
        }

        if !problems.is_empty() {
            out.push_str("### Problematic References\n\n");
            for (ri, r) in &problems {
                write_md_ref(&mut out, *ri, r);
                if let Some(fp) = paper_refs.get(*ri).and_then(|rs| rs.fp_reason) {
                    out.push_str(&format!(
                        "- **User override:** Marked safe ({})\n\n",
                        fp.description()
                    ));
                }
            }
        }

        if !verified.is_empty() {
            out.push_str("### Verified References\n\n");
            out.push_str("| # | Title | Source | URL | FP Override |\n");
            out.push_str("|---|-------|--------|-----|-------------|\n");
            for (ri, r) in &verified {
                let source = r.source.as_deref().unwrap_or("\u{2014}");
                let url = r
                    .paper_url
                    .as_ref()
                    .map(|u| format!("[link]({})", u))
                    .unwrap_or_default();
                let fp_col = paper_refs
                    .get(*ri)
                    .and_then(|rs| rs.fp_reason)
                    .map(|fp| fp.short_label().to_string())
                    .unwrap_or_default();
                out.push_str(&format!(
                    "| {} | {} | {} | {} | {} |\n",
                    ri + 1,
                    md_escape(&r.title),
                    source,
                    url,
                    fp_col,
                ));
            }
            out.push('\n');
        }

        out.push_str("---\n\n");
    }
    out
}

fn write_md_ref(out: &mut String, ri: usize, r: &ValidationResult) {
    let status_icon = if is_retracted(r) {
        "\u{2620}\u{fe0f} RETRACTED"
    } else {
        match r.status {
            Status::NotFound => "\u{2717} Not Found",
            Status::AuthorMismatch => "\u{26a0}\u{fe0f} Author Mismatch",
            Status::Verified => "\u{2713} Verified",
        }
    };

    out.push_str(&format!(
        "**[{}]** {} \u{2014} {}\n\n",
        ri + 1,
        md_escape(&r.title),
        status_icon,
    ));

    // Author comparison for mismatches
    if r.status == Status::AuthorMismatch {
        if !r.ref_authors.is_empty() {
            out.push_str(&format!(
                "- **PDF authors:** {}\n",
                r.ref_authors.join(", ")
            ));
        }
        if !r.found_authors.is_empty() {
            out.push_str(&format!(
                "- **DB authors:** {}\n",
                r.found_authors.join(", ")
            ));
        }
        if let Some(src) = &r.source {
            out.push_str(&format!("- **Source:** {}\n", src));
        }
    }

    // DOI/arXiv issues
    if let Some(doi) = &r.doi_info {
        if !doi.valid {
            out.push_str(&format!(
                "- **DOI** `{}` \u{2014} invalid/unresolvable\n",
                doi.doi
            ));
        }
    }
    if let Some(ax) = &r.arxiv_info {
        if !ax.valid {
            out.push_str(&format!("- **arXiv** `{}` \u{2014} invalid\n", ax.arxiv_id));
        }
    }

    // Retraction details
    if let Some(ret) = &r.retraction_info {
        if ret.is_retracted {
            if let Some(rdoi) = &ret.retraction_doi {
                out.push_str(&format!(
                    "- **Retraction notice:** [{}](https://doi.org/{})\n",
                    rdoi, rdoi
                ));
            }
            if let Some(src) = &ret.retraction_source {
                out.push_str(&format!("- **Retraction source:** {}\n", src));
            }
        }
    }

    // Links
    if let Some(url) = &r.paper_url {
        out.push_str(&format!("- [Paper URL]({})\n", url));
    }
    out.push_str(&format!("- [Google Scholar]({})\n", scholar_url(&r.title)));

    // Failed DBs
    if !r.failed_dbs.is_empty() {
        out.push_str(&format!("- **Timed out:** {}\n", r.failed_dbs.join(", ")));
    }

    // Raw citation in details block
    if !r.raw_citation.is_empty() {
        out.push_str("\n<details><summary>Raw citation</summary>\n\n");
        out.push_str(&r.raw_citation);
        out.push_str("\n\n</details>\n");
    }

    out.push('\n');
}

fn export_text(papers: &[&PaperState], ref_states: &[&[RefState]]) -> String {
    let mut out = String::from("Hallucinator Results\n");
    out.push_str(&"=".repeat(60));
    out.push('\n');

    for (pi, paper) in papers.iter().enumerate() {
        let paper_refs = ref_states.get(pi).copied().unwrap_or(&[]);
        let s = &paper.stats;
        let verdict_badge = match paper.verdict {
            Some(PaperVerdict::Safe) => " [SAFE]",
            Some(PaperVerdict::Questionable) => " [?!]",
            None => "",
        };
        let title = format!("{}{}", paper.filename, verdict_badge);
        out.push_str(&format!("\n{}\n", title));
        out.push_str(&"-".repeat(title.len()));
        out.push('\n');
        out.push_str(&format!(
            "  {} total | {} verified | {} not found | {} mismatch | {} retracted | {} skipped | {:.1}% problematic\n\n",
            s.total, s.verified, s.not_found, s.author_mismatch, s.retracted, s.skipped,
            problematic_pct(s),
        ));

        for (ri, result) in paper.results.iter().enumerate() {
            if let Some(r) = result {
                let status = match r.status {
                    Status::Verified => "Verified",
                    Status::NotFound => "NOT FOUND",
                    Status::AuthorMismatch => "Author Mismatch",
                };
                let retracted = if is_retracted(r) { " [RETRACTED]" } else { "" };
                let source = r.source.as_deref().unwrap_or("-");
                out.push_str(&format!(
                    "  [{}] {} - {} ({}){}\n",
                    ri + 1,
                    r.title,
                    status,
                    source,
                    retracted,
                ));

                // Authors
                if !r.ref_authors.is_empty() {
                    out.push_str(&format!(
                        "       Authors (PDF): {}\n",
                        r.ref_authors.join(", ")
                    ));
                }
                if r.status == Status::AuthorMismatch && !r.found_authors.is_empty() {
                    out.push_str(&format!(
                        "       Authors (DB):  {}\n",
                        r.found_authors.join(", ")
                    ));
                }

                // DOI / arXiv
                if let Some(doi) = &r.doi_info {
                    let valid = if doi.valid { "valid" } else { "INVALID" };
                    out.push_str(&format!("       DOI: {} ({})\n", doi.doi, valid));
                }
                if let Some(ax) = &r.arxiv_info {
                    let valid = if ax.valid { "valid" } else { "INVALID" };
                    out.push_str(&format!("       arXiv: {} ({})\n", ax.arxiv_id, valid));
                }

                // Retraction details
                if let Some(ret) = &r.retraction_info {
                    if ret.is_retracted {
                        if let Some(rdoi) = &ret.retraction_doi {
                            out.push_str(&format!("       Retraction DOI: {}\n", rdoi));
                        }
                        if let Some(src) = &ret.retraction_source {
                            out.push_str(&format!("       Retraction source: {}\n", src));
                        }
                    }
                }

                // Paper URL
                if let Some(url) = &r.paper_url {
                    out.push_str(&format!("       URL: {}\n", url));
                }

                // Failed DBs
                if !r.failed_dbs.is_empty() {
                    out.push_str(&format!("       Timed out: {}\n", r.failed_dbs.join(", ")));
                }

                // Raw citation
                if !r.raw_citation.is_empty() {
                    out.push_str(&format!("       Citation: {}\n", r.raw_citation));
                }

                // FP override
                if let Some(fp) = paper_refs.get(ri).and_then(|rs| rs.fp_reason) {
                    out.push_str(&format!(
                        "       FP override: {}\n",
                        fp.description()
                    ));
                }
            }
        }
    }
    out
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn export_html(papers: &[&PaperState], ref_states: &[&[RefState]]) -> String {
    let mut out = String::with_capacity(16384);

    // Aggregate stats across all papers
    let mut total_stats = CheckStats::default();
    for p in papers {
        total_stats.total += p.stats.total;
        total_stats.verified += p.stats.verified;
        total_stats.not_found += p.stats.not_found;
        total_stats.author_mismatch += p.stats.author_mismatch;
        total_stats.retracted += p.stats.retracted;
        total_stats.skipped += p.stats.skipped;
    }

    out.push_str(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Hallucinator Report</title>
<style>
:root {
  --bg: #1a1a2e;
  --surface: #16213e;
  --card: #0f3460;
  --text: #e0e0e0;
  --dim: #888;
  --green: #4ecca3;
  --red: #e74c3c;
  --yellow: #f39c12;
  --dark-red: #8b0000;
  --blue: #3498db;
  --border: #2a2a4a;
}
* { box-sizing: border-box; margin: 0; padding: 0; }
body {
  font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
  background: var(--bg);
  color: var(--text);
  line-height: 1.6;
  padding: 2rem;
}
h1 { color: var(--green); margin-bottom: 1.5rem; font-size: 1.8rem; }
h2 { color: var(--text); margin: 1.5rem 0 1rem; font-size: 1.3rem; }
.stats {
  display: flex;
  gap: 1rem;
  flex-wrap: wrap;
  margin-bottom: 2rem;
}
.stat-card {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 1rem 1.5rem;
  text-align: center;
  min-width: 120px;
}
.stat-card .number {
  font-size: 2rem;
  font-weight: bold;
  display: block;
}
.stat-card .label { font-size: 0.85rem; color: var(--dim); }
.stat-card.verified .number { color: var(--green); }
.stat-card.not-found .number { color: var(--red); }
.stat-card.mismatch .number { color: var(--yellow); }
.stat-card.retracted .number { color: var(--dark-red); }
.stat-card.total .number { color: var(--text); }
.stat-card.pct .number { color: var(--red); }
details {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 8px;
  margin-bottom: 1rem;
}
summary {
  padding: 0.8rem 1rem;
  cursor: pointer;
  font-weight: 600;
}
summary:hover { background: var(--card); border-radius: 8px; }
.paper-content { padding: 0 1rem 1rem; }
.paper-stats {
  font-size: 0.85rem;
  color: var(--dim);
  margin-bottom: 1rem;
  padding: 0.5rem 0;
  border-bottom: 1px solid var(--border);
}
.ref-card {
  background: var(--card);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 1rem;
  margin-bottom: 0.75rem;
}
.ref-header {
  display: flex;
  align-items: center;
  gap: 0.75rem;
  margin-bottom: 0.5rem;
}
.ref-num {
  color: var(--dim);
  font-weight: bold;
  min-width: 2rem;
}
.ref-title { font-weight: 600; flex: 1; }
.badge {
  display: inline-block;
  padding: 0.2rem 0.6rem;
  border-radius: 4px;
  font-size: 0.75rem;
  font-weight: 700;
  text-transform: uppercase;
  white-space: nowrap;
}
.badge.verified { background: var(--green); color: #000; }
.badge.not-found { background: var(--red); color: #fff; }
.badge.mismatch { background: var(--yellow); color: #000; }
.badge.retracted { background: var(--dark-red); color: #fff; }
.ref-detail {
  font-size: 0.9rem;
  color: var(--dim);
  margin-top: 0.5rem;
}
.ref-detail dt {
  font-weight: 600;
  color: var(--text);
  margin-top: 0.4rem;
}
.ref-detail dd { margin-left: 1rem; }
.author-compare {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 0.5rem;
  margin-top: 0.3rem;
}
.author-compare div { padding: 0.3rem 0.5rem; border-radius: 4px; font-size: 0.85rem; }
.author-compare .pdf-authors { background: rgba(231, 76, 60, 0.15); }
.author-compare .db-authors { background: rgba(78, 204, 163, 0.15); }
.retraction-warning {
  background: rgba(139, 0, 0, 0.2);
  border: 1px solid var(--dark-red);
  border-radius: 4px;
  padding: 0.5rem 0.75rem;
  margin-top: 0.5rem;
  font-size: 0.9rem;
}
.citation-block {
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: 4px;
  padding: 0.5rem 0.75rem;
  margin-top: 0.5rem;
  font-size: 0.85rem;
  white-space: pre-wrap;
  word-break: break-word;
}
a { color: var(--blue); text-decoration: none; }
a:hover { text-decoration: underline; }
.links { margin-top: 0.4rem; }
.links a { margin-right: 1rem; }
footer {
  margin-top: 3rem;
  padding-top: 1rem;
  border-top: 1px solid var(--border);
  font-size: 0.85rem;
  color: var(--dim);
  text-align: center;
}
</style>
</head>
<body>
<h1>Hallucinator Report</h1>
"#,
    );

    // Overall stats cards
    out.push_str("<div class=\"stats\">\n");
    write_stat_card(&mut out, "total", total_stats.total, "Total");
    write_stat_card(&mut out, "verified", total_stats.verified, "Verified");
    write_stat_card(&mut out, "not-found", total_stats.not_found, "Not Found");
    write_stat_card(
        &mut out,
        "mismatch",
        total_stats.author_mismatch,
        "Mismatch",
    );
    write_stat_card(&mut out, "retracted", total_stats.retracted, "Retracted");
    let pct = problematic_pct(&total_stats);
    out.push_str(&format!(
        "<div class=\"stat-card pct\"><span class=\"number\">{:.1}%</span><span class=\"label\">Problematic</span></div>\n",
        pct,
    ));
    out.push_str("</div>\n");

    // Per-paper sections
    for (pi, paper) in papers.iter().enumerate() {
        let paper_refs = ref_states.get(pi).copied().unwrap_or(&[]);
        let s = &paper.stats;
        let pp = problematic_pct(s);
        let verdict_html = match paper.verdict {
            Some(PaperVerdict::Safe) => " <span class=\"badge verified\">SAFE</span>",
            Some(PaperVerdict::Questionable) => " <span class=\"badge not-found\">?!</span>",
            None => "",
        };
        out.push_str(&format!(
            "<details open>\n<summary>{}{}</summary>\n<div class=\"paper-content\">\n",
            html_escape(&paper.filename),
            verdict_html,
        ));
        out.push_str(&format!(
            "<div class=\"paper-stats\">{} total &middot; {} verified &middot; {} not found &middot; {} mismatch &middot; {} retracted &middot; {} skipped &middot; {:.1}% problematic</div>\n",
            s.total, s.verified, s.not_found, s.author_mismatch, s.retracted, s.skipped, pp,
        ));

        for (ri, result) in paper.results.iter().enumerate() {
            if let Some(r) = result {
                let fp = paper_refs.get(ri).and_then(|rs| rs.fp_reason);
                write_html_ref(&mut out, ri, r, fp);
            }
        }

        out.push_str("</div>\n</details>\n");
    }

    // Footer with timestamp
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Simple UTC date: YYYY-MM-DD HH:MM UTC
    let secs_per_day = 86400u64;
    let days = now / secs_per_day;
    let time_of_day = now % secs_per_day;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    // Days since epoch to Y-M-D (simplified)
    let (year, month, day) = days_to_ymd(days);
    out.push_str(&format!(
        "\n<footer>Generated by <strong>Hallucinator</strong> &mdash; {:04}-{:02}-{:02} {:02}:{:02} UTC</footer>\n",
        year, month, day, hours, minutes,
    ));

    out.push_str("</body>\n</html>\n");
    out
}

fn write_stat_card(out: &mut String, class: &str, value: usize, label: &str) {
    out.push_str(&format!(
        "<div class=\"stat-card {}\"><span class=\"number\">{}</span><span class=\"label\">{}</span></div>\n",
        class, value, label,
    ));
}

fn write_html_ref(
    out: &mut String,
    ri: usize,
    r: &ValidationResult,
    fp: Option<crate::model::paper::FpReason>,
) {
    let retracted = is_retracted(r);
    let (badge_class, badge_text) = if retracted {
        ("retracted", "RETRACTED")
    } else {
        match r.status {
            Status::Verified => ("verified", "Verified"),
            Status::NotFound => ("not-found", "Not Found"),
            Status::AuthorMismatch => ("mismatch", "Author Mismatch"),
        }
    };

    out.push_str("<div class=\"ref-card\">\n");
    out.push_str("<div class=\"ref-header\">\n");
    out.push_str(&format!("<span class=\"ref-num\">[{}]</span>\n", ri + 1));
    out.push_str(&format!(
        "<span class=\"ref-title\">{}</span>\n",
        html_escape(&r.title)
    ));
    out.push_str(&format!(
        "<span class=\"badge {}\">{}</span>\n",
        badge_class, badge_text
    ));
    if let Some(reason) = fp {
        out.push_str(&format!(
            "<span class=\"badge verified\" style=\"margin-left:0.3rem\">FP: {}</span>\n",
            html_escape(reason.short_label())
        ));
    }
    out.push_str("</div>\n");

    // Source
    if let Some(src) = &r.source {
        out.push_str(&format!(
            "<div class=\"ref-detail\">Verified via <strong>{}</strong></div>\n",
            html_escape(src)
        ));
    }

    // Author comparison for mismatches
    if r.status == Status::AuthorMismatch
        && (!r.ref_authors.is_empty() || !r.found_authors.is_empty())
    {
        out.push_str("<div class=\"author-compare\">\n");
        out.push_str(&format!(
            "<div class=\"pdf-authors\"><strong>PDF:</strong> {}</div>\n",
            html_escape(&r.ref_authors.join(", "))
        ));
        out.push_str(&format!(
            "<div class=\"db-authors\"><strong>DB:</strong> {}</div>\n",
            html_escape(&r.found_authors.join(", "))
        ));
        out.push_str("</div>\n");
    }

    // DOI / arXiv
    if let Some(doi) = &r.doi_info {
        if doi.valid {
            out.push_str(&format!(
                "<div class=\"ref-detail\">DOI: <a href=\"https://doi.org/{}\">{}</a></div>\n",
                html_escape(&doi.doi),
                html_escape(&doi.doi),
            ));
        } else {
            out.push_str(&format!(
                "<div class=\"ref-detail\" style=\"color:var(--red)\">DOI: {} (invalid)</div>\n",
                html_escape(&doi.doi),
            ));
        }
    }
    if let Some(ax) = &r.arxiv_info {
        if ax.valid {
            out.push_str(&format!(
                "<div class=\"ref-detail\">arXiv: <a href=\"https://arxiv.org/abs/{}\">{}</a></div>\n",
                html_escape(&ax.arxiv_id),
                html_escape(&ax.arxiv_id),
            ));
        } else {
            out.push_str(&format!(
                "<div class=\"ref-detail\" style=\"color:var(--red)\">arXiv: {} (invalid)</div>\n",
                html_escape(&ax.arxiv_id),
            ));
        }
    }

    // Retraction warning
    if let Some(ret) = &r.retraction_info {
        if ret.is_retracted {
            out.push_str("<div class=\"retraction-warning\">\u{26a0}\u{fe0f} <strong>This paper has been retracted.</strong>");
            if let Some(rdoi) = &ret.retraction_doi {
                out.push_str(&format!(
                    " <a href=\"https://doi.org/{}\">Retraction notice</a>",
                    html_escape(rdoi),
                ));
            }
            if let Some(src) = &ret.retraction_source {
                out.push_str(&format!(" ({})", html_escape(src)));
            }
            out.push_str("</div>\n");
        }
    }

    // Links
    out.push_str("<div class=\"links\">");
    if let Some(url) = &r.paper_url {
        out.push_str(&format!("<a href=\"{}\">Paper URL</a>", html_escape(url)));
    }
    out.push_str(&format!(
        "<a href=\"{}\">Google Scholar</a>",
        html_escape(&scholar_url(&r.title))
    ));
    out.push_str("</div>\n");

    // Failed DBs
    if !r.failed_dbs.is_empty() {
        out.push_str(&format!(
            "<div class=\"ref-detail\">Timed out: {}</div>\n",
            html_escape(&r.failed_dbs.join(", "))
        ));
    }

    // Raw citation
    if !r.raw_citation.is_empty() {
        out.push_str("<details><summary>Raw citation</summary>\n");
        out.push_str(&format!(
            "<div class=\"citation-block\">{}</div>\n",
            html_escape(&r.raw_citation)
        ));
        out.push_str("</details>\n");
    }

    out.push_str("</div>\n");
}

/// Convert days since Unix epoch to (year, month, day).
fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Simplified civil calendar conversion
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}
