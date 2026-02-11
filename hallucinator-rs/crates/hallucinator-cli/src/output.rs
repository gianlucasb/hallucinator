use std::io::Write;

use hallucinator_core::{ProgressEvent, SkipStats, Status, ValidationResult};
use owo_colors::OwoColorize;

/// Whether to use colored output.
#[derive(Debug, Clone, Copy)]
pub struct ColorMode(pub bool);

impl ColorMode {
    pub fn enabled(&self) -> bool {
        self.0
    }
}

/// Print the extraction summary after PDF parsing.
pub fn print_extraction_summary(
    w: &mut dyn Write,
    pdf_name: &str,
    total_refs: usize,
    skip_stats: &SkipStats,
    color: ColorMode,
) -> std::io::Result<()> {
    writeln!(w, "Extracting references from {}...", pdf_name)?;
    writeln!(w, "Found {} references to check", total_refs)?;

    let skipped = skip_stats.url_only + skip_stats.short_title;
    if skipped > 0 {
        if color.enabled() {
            writeln!(
                w,
                "{}",
                format!(
                    "(Skipped {} URLs, {} short titles)",
                    skip_stats.url_only, skip_stats.short_title
                )
                .dimmed()
            )?;
        } else {
            writeln!(
                w,
                "(Skipped {} URLs, {} short titles)",
                skip_stats.url_only, skip_stats.short_title
            )?;
        }
    }
    writeln!(w)?;
    Ok(())
}

/// Print a real-time progress event.
pub fn print_progress(
    w: &mut dyn Write,
    event: &ProgressEvent,
    color: ColorMode,
) -> std::io::Result<()> {
    match event {
        ProgressEvent::Checking {
            index,
            total,
            title,
        } => {
            let short = if title.len() > 50 {
                format!("{}...", &title[..50])
            } else {
                title.clone()
            };
            writeln!(w, "[{}/{}] Checking: \"{}\"", index + 1, total, short)?;
        }
        ProgressEvent::Result {
            index,
            total,
            result,
        } => {
            let idx = index + 1;
            match result.status {
                Status::Verified => {
                    let source = result.source.as_deref().unwrap_or("unknown");
                    if color.enabled() {
                        writeln!(
                            w,
                            "[{}/{}] -> {} ({})",
                            idx,
                            total,
                            "VERIFIED".green(),
                            source
                        )?;
                    } else {
                        writeln!(w, "[{}/{}] -> VERIFIED ({})", idx, total, source)?;
                    }
                }
                Status::AuthorMismatch => {
                    let source = result.source.as_deref().unwrap_or("unknown");
                    if color.enabled() {
                        writeln!(
                            w,
                            "[{}/{}] -> {} ({})",
                            idx,
                            total,
                            "AUTHOR MISMATCH".yellow(),
                            source
                        )?;
                    } else {
                        writeln!(w, "[{}/{}] -> AUTHOR MISMATCH ({})", idx, total, source)?;
                    }
                }
                Status::NotFound => {
                    if color.enabled() {
                        writeln!(w, "[{}/{}] -> {}", idx, total, "NOT FOUND".red())?;
                    } else {
                        writeln!(w, "[{}/{}] -> NOT FOUND", idx, total)?;
                    }
                }
            }
        }
        ProgressEvent::Warning { message, .. } => {
            if color.enabled() {
                writeln!(w, "{} {}", "WARNING:".yellow(), message)?;
            } else {
                writeln!(w, "WARNING: {}", message)?;
            }
        }
        ProgressEvent::RetryPass { count } => {
            writeln!(w)?;
            writeln!(
                w,
                "Retrying {} references that had database errors...",
                count
            )?;
        }
        ProgressEvent::DatabaseQueryComplete { .. } => {
            // Not displayed in CLI output
        }
    }
    Ok(())
}

/// Print the detailed hallucination/mismatch report for all problematic references.
pub fn print_hallucination_report(
    w: &mut dyn Write,
    results: &[ValidationResult],
    searched_openalex: bool,
    color: ColorMode,
) -> std::io::Result<()> {
    for result in results {
        match result.status {
            Status::NotFound => {
                print_not_found_block(w, result, searched_openalex, color)?;
            }
            Status::AuthorMismatch => {
                print_author_mismatch_block(w, result, color)?;
            }
            Status::Verified => {}
        }
    }
    Ok(())
}

fn print_not_found_block(
    w: &mut dyn Write,
    result: &ValidationResult,
    searched_openalex: bool,
    color: ColorMode,
) -> std::io::Result<()> {
    writeln!(w)?;
    let sep = "=".repeat(60);
    if color.enabled() {
        writeln!(w, "{}", sep.bold().red())?;
        writeln!(w, "{}", "POTENTIAL HALLUCINATION DETECTED".bold().red())?;
        writeln!(w, "{}", sep.bold().red())?;
    } else {
        writeln!(w, "{}", sep)?;
        writeln!(w, "POTENTIAL HALLUCINATION DETECTED")?;
        writeln!(w, "{}", sep)?;
    }
    writeln!(w)?;

    if color.enabled() {
        writeln!(w, "{}:", "Title".bold())?;
        writeln!(w, "  {}", result.title.cyan())?;
    } else {
        writeln!(w, "Title:")?;
        writeln!(w, "  {}", result.title)?;
    }
    writeln!(w)?;

    if color.enabled() {
        writeln!(w, "{} Reference not found in any database", "Status:".red())?;
    } else {
        writeln!(w, "Status: Reference not found in any database")?;
    }

    let dbs = if searched_openalex {
        "Searched: OpenAlex, CrossRef, arXiv, DBLP, Semantic Scholar, ACL, Europe PMC, PubMed"
    } else {
        "Searched: CrossRef, arXiv, DBLP, Semantic Scholar, ACL, Europe PMC, PubMed"
    };
    if color.enabled() {
        writeln!(w, "{}", dbs.dimmed())?;
    } else {
        writeln!(w, "{}", dbs)?;
    }

    writeln!(w)?;
    let dash_sep = "-".repeat(60);
    if color.enabled() {
        writeln!(w, "{}", dash_sep.bold().red())?;
    } else {
        writeln!(w, "{}", dash_sep)?;
    }
    writeln!(w)?;
    Ok(())
}

fn print_author_mismatch_block(
    w: &mut dyn Write,
    result: &ValidationResult,
    color: ColorMode,
) -> std::io::Result<()> {
    writeln!(w)?;
    let sep = "=".repeat(60);
    if color.enabled() {
        writeln!(w, "{}", sep.bold().red())?;
        writeln!(w, "{}", "POTENTIAL HALLUCINATION DETECTED".bold().red())?;
        writeln!(w, "{}", sep.bold().red())?;
    } else {
        writeln!(w, "{}", sep)?;
        writeln!(w, "POTENTIAL HALLUCINATION DETECTED")?;
        writeln!(w, "{}", sep)?;
    }
    writeln!(w)?;

    let source = result.source.as_deref().unwrap_or("unknown");

    if color.enabled() {
        writeln!(w, "{}:", "Title".bold())?;
        writeln!(w, "  {}", result.title.cyan())?;
    } else {
        writeln!(w, "Title:")?;
        writeln!(w, "  {}", result.title)?;
    }
    writeln!(w)?;

    if color.enabled() {
        writeln!(
            w,
            "{} Title found on {} but authors don't match",
            "Status:".yellow(),
            source
        )?;
    } else {
        writeln!(
            w,
            "Status: Title found on {} but authors don't match",
            source
        )?;
    }
    writeln!(w)?;

    // We don't have ref_authors stored in ValidationResult directly,
    // so we show only the found (DB) authors.
    if color.enabled() {
        writeln!(w, "{}", format!("Authors in {}:", source).bold())?;
        for author in &result.found_authors {
            writeln!(w, "  {}", format!("• {}", author).magenta())?;
        }
    } else {
        writeln!(w, "Authors in {}:", source)?;
        for author in &result.found_authors {
            writeln!(w, "  • {}", author)?;
        }
    }

    writeln!(w)?;
    let dash_sep = "-".repeat(60);
    if color.enabled() {
        writeln!(w, "{}", dash_sep.bold().red())?;
    } else {
        writeln!(w, "{}", dash_sep)?;
    }
    writeln!(w)?;
    Ok(())
}

/// Print DOI-related issues.
pub fn print_doi_issues(
    w: &mut dyn Write,
    results: &[ValidationResult],
    color: ColorMode,
) -> std::io::Result<()> {
    let issues: Vec<_> = results
        .iter()
        .filter(|r| r.doi_info.as_ref().map_or(false, |d| !d.valid))
        .collect();

    if issues.is_empty() {
        return Ok(());
    }

    writeln!(w)?;
    let sep = "=".repeat(60);
    if color.enabled() {
        writeln!(w, "{}", sep.bold().red())?;
        writeln!(
            w,
            "{}",
            "DOI ISSUES - POTENTIAL HALLUCINATIONS".bold().red()
        )?;
        writeln!(w, "{}", sep.bold().red())?;
    } else {
        writeln!(w, "{}", sep)?;
        writeln!(w, "DOI ISSUES - POTENTIAL HALLUCINATIONS")?;
        writeln!(w, "{}", sep)?;
    }

    for result in issues {
        let doi_info = result.doi_info.as_ref().unwrap();
        let short_title = truncate(&result.title, 70);
        writeln!(w)?;
        if color.enabled() {
            writeln!(w, "{} {}", "Reference:".bold(), short_title)?;
            writeln!(w, "{} {}", "DOI:".bold(), doi_info.doi)?;
            writeln!(w, "{} DOI does not resolve", "Issue:".red())?;
        } else {
            writeln!(w, "Reference: {}", short_title)?;
            writeln!(w, "DOI: {}", doi_info.doi)?;
            writeln!(w, "Issue: DOI does not resolve")?;
        }
    }
    writeln!(w)?;
    Ok(())
}

/// Print retraction warnings.
pub fn print_retraction_warnings(
    w: &mut dyn Write,
    results: &[ValidationResult],
    color: ColorMode,
) -> std::io::Result<()> {
    let retracted: Vec<_> = results
        .iter()
        .filter(|r| {
            r.retraction_info
                .as_ref()
                .map_or(false, |ri| ri.is_retracted)
        })
        .collect();

    if retracted.is_empty() {
        return Ok(());
    }

    writeln!(w)?;
    let sep = "=".repeat(60);
    if color.enabled() {
        writeln!(w, "{}", sep.bold().red())?;
        writeln!(w, "{}", "RETRACTED PAPERS".bold().red())?;
        writeln!(w, "{}", sep.bold().red())?;
    } else {
        writeln!(w, "{}", sep)?;
        writeln!(w, "RETRACTED PAPERS")?;
        writeln!(w, "{}", sep)?;
    }

    for result in &retracted {
        let ri = result.retraction_info.as_ref().unwrap();
        let short_title = truncate(&result.title, 70);
        writeln!(w)?;
        if color.enabled() {
            writeln!(w, "{} {}", "Reference:".bold(), short_title)?;
            writeln!(
                w,
                "{} {}",
                "Status:".red().bold(),
                ri.retraction_source.as_deref().unwrap_or("Retraction")
            )?;
        } else {
            writeln!(w, "Reference: {}", short_title)?;
            writeln!(
                w,
                "Status: {}",
                ri.retraction_source.as_deref().unwrap_or("Retraction")
            )?;
        }
        if let Some(ref doi) = ri.retraction_doi {
            if color.enabled() {
                writeln!(w, "{} https://doi.org/{}", "Retraction notice:".bold(), doi)?;
            } else {
                writeln!(w, "Retraction notice: https://doi.org/{}", doi)?;
            }
        }
    }
    writeln!(w)?;
    Ok(())
}

/// Print the final summary.
pub fn print_summary(
    w: &mut dyn Write,
    results: &[ValidationResult],
    skip_stats: &SkipStats,
    color: ColorMode,
) -> std::io::Result<()> {
    let verified = results
        .iter()
        .filter(|r| r.status == Status::Verified)
        .count();
    let not_found = results
        .iter()
        .filter(|r| r.status == Status::NotFound)
        .count();
    let mismatched = results
        .iter()
        .filter(|r| r.status == Status::AuthorMismatch)
        .count();
    let retracted = results
        .iter()
        .filter(|r| {
            r.retraction_info
                .as_ref()
                .map_or(false, |ri| ri.is_retracted)
        })
        .count();

    writeln!(w)?;
    let sep = "=".repeat(60);
    if color.enabled() {
        writeln!(w, "{}", sep.bold())?;
        writeln!(w, "{}", "SUMMARY".bold())?;
        writeln!(w, "{}", sep.bold())?;
    } else {
        writeln!(w, "{}", sep)?;
        writeln!(w, "SUMMARY")?;
        writeln!(w, "{}", sep)?;
    }

    let total_skipped = skip_stats.url_only + skip_stats.short_title;
    writeln!(w, "  Total references found: {}", skip_stats.total_raw)?;
    writeln!(w, "  References analyzed: {}", results.len())?;
    if total_skipped > 0 {
        let msg = format!(
            "Skipped: {} (URLs: {}, short titles: {})",
            total_skipped, skip_stats.url_only, skip_stats.short_title
        );
        if color.enabled() {
            writeln!(w, "  {}", msg.dimmed())?;
        } else {
            writeln!(w, "  {}", msg)?;
        }
    }
    if skip_stats.no_authors > 0 {
        let msg = format!(
            "Title-only (no authors extracted): {}",
            skip_stats.no_authors
        );
        if color.enabled() {
            writeln!(w, "  {}", msg.dimmed())?;
        } else {
            writeln!(w, "  {}", msg)?;
        }
    }
    writeln!(w)?;

    if color.enabled() {
        writeln!(w, "  {} {}", "Verified:".green(), verified)?;
    } else {
        writeln!(w, "  Verified: {}", verified)?;
    }
    if mismatched > 0 {
        if color.enabled() {
            writeln!(w, "  {} {}", "Author mismatches:".yellow(), mismatched)?;
        } else {
            writeln!(w, "  Author mismatches: {}", mismatched)?;
        }
    }
    if not_found > 0 {
        if color.enabled() {
            writeln!(
                w,
                "  {} {}",
                "Not found (potential hallucinations):".red(),
                not_found
            )?;
        } else {
            writeln!(w, "  Not found (potential hallucinations): {}", not_found)?;
        }
    }
    if retracted > 0 {
        if color.enabled() {
            writeln!(w, "  {} {}", "Retracted papers:".red(), retracted)?;
        } else {
            writeln!(w, "  Retracted papers: {}", retracted)?;
        }
    }

    // DOI stats
    let dois_found = results.iter().filter(|r| r.doi_info.is_some()).count();
    let dois_valid = results
        .iter()
        .filter(|r| r.doi_info.as_ref().map_or(false, |d| d.valid))
        .count();
    if dois_found > 0 && dois_valid > 0 {
        let msg = format!("DOIs validated: {}/{}", dois_valid, dois_found);
        writeln!(w)?;
        if color.enabled() {
            writeln!(w, "  {}", msg.dimmed())?;
        } else {
            writeln!(w, "  {}", msg)?;
        }
    }

    writeln!(w)?;
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}...", &s[..max])
    } else {
        s.to_string()
    }
}
