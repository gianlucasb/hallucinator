use std::io::Write;
use std::path::Path;

use hallucinator_core::{CheckStats, DbStatus, Status, ValidationResult};

use crate::types::{ExportFormat, FpReason, PaperVerdict, ReportPaper, ReportRef};

/// Export results for a set of papers to the given path.
///
/// `ref_states` is a parallel slice to `papers` — `ref_states[i]` are the ReportRefs
/// for `papers[i]`. This is used to include FP reason overrides in the output.
pub fn export_results(
    papers: &[ReportPaper<'_>],
    ref_states: &[&[ReportRef]],
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

/// Whether a verified reference has an invalid DOI or arXiv ID.
fn has_doi_arxiv_issue(r: &ValidationResult) -> bool {
    r.status == Status::Verified
        && (r.doi_info.as_ref().is_some_and(|d| !d.valid)
            || r.arxiv_info.as_ref().is_some_and(|a| !a.valid))
}

/// Sort bucket for export ordering.
///
/// 0 = Retracted, 1 = Not Found, 2 = Author Mismatch,
/// 3 = DOI/arXiv issues (verified but invalid DOI/arXiv),
/// 4 = FP-overridden, 5 = Clean verified, 6 = Skipped.
fn export_sort_key(r: &ValidationResult, fp: Option<FpReason>) -> u8 {
    if fp.is_some() {
        return 4;
    }
    if is_retracted(r) {
        return 0;
    }
    match r.status {
        Status::NotFound => 1,
        Status::AuthorMismatch => 2,
        Status::Verified => {
            if has_doi_arxiv_issue(r) {
                3
            } else {
                5
            }
        }
    }
}

/// Entry for the sorted reference index used across all export formats.
struct SortedRef<'a> {
    /// Index into paper.results
    ri: usize,
    result: &'a ValidationResult,
    fp: Option<FpReason>,
    ref_num: usize,
}

/// Build a sorted list of refs for export: retracted → not found → mismatch →
/// DOI/arXiv issues → FP-overridden → clean verified, with original ref number
/// as tiebreaker within each bucket.
fn build_sorted_refs<'a>(paper: &ReportPaper<'a>, paper_refs: &[ReportRef]) -> Vec<SortedRef<'a>> {
    let mut entries: Vec<SortedRef<'a>> = Vec::new();
    for (ri, result) in paper.results.iter().enumerate() {
        if let Some(r) = result {
            let fp = paper_refs.get(ri).and_then(|rs| rs.fp_reason);
            let ref_num = paper_refs.get(ri).map(|rs| rs.index + 1).unwrap_or(ri + 1);
            entries.push(SortedRef {
                ri,
                result: r,
                fp,
                ref_num,
            });
        }
    }
    entries.sort_by_key(|e| (export_sort_key(e.result, e.fp), e.ref_num));
    entries
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

/// Compute stats adjusted for false-positive overrides.
///
/// References marked as FP are moved out of their original bucket
/// (not_found / author_mismatch / retracted) and into `verified`,
/// since the user has vouched for them.
fn adjusted_stats(paper: &ReportPaper<'_>, refs: &[ReportRef]) -> CheckStats {
    let mut s = paper.stats.clone();
    for (ri, result) in paper.results.iter().enumerate() {
        if let Some(r) = result
            && refs.get(ri).and_then(|rs| rs.fp_reason).is_some()
        {
            match r.status {
                Status::NotFound => {
                    s.not_found = s.not_found.saturating_sub(1);
                    s.verified += 1;
                }
                Status::AuthorMismatch => {
                    s.author_mismatch = s.author_mismatch.saturating_sub(1);
                    s.verified += 1;
                }
                Status::Verified => {}
            }
            if r.retraction_info.as_ref().is_some_and(|ri| ri.is_retracted) {
                s.retracted = s.retracted.saturating_sub(1);
            }
        }
    }
    s
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

pub fn export_json(papers: &[ReportPaper<'_>], ref_states: &[&[ReportRef]]) -> String {
    let mut out = String::from("[\n");
    for (pi, paper) in papers.iter().enumerate() {
        let paper_refs = ref_states.get(pi).copied().unwrap_or(&[]);
        let s = adjusted_stats(paper, paper_refs);
        let verdict_json = match paper.verdict {
            Some(_) => json_str(verdict_str(paper.verdict)),
            None => "null".to_string(),
        };
        out.push_str(&format!(
            "  {{\n    \"filename\": {},\n    \"verdict\": {},\n    \"stats\": {{\n      \"total\": {},\n      \"verified\": {},\n      \"not_found\": {},\n      \"author_mismatch\": {},\n      \"retracted\": {},\n      \"skipped\": {},\n      \"problematic_pct\": {:.1}\n    }},\n    \"references\": [\n",
            json_str(paper.filename),
            verdict_json,
            s.total, s.verified, s.not_found, s.author_mismatch, s.retracted, s.skipped,
            problematic_pct(&s),
        ));

        // Collect all entries to write: sorted results + skipped refs
        let mut entries: Vec<String> = Vec::new();
        let sorted = build_sorted_refs(paper, paper_refs);

        for sref in &sorted {
            let r = sref.result;
            let ri = sref.ri;
            let fp_json = sref
                .fp
                .map(|fp| json_str(fp.as_str()))
                .unwrap_or_else(|| "null".to_string());
            let orig_num = sref.ref_num;
            let effective = if sref.fp.is_some() {
                "\"verified\""
            } else {
                match r.status {
                    Status::Verified => "\"verified\"",
                    Status::NotFound => "\"not_found\"",
                    Status::AuthorMismatch => "\"author_mismatch\"",
                }
            };
            let mut entry = String::new();
            entry.push_str("      {\n");
            entry.push_str(&format!("        \"index\": {},\n", ri));
            entry.push_str(&format!("        \"original_number\": {},\n", orig_num));
            entry.push_str(&format!("        \"title\": {},\n", json_str(&r.title)));
            entry.push_str(&format!(
                "        \"raw_citation\": {},\n",
                json_str(&r.raw_citation)
            ));
            entry.push_str(&format!(
                "        \"status\": {},\n",
                json_str(status_str(&r.status))
            ));
            entry.push_str(&format!("        \"effective_status\": {},\n", effective));
            entry.push_str(&format!("        \"fp_reason\": {},\n", fp_json));
            entry.push_str(&format!(
                "        \"source\": {},\n",
                json_opt_str(&r.source)
            ));
            entry.push_str(&format!(
                "        \"ref_authors\": {},\n",
                json_str_array(&r.ref_authors)
            ));
            entry.push_str(&format!(
                "        \"found_authors\": {},\n",
                json_str_array(&r.found_authors)
            ));
            entry.push_str(&format!(
                "        \"paper_url\": {},\n",
                json_opt_str(&r.paper_url)
            ));
            entry.push_str(&format!(
                "        \"failed_dbs\": {},\n",
                json_str_array(&r.failed_dbs)
            ));

            // DOI info
            if let Some(doi) = &r.doi_info {
                entry.push_str(&format!(
                    "        \"doi_info\": {{\"doi\": {}, \"valid\": {}, \"title\": {}}},\n",
                    json_str(&doi.doi),
                    doi.valid,
                    json_opt_str(&doi.title)
                ));
            } else {
                entry.push_str("        \"doi_info\": null,\n");
            }

            // arXiv info
            if let Some(ax) = &r.arxiv_info {
                entry.push_str(&format!(
                    "        \"arxiv_info\": {{\"arxiv_id\": {}, \"valid\": {}, \"title\": {}}},\n",
                    json_str(&ax.arxiv_id),
                    ax.valid,
                    json_opt_str(&ax.title)
                ));
            } else {
                entry.push_str("        \"arxiv_info\": null,\n");
            }

            // Retraction info
            if let Some(ret) = &r.retraction_info {
                entry.push_str(&format!(
                    "        \"retraction_info\": {{\"is_retracted\": {}, \"retraction_doi\": {}, \"retraction_source\": {}}},\n",
                    ret.is_retracted,
                    json_opt_str(&ret.retraction_doi),
                    json_opt_str(&ret.retraction_source)
                ));
            } else {
                entry.push_str("        \"retraction_info\": null,\n");
            }

            // Per-DB results
            entry.push_str("        \"db_results\": [");
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
                entry.push_str(&format!(
                    "{{\"db\": {}, \"status\": {}, \"elapsed_ms\": {}, \"authors\": {}, \"url\": {}}}",
                    json_str(&db.db_name),
                    json_str(db_status),
                    elapsed_ms,
                    json_str_array(&db.found_authors),
                    json_opt_str(&db.paper_url),
                ));
                if di + 1 < r.db_results.len() {
                    entry.push_str(", ");
                }
            }
            entry.push_str("]\n");
            entry.push_str("      }");
            entries.push(entry);
        }

        // Add skipped refs from ref_states
        for rs in paper_refs {
            if let Some(skip) = &rs.skip_info {
                let mut entry = String::new();
                entry.push_str("      {\n");
                entry.push_str(&format!("        \"index\": {},\n", rs.index));
                entry.push_str(&format!("        \"original_number\": {},\n", rs.index + 1));
                entry.push_str(&format!("        \"title\": {},\n", json_str(&rs.title)));
                entry.push_str("        \"raw_citation\": \"\",\n");
                entry.push_str("        \"status\": \"skipped\",\n");
                entry.push_str("        \"effective_status\": \"skipped\",\n");
                entry.push_str(&format!(
                    "        \"skip_reason\": {},\n",
                    json_str(&skip.reason)
                ));
                entry.push_str("        \"fp_reason\": null,\n");
                entry.push_str("        \"source\": null,\n");
                entry.push_str("        \"ref_authors\": [],\n");
                entry.push_str("        \"found_authors\": [],\n");
                entry.push_str("        \"paper_url\": null,\n");
                entry.push_str("        \"failed_dbs\": [],\n");
                entry.push_str("        \"doi_info\": null,\n");
                entry.push_str("        \"arxiv_info\": null,\n");
                entry.push_str("        \"retraction_info\": null,\n");
                entry.push_str("        \"db_results\": []\n");
                entry.push_str("      }");
                entries.push(entry);
            }
        }

        for (ei, entry) in entries.iter().enumerate() {
            out.push_str(entry);
            if ei + 1 < entries.len() {
                out.push(',');
            }
            out.push('\n');
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

fn export_csv(papers: &[ReportPaper<'_>], ref_states: &[&[ReportRef]]) -> String {
    let mut out = String::from(
        "Filename,Verdict,Ref#,Title,Status,EffectiveStatus,FpReason,Source,Retracted,Authors,FoundAuthors,PaperURL,DOI,ArxivID,FailedDBs\n",
    );
    for (pi, paper) in papers.iter().enumerate() {
        let verdict = verdict_str(paper.verdict);
        let paper_refs = ref_states.get(pi).copied().unwrap_or(&[]);
        let sorted = build_sorted_refs(paper, paper_refs);
        for sref in &sorted {
            let r = sref.result;
            let retracted = is_retracted(r);
            let fp = sref.fp.map(|fp| fp.as_str()).unwrap_or("");
            let effective = if sref.fp.is_some() {
                "verified"
            } else {
                status_str(&r.status)
            };
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
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
                csv_escape(paper.filename),
                csv_escape(verdict),
                sref.ref_num,
                csv_escape(&r.title),
                status_str(&r.status),
                effective,
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
        // Add skipped refs
        for rs in paper_refs {
            if let Some(skip) = &rs.skip_info {
                out.push_str(&format!(
                    "{},{},{},{},skipped,skipped,{},,,,,,,,\n",
                    csv_escape(paper.filename),
                    csv_escape(verdict),
                    rs.index + 1,
                    csv_escape(&rs.title),
                    csv_escape(&skip.reason),
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

fn export_markdown(papers: &[ReportPaper<'_>], ref_states: &[&[ReportRef]]) -> String {
    let mut out = String::from("# Hallucinator Results\n\n");

    for (pi, paper) in papers.iter().enumerate() {
        let paper_refs = ref_states.get(pi).copied().unwrap_or(&[]);
        let s = adjusted_stats(paper, paper_refs);
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
            problematic_pct(&s),
        ));

        // Build sorted refs and split into buckets
        let sorted = build_sorted_refs(paper, paper_refs);
        let mut problems: Vec<&SortedRef> = Vec::new();
        let mut doi_arxiv_issues: Vec<&SortedRef> = Vec::new();
        let mut fp_overrides: Vec<&SortedRef> = Vec::new();
        let mut verified: Vec<&SortedRef> = Vec::new();
        for sref in &sorted {
            match export_sort_key(sref.result, sref.fp) {
                0..=2 => problems.push(sref),
                3 => doi_arxiv_issues.push(sref),
                4 => fp_overrides.push(sref),
                _ => verified.push(sref),
            }
        }

        if !problems.is_empty() {
            out.push_str("### Problematic References\n\n");
            for sref in &problems {
                write_md_ref(&mut out, sref.ref_num, sref.result);
            }
        }

        if !doi_arxiv_issues.is_empty() {
            out.push_str("### DOI/arXiv Issues\n\n");
            for sref in &doi_arxiv_issues {
                write_md_ref(&mut out, sref.ref_num, sref.result);
            }
        }

        if !fp_overrides.is_empty() {
            out.push_str("### User-Verified References (FP Overrides)\n\n");
            for sref in &fp_overrides {
                let fp = sref.fp.unwrap();
                out.push_str(&format!(
                    "**[{}]** {} \u{2014} \u{2713} Verified (FP: {})\n\n",
                    sref.ref_num,
                    md_escape(&sref.result.title),
                    fp.description(),
                ));
                if let Some(src) = &sref.result.source {
                    out.push_str(&format!("- **Source:** {}\n", src));
                }
                if let Some(url) = &sref.result.paper_url {
                    out.push_str(&format!("- [Paper URL]({})\n", url));
                }
                out.push_str(&format!(
                    "- [Google Scholar]({})\n\n",
                    scholar_url(&sref.result.title)
                ));
            }
        }

        if !verified.is_empty() {
            out.push_str("### Verified References\n\n");
            out.push_str("| # | Title | Source | URL |\n");
            out.push_str("|---|-------|--------|-----|\n");
            for sref in &verified {
                let r = sref.result;
                let source = r.source.as_deref().unwrap_or("\u{2014}");
                let url = r
                    .paper_url
                    .as_ref()
                    .map(|u| format!("[link]({})", u))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "| {} | {} | {} | {} |\n",
                    sref.ref_num,
                    md_escape(&r.title),
                    source,
                    url,
                ));
            }
            out.push('\n');
        }

        // Skipped references
        let skipped: Vec<&ReportRef> = paper_refs
            .iter()
            .filter(|rs| rs.skip_info.is_some())
            .collect();
        if !skipped.is_empty() {
            out.push_str("### Skipped References\n\n");
            out.push_str("| # | Title | Reason |\n");
            out.push_str("|---|-------|--------|\n");
            for rs in &skipped {
                let reason = match rs.skip_info.as_ref().map(|s| s.reason.as_str()) {
                    Some("url_only") => "URL-only",
                    Some("short_title") => "Short title",
                    Some("no_title") => "No title",
                    Some(other) => other,
                    None => "",
                };
                let title = if rs.title.is_empty() {
                    "\u{2014}"
                } else {
                    &rs.title
                };
                out.push_str(&format!(
                    "| {} | {} | {} |\n",
                    rs.index + 1,
                    md_escape(title),
                    reason,
                ));
            }
            out.push('\n');
        }

        out.push_str("---\n\n");
    }
    out
}

fn write_md_ref(out: &mut String, ref_num: usize, r: &ValidationResult) {
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
        ref_num,
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
        } else {
            out.push_str("- **DB authors:** *(no authors returned)*\n");
        }
        if let Some(src) = &r.source {
            out.push_str(&format!("- **Source:** {}\n", src));
        }
    }

    // DOI/arXiv issues
    if let Some(doi) = &r.doi_info
        && !doi.valid
    {
        out.push_str(&format!(
            "- **DOI** `{}` \u{2014} invalid/unresolvable\n",
            doi.doi
        ));
    }
    if let Some(ax) = &r.arxiv_info
        && !ax.valid
    {
        out.push_str(&format!("- **arXiv** `{}` \u{2014} invalid\n", ax.arxiv_id));
    }

    // Retraction details
    if let Some(ret) = &r.retraction_info
        && ret.is_retracted
    {
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

fn export_text(papers: &[ReportPaper<'_>], ref_states: &[&[ReportRef]]) -> String {
    let mut out = String::from("Hallucinator Results\n");
    out.push_str(&"=".repeat(60));
    out.push('\n');

    for (pi, paper) in papers.iter().enumerate() {
        let paper_refs = ref_states.get(pi).copied().unwrap_or(&[]);
        let s = adjusted_stats(paper, paper_refs);
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
            problematic_pct(&s),
        ));

        let sorted = build_sorted_refs(paper, paper_refs);
        for sref in &sorted {
            let r = sref.result;
            let status = if let Some(fp) = sref.fp {
                format!("Verified (FP: {})", fp.short_label())
            } else if is_retracted(r) {
                "RETRACTED".to_string()
            } else {
                match r.status {
                    Status::Verified => "Verified".to_string(),
                    Status::NotFound => "NOT FOUND".to_string(),
                    Status::AuthorMismatch => "Author Mismatch".to_string(),
                }
            };
            // When FP is set, status already shows "Verified (FP: ...)",
            // but still tag retracted papers so the info isn't lost.
            let retracted_tag = if is_retracted(r) && sref.fp.is_some() {
                " [RETRACTED]"
            } else {
                ""
            };
            let source = r.source.as_deref().unwrap_or("-");
            out.push_str(&format!(
                "  [{}] {} - {} ({}){}\n",
                sref.ref_num, r.title, status, source, retracted_tag,
            ));

            // Authors
            if !r.ref_authors.is_empty() {
                out.push_str(&format!(
                    "       Authors (PDF): {}\n",
                    r.ref_authors.join(", ")
                ));
            }
            if r.status == Status::AuthorMismatch {
                if !r.found_authors.is_empty() {
                    out.push_str(&format!(
                        "       Authors (DB):  {}\n",
                        r.found_authors.join(", ")
                    ));
                } else {
                    out.push_str("       Authors (DB):  (no authors returned)\n");
                }
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
            if let Some(ret) = &r.retraction_info
                && ret.is_retracted
            {
                if let Some(rdoi) = &ret.retraction_doi {
                    out.push_str(&format!("       Retraction DOI: {}\n", rdoi));
                }
                if let Some(src) = &ret.retraction_source {
                    out.push_str(&format!("       Retraction source: {}\n", src));
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
        }

        // Skipped references
        let skipped: Vec<&ReportRef> = paper_refs
            .iter()
            .filter(|rs| rs.skip_info.is_some())
            .collect();
        if !skipped.is_empty() {
            out.push_str("\n  Skipped references:\n");
            for rs in &skipped {
                let reason = match rs.skip_info.as_ref().map(|s| s.reason.as_str()) {
                    Some("url_only") => "URL-only",
                    Some("short_title") => "Short title",
                    Some("no_title") => "No title",
                    Some(other) => other,
                    None => "",
                };
                let title = if rs.title.is_empty() {
                    "(no title)"
                } else {
                    &rs.title
                };
                out.push_str(&format!("  [{}] {} - {}\n", rs.index + 1, title, reason));
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

fn export_html(papers: &[ReportPaper<'_>], ref_states: &[&[ReportRef]]) -> String {
    let mut out = String::with_capacity(16384);

    // Aggregate adjusted stats across all papers
    let mut total_stats = CheckStats::default();
    for (pi, p) in papers.iter().enumerate() {
        let pr = ref_states.get(pi).copied().unwrap_or(&[]);
        let adj = adjusted_stats(p, pr);
        total_stats.total += adj.total;
        total_stats.verified += adj.verified;
        total_stats.not_found += adj.not_found;
        total_stats.author_mismatch += adj.author_mismatch;
        total_stats.retracted += adj.retracted;
        total_stats.skipped += adj.skipped;
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
        let s = adjusted_stats(paper, paper_refs);
        let pp = problematic_pct(&s);
        let verdict_html = match paper.verdict {
            Some(PaperVerdict::Safe) => " <span class=\"badge verified\">SAFE</span>",
            Some(PaperVerdict::Questionable) => " <span class=\"badge not-found\">?!</span>",
            None => "",
        };
        out.push_str(&format!(
            "<details open>\n<summary>{}{}</summary>\n<div class=\"paper-content\">\n",
            html_escape(paper.filename),
            verdict_html,
        ));
        out.push_str(&format!(
            "<div class=\"paper-stats\">{} total &middot; {} verified &middot; {} not found &middot; {} mismatch &middot; {} retracted &middot; {} skipped &middot; {:.1}% problematic</div>\n",
            s.total, s.verified, s.not_found, s.author_mismatch, s.retracted, s.skipped, pp,
        ));

        let sorted = build_sorted_refs(paper, paper_refs);
        for sref in &sorted {
            write_html_ref(&mut out, sref.ref_num, sref.result, sref.fp);
        }

        // Skipped refs
        let skipped: Vec<&ReportRef> = paper_refs
            .iter()
            .filter(|rs| rs.skip_info.is_some())
            .collect();
        if !skipped.is_empty() {
            out.push_str(
                "<h3 style=\"color:var(--dim);margin-top:1.5rem\">Skipped References</h3>\n",
            );
            for rs in &skipped {
                let reason = match rs.skip_info.as_ref().map(|s| s.reason.as_str()) {
                    Some("url_only") => "URL-only",
                    Some("short_title") => "Short title",
                    Some("no_title") => "No title",
                    Some(other) => other,
                    None => "",
                };
                let title = if rs.title.is_empty() {
                    "\u{2014}"
                } else {
                    &rs.title
                };
                out.push_str(&format!(
                    "<div class=\"ref-card\" style=\"opacity:0.5\"><div class=\"ref-header\"><span class=\"ref-num\">[{}]</span><span class=\"ref-title\">{}</span><span class=\"badge\" style=\"background:var(--dim);color:#fff\">{}</span></div></div>\n",
                    rs.index + 1,
                    html_escape(title),
                    html_escape(reason),
                ));
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

fn write_html_ref(out: &mut String, ref_num: usize, r: &ValidationResult, fp: Option<FpReason>) {
    let retracted = is_retracted(r);
    let (badge_class, badge_text) = if fp.is_some() {
        ("verified", "Verified")
    } else if retracted {
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
    out.push_str(&format!("<span class=\"ref-num\">[{}]</span>\n", ref_num));
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
    if r.status == Status::AuthorMismatch {
        out.push_str("<div class=\"author-compare\">\n");
        out.push_str(&format!(
            "<div class=\"pdf-authors\"><strong>PDF:</strong> {}</div>\n",
            html_escape(&r.ref_authors.join(", "))
        ));
        let db_authors_text = if r.found_authors.is_empty() {
            "<em>(no authors returned)</em>".to_string()
        } else {
            html_escape(&r.found_authors.join(", "))
        };
        out.push_str(&format!(
            "<div class=\"db-authors\"><strong>DB:</strong> {}</div>\n",
            db_authors_text
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
    if let Some(ret) = &r.retraction_info
        && ret.is_retracted
    {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    use crate::types::{ExportFormat, FpReason, PaperVerdict, ReportPaper, ReportRef, SkipInfo};
    use hallucinator_core::{CheckStats, DoiInfo, RetractionInfo, Status, ValidationResult};

    // ── helpers ──────────────────────────────────────────────────────

    fn make_result(title: &str, status: Status) -> ValidationResult {
        ValidationResult {
            title: title.to_string(),
            raw_citation: String::new(),
            ref_authors: vec![],
            status,
            source: None,
            found_authors: vec![],
            paper_url: None,
            failed_dbs: vec![],
            db_results: vec![],
            doi_info: None,
            arxiv_info: None,
            retraction_info: None,
        }
    }

    fn make_paper<'a>(
        filename: &'a str,
        stats: &'a CheckStats,
        results: &'a [Option<ValidationResult>],
    ) -> ReportPaper<'a> {
        ReportPaper {
            filename,
            stats,
            results,
            verdict: None,
        }
    }

    fn make_ref(index: usize, title: &str) -> ReportRef {
        ReportRef {
            index,
            title: title.to_string(),
            skip_info: None,
            fp_reason: None,
        }
    }

    fn make_ref_fp(index: usize, title: &str, fp: FpReason) -> ReportRef {
        ReportRef {
            index,
            title: title.to_string(),
            skip_info: None,
            fp_reason: Some(fp),
        }
    }

    fn make_ref_skipped(index: usize, title: &str, reason: &str) -> ReportRef {
        ReportRef {
            index,
            title: title.to_string(),
            skip_info: Some(SkipInfo {
                reason: reason.to_string(),
            }),
            fp_reason: None,
        }
    }

    fn make_retracted(title: &str) -> ValidationResult {
        let mut r = make_result(title, Status::Verified);
        r.retraction_info = Some(RetractionInfo {
            is_retracted: true,
            retraction_doi: Some("10.1234/retracted".to_string()),
            retraction_source: Some("CrossRef".to_string()),
        });
        r
    }

    // ── P1: pure helper functions ───────────────────────────────────

    #[test]
    fn test_json_escape_special_chars() {
        assert_eq!(json_escape(r#"He said "hi""#), r#"He said \"hi\""#);
        assert_eq!(json_escape("back\\slash"), "back\\\\slash");
        assert_eq!(json_escape("line\nbreak"), "line\\nbreak");
        assert_eq!(json_escape("carriage\rreturn"), "carriage\\rreturn");
        assert_eq!(json_escape("tab\there"), "tab\\there");
    }

    #[test]
    fn test_json_escape_control_chars() {
        // NUL, BEL, and other chars < 0x20 that aren't \n \r \t
        assert_eq!(json_escape("\x00"), "\\u0000");
        assert_eq!(json_escape("\x07"), "\\u0007");
        assert_eq!(json_escape("\x1f"), "\\u001f");
    }

    #[test]
    fn test_json_escape_passthrough() {
        assert_eq!(json_escape("hello world"), "hello world");
        assert_eq!(json_escape("café"), "café");
        assert_eq!(json_escape("日本語"), "日本語");
    }

    #[test]
    fn test_csv_escape_quotes() {
        assert_eq!(csv_escape(r#"He said "hi""#), r#""He said ""hi""""#);
    }

    #[test]
    fn test_csv_escape_comma() {
        assert_eq!(csv_escape("a,b"), "\"a,b\"");
    }

    #[test]
    fn test_csv_escape_newline() {
        assert_eq!(csv_escape("a\nb"), "\"a\nb\"");
    }

    #[test]
    fn test_csv_escape_clean() {
        assert_eq!(csv_escape("hello"), "hello");
    }

    #[test]
    fn test_html_escape() {
        assert_eq!(
            html_escape(r#"<script>&"x"</script>"#),
            "&lt;script&gt;&amp;&quot;x&quot;&lt;/script&gt;",
        );
    }

    #[test]
    fn test_md_escape_pipe() {
        assert_eq!(md_escape("A | B"), "A \\| B");
    }

    #[test]
    fn test_days_to_ymd_epoch() {
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
    }

    #[test]
    fn test_days_to_ymd_known_dates() {
        // 2000-01-01 is day 10957
        assert_eq!(days_to_ymd(10957), (2000, 1, 1));
        // 2024-02-15 is day 19768
        assert_eq!(days_to_ymd(19768), (2024, 2, 15));
    }

    #[test]
    fn test_days_to_ymd_leap_year() {
        // 2000-02-29 (leap year) = day 10957 + 31 (Jan) + 28 (Feb 1..28) = 11016
        assert_eq!(days_to_ymd(11016), (2000, 2, 29));
        // 2000-03-01 = day 11017
        assert_eq!(days_to_ymd(11017), (2000, 3, 1));
    }

    // ── P2: stats & sorting logic ───────────────────────────────────

    #[test]
    fn test_problematic_pct_zero_checked() {
        let stats = CheckStats {
            total: 5,
            verified: 0,
            not_found: 0,
            author_mismatch: 0,
            retracted: 0,
            skipped: 5,
        };
        assert_eq!(problematic_pct(&stats), 0.0);
    }

    #[test]
    fn test_problematic_pct_normal() {
        let stats = CheckStats {
            total: 10,
            verified: 8,
            not_found: 2,
            author_mismatch: 0,
            retracted: 0,
            skipped: 0,
        };
        assert!((problematic_pct(&stats) - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_adjusted_stats_fp_not_found() {
        let stats = CheckStats {
            total: 3,
            verified: 1,
            not_found: 2,
            author_mismatch: 0,
            retracted: 0,
            skipped: 0,
        };
        let results: Vec<Option<ValidationResult>> = vec![
            Some(make_result("A", Status::Verified)),
            Some(make_result("B", Status::NotFound)),
            Some(make_result("C", Status::NotFound)),
        ];
        let paper = make_paper("test.pdf", &stats, &results);
        let refs = vec![
            make_ref(0, "A"),
            make_ref_fp(1, "B", FpReason::BrokenParse),
            make_ref(2, "C"),
        ];
        let adj = adjusted_stats(&paper, &refs);
        assert_eq!(adj.not_found, 1);
        assert_eq!(adj.verified, 2);
    }

    #[test]
    fn test_adjusted_stats_fp_mismatch() {
        let stats = CheckStats {
            total: 2,
            verified: 1,
            not_found: 0,
            author_mismatch: 1,
            retracted: 0,
            skipped: 0,
        };
        let results: Vec<Option<ValidationResult>> = vec![
            Some(make_result("A", Status::Verified)),
            Some(make_result("B", Status::AuthorMismatch)),
        ];
        let paper = make_paper("test.pdf", &stats, &results);
        let refs = vec![
            make_ref(0, "A"),
            make_ref_fp(1, "B", FpReason::ExistsElsewhere),
        ];
        let adj = adjusted_stats(&paper, &refs);
        assert_eq!(adj.author_mismatch, 0);
        assert_eq!(adj.verified, 2);
    }

    #[test]
    fn test_adjusted_stats_fp_verified_noop() {
        let stats = CheckStats {
            total: 1,
            verified: 1,
            not_found: 0,
            author_mismatch: 0,
            retracted: 0,
            skipped: 0,
        };
        let results: Vec<Option<ValidationResult>> = vec![Some(make_result("A", Status::Verified))];
        let paper = make_paper("test.pdf", &stats, &results);
        let refs = vec![make_ref_fp(0, "A", FpReason::KnownGood)];
        let adj = adjusted_stats(&paper, &refs);
        // Verified → Verified is a no-op
        assert_eq!(adj.verified, 1);
        assert_eq!(adj.not_found, 0);
    }

    #[test]
    fn test_adjusted_stats_fp_retracted() {
        let stats = CheckStats {
            total: 1,
            verified: 1,
            not_found: 0,
            author_mismatch: 0,
            retracted: 1,
            skipped: 0,
        };
        let results: Vec<Option<ValidationResult>> = vec![Some(make_retracted("A"))];
        let paper = make_paper("test.pdf", &stats, &results);
        let refs = vec![make_ref_fp(0, "A", FpReason::KnownGood)];
        let adj = adjusted_stats(&paper, &refs);
        assert_eq!(adj.retracted, 0);
    }

    #[test]
    fn test_build_sorted_refs_order() {
        // Build refs in scrambled order: verified, not_found, retracted, mismatch, FP, doi_issue
        let mut retracted = make_retracted("Retracted Paper");
        retracted.status = Status::NotFound; // retracted + not_found
        let mut doi_issue = make_result("DOI Issue", Status::Verified);
        doi_issue.doi_info = Some(DoiInfo {
            doi: "10.bad".into(),
            valid: false,
            title: None,
        });

        let results: Vec<Option<ValidationResult>> = vec![
            Some(make_result("Verified", Status::Verified)), // bucket 5
            Some(make_result("Not Found", Status::NotFound)), // bucket 1
            Some(retracted),                                 // bucket 0
            Some(make_result("Mismatch", Status::AuthorMismatch)), // bucket 2
            Some(make_result("FP Paper", Status::NotFound)), // bucket 4 (FP)
            Some(doi_issue),                                 // bucket 3
        ];
        let stats = CheckStats::default();
        let paper = make_paper("test.pdf", &stats, &results);
        let refs = vec![
            make_ref(0, "Verified"),
            make_ref(1, "Not Found"),
            make_ref(2, "Retracted Paper"),
            make_ref(3, "Mismatch"),
            make_ref_fp(4, "FP Paper", FpReason::BrokenParse),
            make_ref(5, "DOI Issue"),
        ];
        let sorted = build_sorted_refs(&paper, &refs);
        let titles: Vec<&str> = sorted.iter().map(|s| s.result.title.as_str()).collect();
        assert_eq!(
            titles,
            vec![
                "Retracted Paper", // 0: retracted
                "Not Found",       // 1: not_found
                "Mismatch",        // 2: author_mismatch
                "DOI Issue",       // 3: doi/arxiv issue
                "FP Paper",        // 4: FP override
                "Verified",        // 5: clean verified
            ]
        );
    }

    #[test]
    fn test_build_sorted_refs_tiebreak() {
        // Two not_found refs: should be ordered by ref_num (index+1)
        let results: Vec<Option<ValidationResult>> = vec![
            Some(make_result("Second", Status::NotFound)),
            Some(make_result("First", Status::NotFound)),
        ];
        let stats = CheckStats::default();
        let paper = make_paper("test.pdf", &stats, &results);
        let refs = vec![
            make_ref(0, "Second"), // ref_num = 1
            make_ref(1, "First"),  // ref_num = 2
        ];
        let sorted = build_sorted_refs(&paper, &refs);
        assert_eq!(sorted[0].ref_num, 1);
        assert_eq!(sorted[1].ref_num, 2);
    }

    #[test]
    fn test_export_sort_key_fp_precedence() {
        let nf = make_result("A", Status::NotFound);
        let mm = make_result("B", Status::AuthorMismatch);
        let v = make_result("C", Status::Verified);
        // All should return 4 when FP is set, regardless of status
        assert_eq!(export_sort_key(&nf, Some(FpReason::BrokenParse)), 4);
        assert_eq!(export_sort_key(&mm, Some(FpReason::ExistsElsewhere)), 4);
        assert_eq!(export_sort_key(&v, Some(FpReason::KnownGood)), 4);
    }

    // ── P3: enum roundtrips ─────────────────────────────────────────

    #[test]
    fn test_fp_reason_cycle() {
        let mut current = None;
        let expected = [
            Some(FpReason::BrokenParse),
            Some(FpReason::ExistsElsewhere),
            Some(FpReason::AllTimedOut),
            Some(FpReason::KnownGood),
            Some(FpReason::NonAcademic),
            None,
        ];
        for exp in &expected {
            current = FpReason::cycle(current);
            assert_eq!(current, *exp);
        }
    }

    #[test]
    fn test_fp_reason_str_roundtrip() {
        let all = [
            FpReason::BrokenParse,
            FpReason::ExistsElsewhere,
            FpReason::AllTimedOut,
            FpReason::KnownGood,
            FpReason::NonAcademic,
        ];
        for fp in &all {
            assert_eq!(FpReason::from_str(fp.as_str()), Ok(*fp));
        }
    }

    #[test]
    fn test_fp_reason_from_str_unknown() {
        assert!(FpReason::from_str("garbage").is_err());
        assert!(FpReason::from_str("").is_err());
    }

    #[test]
    fn test_paper_verdict_cycle() {
        assert_eq!(PaperVerdict::cycle(None), Some(PaperVerdict::Safe));
        assert_eq!(
            PaperVerdict::cycle(Some(PaperVerdict::Safe)),
            Some(PaperVerdict::Questionable)
        );
        assert_eq!(PaperVerdict::cycle(Some(PaperVerdict::Questionable)), None);
    }

    #[test]
    fn test_export_format_all() {
        let all = ExportFormat::all();
        assert_eq!(all.len(), 5);
        for fmt in all {
            assert!(!fmt.label().is_empty());
            assert!(!fmt.extension().is_empty());
        }
    }

    // ── P4: format integration (smoke tests) ────────────────────────

    #[test]
    fn test_json_empty_papers() {
        let out = export_json(&[], &[]);
        assert_eq!(out, "[\n]\n");
    }

    #[test]
    fn test_json_single_paper() {
        let stats = CheckStats {
            total: 1,
            verified: 1,
            not_found: 0,
            author_mismatch: 0,
            retracted: 0,
            skipped: 0,
        };
        let results = vec![Some(make_result("Good Paper", Status::Verified))];
        let paper = make_paper("test.pdf", &stats, &results);
        let refs = vec![make_ref(0, "Good Paper")];
        let ref_slices: &[&[ReportRef]] = &[&refs];
        let out = export_json(&[paper], ref_slices);
        // Should be valid-ish JSON structure
        assert!(out.starts_with("[\n"));
        assert!(out.ends_with("]\n"));
        assert!(out.contains("\"filename\": \"test.pdf\""));
        assert!(out.contains("\"verified\": 1"));
        assert!(out.contains("\"title\": \"Good Paper\""));
        assert!(out.contains("\"status\": \"verified\""));
    }

    #[test]
    fn test_json_skipped_ref() {
        let stats = CheckStats {
            total: 1,
            verified: 0,
            not_found: 0,
            author_mismatch: 0,
            retracted: 0,
            skipped: 1,
        };
        let results: Vec<Option<ValidationResult>> = vec![];
        let paper = make_paper("test.pdf", &stats, &results);
        let refs = vec![make_ref_skipped(0, "Short", "short_title")];
        let ref_slices: &[&[ReportRef]] = &[&refs];
        let out = export_json(&[paper], ref_slices);
        assert!(out.contains("\"status\": \"skipped\""));
        assert!(out.contains("\"skip_reason\": \"short_title\""));
    }

    #[test]
    fn test_json_fp_override() {
        let stats = CheckStats {
            total: 1,
            verified: 0,
            not_found: 1,
            author_mismatch: 0,
            retracted: 0,
            skipped: 0,
        };
        let results = vec![Some(make_result("FP Ref", Status::NotFound))];
        let paper = make_paper("test.pdf", &stats, &results);
        let refs = vec![make_ref_fp(0, "FP Ref", FpReason::ExistsElsewhere)];
        let ref_slices: &[&[ReportRef]] = &[&refs];
        let out = export_json(&[paper], ref_slices);
        assert!(out.contains("\"effective_status\": \"verified\""));
        assert!(out.contains("\"status\": \"not_found\""));
        assert!(out.contains("\"fp_reason\": \"exists_elsewhere\""));
    }

    #[test]
    fn test_csv_header() {
        let out = export_csv(&[], &[]);
        let first_line = out.lines().next().unwrap();
        assert_eq!(
            first_line,
            "Filename,Verdict,Ref#,Title,Status,EffectiveStatus,FpReason,Source,Retracted,Authors,FoundAuthors,PaperURL,DOI,ArxivID,FailedDBs",
        );
    }

    #[test]
    fn test_csv_single_ref() {
        let stats = CheckStats {
            total: 1,
            verified: 1,
            not_found: 0,
            author_mismatch: 0,
            retracted: 0,
            skipped: 0,
        };
        let results = vec![Some(make_result("My Paper", Status::Verified))];
        let paper = make_paper("test.pdf", &stats, &results);
        let refs = vec![make_ref(0, "My Paper")];
        let ref_slices: &[&[ReportRef]] = &[&refs];
        let out = export_csv(&[paper], ref_slices);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2); // header + 1 data row
        assert!(lines[1].starts_with("test.pdf,"));
        assert!(lines[1].contains("My Paper"));
    }

    #[test]
    fn test_markdown_structure() {
        let stats = CheckStats {
            total: 2,
            verified: 1,
            not_found: 1,
            author_mismatch: 0,
            retracted: 0,
            skipped: 0,
        };
        let results = vec![
            Some(make_result("Good", Status::Verified)),
            Some(make_result("Bad", Status::NotFound)),
        ];
        let paper = make_paper("paper.pdf", &stats, &results);
        let refs = vec![make_ref(0, "Good"), make_ref(1, "Bad")];
        let ref_slices: &[&[ReportRef]] = &[&refs];
        let out = export_markdown(&[paper], ref_slices);
        assert!(out.contains("# Hallucinator Results"));
        assert!(out.contains("## paper.pdf"));
        assert!(out.contains("**2** references"));
        assert!(out.contains("**1** verified"));
        assert!(out.contains("**1** not found"));
    }

    #[test]
    fn test_text_structure() {
        let stats = CheckStats {
            total: 1,
            verified: 1,
            not_found: 0,
            author_mismatch: 0,
            retracted: 0,
            skipped: 0,
        };
        let results = vec![Some(make_result("Paper", Status::Verified))];
        let paper = make_paper("f.pdf", &stats, &results);
        let refs = vec![make_ref(0, "Paper")];
        let ref_slices: &[&[ReportRef]] = &[&refs];
        let out = export_text(&[paper], ref_slices);
        assert!(out.starts_with("Hallucinator Results\n===="));
        assert!(out.contains("f.pdf"));
        assert!(out.contains("1 total"));
        assert!(out.contains("1 verified"));
    }

    #[test]
    fn test_html_structure() {
        let stats = CheckStats {
            total: 1,
            verified: 1,
            not_found: 0,
            author_mismatch: 0,
            retracted: 0,
            skipped: 0,
        };
        let results = vec![Some(make_result("Paper", Status::Verified))];
        let paper = make_paper("f.pdf", &stats, &results);
        let refs = vec![make_ref(0, "Paper")];
        let ref_slices: &[&[ReportRef]] = &[&refs];
        let out = export_html(&[paper], ref_slices);
        assert!(out.contains("<!DOCTYPE html>"));
        assert!(out.contains("stat-card"));
        assert!(out.contains("</html>"));
    }

    #[test]
    fn test_html_verdict_badges() {
        let stats = CheckStats::default();
        let results: Vec<Option<ValidationResult>> = vec![];

        let mut safe_paper = make_paper("safe.pdf", &stats, &results);
        safe_paper.verdict = Some(PaperVerdict::Safe);
        let mut quest_paper = make_paper("quest.pdf", &stats, &results);
        quest_paper.verdict = Some(PaperVerdict::Questionable);

        let empty_refs: &[ReportRef] = &[];
        let ref_slices: &[&[ReportRef]] = &[empty_refs, empty_refs];
        let out = export_html(&[safe_paper, quest_paper], ref_slices);
        assert!(out.contains("badge verified\">SAFE</span>"));
        assert!(out.contains("badge not-found\">?!</span>"));
    }
}
