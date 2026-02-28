use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use hallucinator_core::{DbStatus, MismatchKind, Status};

use crate::app::App;
use crate::model::paper::RefPhase;
use crate::theme::Theme;
use crate::view::truncate;

struct LinkEntry {
    label: String,
    url: String,
}

/// Render the Reference Detail screen into the given area.
/// `footer_area` is a full-width row below the main content + activity panel.
pub fn render_in(
    f: &mut Frame,
    app: &App,
    paper_index: usize,
    ref_index: usize,
    area: Rect,
    footer_area: Rect,
) {
    let theme = &app.theme;
    let paper = &app.papers[paper_index];
    let refs = &app.ref_states[paper_index];
    let rs = &refs[ref_index];

    // --- Collect links for OSC 8 hyperlink rendering ---
    let mut links: Vec<LinkEntry> = Vec::new();
    if let Some(result) = &rs.result {
        let scholar_query = encode_url_param(&rs.title);
        let scholar_url = format!(
            "https://scholar.google.com/scholar?q=%22{}%22",
            scholar_query
        );
        links.push(LinkEntry {
            label: "Open in Google Scholar".into(),
            url: scholar_url,
        });
        if let Some(doi) = &result.doi_info {
            let url = format!("https://doi.org/{}", doi.doi);
            links.push(LinkEntry {
                label: format!("DOI: {url}"),
                url,
            });
        }
        if let Some(arxiv) = &result.arxiv_info {
            let url = format!("https://arxiv.org/abs/{}", arxiv.arxiv_id);
            links.push(LinkEntry {
                label: format!("arXiv: {url}"),
                url,
            });
        }
    } else if matches!(rs.phase, RefPhase::Skipped(_)) && !rs.title.is_empty() {
        let scholar_query = encode_url_param(&rs.title);
        let scholar_url = format!(
            "https://scholar.google.com/scholar?q=%22{}%22",
            scholar_query
        );
        links.push(LinkEntry {
            label: "Open in Google Scholar".into(),
            url: scholar_url,
        });
    }

    let chunks = Layout::vertical([
        Constraint::Length(1), // breadcrumb
        Constraint::Min(5),    // scrollable content
    ])
    .split(area);

    // --- Breadcrumb ---
    let title_short = truncate(&rs.title, 40);
    let breadcrumb = Line::from(vec![
        Span::styled(" Reference ", theme.header_style()),
        Span::styled(" > ", Style::default().fg(theme.dim)),
        Span::styled(
            &paper.filename,
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" > ", Style::default().fg(theme.dim)),
        Span::styled(
            format!("#{} {}", rs.index + 1, title_short),
            Style::default().fg(theme.text),
        ),
    ]);
    f.render_widget(Paragraph::new(breadcrumb), chunks[0]);

    // --- Content ---
    let mut lines: Vec<Line> = Vec::new();

    // Safe marker (FP reason)
    if let Some(reason) = rs.fp_reason {
        lines.push(Line::from(Span::styled(
            format!(
                "  \u{2713} Marked as SAFE \u{2014} {}",
                reason.description()
            ),
            Style::default()
                .fg(theme.verified)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
    }

    // Skipped reference banner
    if let RefPhase::Skipped(reason) = &rs.phase {
        let reason_desc = match reason.as_str() {
            "url_only" => "URL-only (non-academic URL)",
            "short_title" => "Short title (fewer than minimum words)",
            "no_title" => "No title could be extracted",
            other => other,
        };
        lines.push(Line::from(Span::styled(
            format!("  Skipped: {}", reason_desc),
            Style::default().fg(theme.dim).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
    }

    // CITATION section
    section_header(&mut lines, "CITATION", theme);
    labeled_line(&mut lines, "Title", &rs.title, theme);

    // Show raw citation and authors from RefState (always available, even for skipped refs)
    if !rs.raw_citation.is_empty() {
        labeled_line(&mut lines, "Raw Citation", &rs.raw_citation, theme);
    }
    if !rs.authors.is_empty() {
        labeled_line(&mut lines, "Authors", &rs.authors.join(", "), theme);
    }
    if let Some(doi) = &rs.doi {
        labeled_line(&mut lines, "DOI", doi, theme);
    }
    if let Some(arxiv) = &rs.arxiv_id {
        labeled_line(&mut lines, "arXiv ID", arxiv, theme);
    }

    if let Some(result) = &rs.result {
        lines.push(Line::from(""));

        // VALIDATION section
        section_header(&mut lines, "VALIDATION", theme);

        let (status_text, status_color) = if result
            .retraction_info
            .as_ref()
            .is_some_and(|ri| ri.is_retracted)
        {
            ("\u{2620} RETRACTED".to_string(), theme.retracted)
        } else {
            match &result.status {
                Status::Verified => ("\u{2713} Verified".to_string(), theme.verified),
                Status::NotFound => ("\u{2717} Not Found".to_string(), theme.not_found),
                Status::Mismatch(kind) => {
                    (format!("\u{26A0} Mismatch ({})", kind.description()), theme.mismatch)
                }
            }
        };

        lines.push(Line::from(vec![
            Span::styled("  Status:        ", Style::default().fg(theme.dim)),
            Span::styled(
                status_text,
                Style::default()
                    .fg(status_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        if let Some(source) = &result.source {
            labeled_line(&mut lines, "Source", source, theme);
        }
        // Author comparison for mismatches: always show both rows
        let is_author_mismatch = result
            .status
            .mismatch_kind()
            .is_some_and(|k| k.contains(MismatchKind::AUTHOR));
        if is_author_mismatch {
            // PDF Authors (what was extracted from the paper)
            if !result.ref_authors.is_empty() {
                labeled_line(
                    &mut lines,
                    "PDF Authors",
                    &result.ref_authors.join(", "),
                    theme,
                );
            }

            // DB Authors (what the database returned) — always show, even if empty
            if !result.found_authors.is_empty() {
                let overlap = if !result.ref_authors.is_empty() {
                    let ref_set: std::collections::HashSet<_> = result
                        .ref_authors
                        .iter()
                        .map(|a| a.to_lowercase())
                        .collect();
                    let found_set: std::collections::HashSet<_> = result
                        .found_authors
                        .iter()
                        .map(|a| a.to_lowercase())
                        .collect();
                    let overlap_count = ref_set.intersection(&found_set).count();
                    format!(" ({}/{})", overlap_count, result.ref_authors.len())
                } else {
                    String::new()
                };
                labeled_line(
                    &mut lines,
                    "DB Authors",
                    &format!("{}{}", result.found_authors.join(", "), overlap),
                    theme,
                );
            } else {
                lines.push(Line::from(vec![
                    Span::styled("  DB Authors        ", Style::default().fg(theme.dim)),
                    Span::styled(
                        "(no authors returned)",
                        Style::default()
                            .fg(theme.dim)
                            .add_modifier(Modifier::ITALIC),
                    ),
                ]));
            }
        } else if !result.found_authors.is_empty() {
            // For Verified status, just show DB Authors if present
            labeled_line(
                &mut lines,
                "DB Authors",
                &result.found_authors.join(", "),
                theme,
            );
        }

        // DATABASE RESULTS section (per-DB table)
        if !result.db_results.is_empty() {
            lines.push(Line::from(""));
            section_header(&mut lines, "DATABASE RESULTS", theme);

            // Header
            lines.push(Line::from(vec![Span::styled(
                format!(
                    "  {:<20}{:<16}{:<8}{}",
                    "Database", "Result", "Time", "Notes"
                ),
                Style::default().fg(theme.dim).add_modifier(Modifier::BOLD),
            )]));
            lines.push(Line::from(Span::styled(
                "  \u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}",
                Style::default().fg(theme.dim),
            )));

            for db_result in &result.db_results {
                let (result_text, result_color) = match db_result.status {
                    DbStatus::Match => ("\u{2713} match".to_string(), theme.verified),
                    DbStatus::NoMatch => ("no match".to_string(), theme.dim),
                    DbStatus::AuthorMismatch => {
                        ("\u{26A0} mismatch".to_string(), theme.mismatch)
                    }
                    DbStatus::Timeout => ("timeout".to_string(), theme.not_found),
                    DbStatus::RateLimited => (
                        "\u{2717} 429 rate limited".to_string(),
                        theme.mismatch,
                    ),
                    DbStatus::Error => {
                        let msg = db_result.error_message.as_deref().unwrap_or("error");
                        (format!("\u{2717} {}", msg), theme.not_found)
                    }
                    DbStatus::Skipped => ("(skipped)".to_string(), theme.dim),
                };

                let time_str = match db_result.elapsed {
                    Some(d) => format!("{:.1}s", d.as_secs_f64()),
                    None => "\u{2014}".to_string(),
                };

                let mut notes = String::new();
                if db_result.status == DbStatus::Match
                    && result.source.as_deref() == Some(&db_result.db_name)
                {
                    notes = "\u{2190} verified (early exit)".to_string();
                } else if db_result.status == DbStatus::Skipped {
                    notes = "early exit".to_string();
                }

                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {:<20}", db_result.db_name),
                        Style::default().fg(theme.text),
                    ),
                    Span::styled(
                        format!("{:<16}", result_text),
                        Style::default().fg(result_color),
                    ),
                    Span::styled(format!("{:<8}", time_str), Style::default().fg(theme.dim)),
                    Span::styled(notes, Style::default().fg(theme.dim)),
                ]));

                // Show per-DB found authors for mismatch rows
                if db_result.status == DbStatus::AuthorMismatch
                    && !db_result.found_authors.is_empty()
                {
                    lines.push(Line::from(Span::styled(
                        format!("    Authors: {}", db_result.found_authors.join(", ")),
                        Style::default().fg(theme.dim),
                    )));
                }
            }
        }

        // IDENTIFIERS section
        let has_doi = result.doi_info.is_some();
        let has_arxiv = result.arxiv_info.is_some();
        let has_url = result.paper_url.is_some();
        if has_doi || has_arxiv || has_url {
            lines.push(Line::from(""));
            section_header(&mut lines, "IDENTIFIERS", theme);

            if let Some(doi) = &result.doi_info {
                let validity = if doi.valid { "valid" } else { "invalid" };
                labeled_line(
                    &mut lines,
                    "DOI",
                    &format!("{} ({})", doi.doi, validity),
                    theme,
                );
            }
            if let Some(arxiv) = &result.arxiv_info {
                let validity = if arxiv.valid { "valid" } else { "invalid" };
                labeled_line(
                    &mut lines,
                    "arXiv",
                    &format!("{} ({})", arxiv.arxiv_id, validity),
                    theme,
                );
            }
            if let Some(url) = &result.paper_url {
                labeled_line(&mut lines, "Paper URL", url, theme);
            }
        }

        // RETRACTION section
        if let Some(retraction) = &result.retraction_info
            && retraction.is_retracted
        {
            lines.push(Line::from(""));
            // Heavy box border for retraction
            lines.push(Line::from(Span::styled(
                    "  \u{2554}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2557}",
                    Style::default().fg(theme.retracted),
                )));
            lines.push(Line::from(Span::styled(
                "  \u{2551} \u{26A0} WARNING: This paper has been retracted!  \u{2551}",
                Style::default()
                    .fg(theme.retracted)
                    .add_modifier(Modifier::BOLD),
            )));
            if let Some(rdoi) = &retraction.retraction_doi {
                lines.push(Line::from(Span::styled(
                    format!("  \u{2551} DOI: {:<38}\u{2551}", rdoi),
                    Style::default().fg(theme.retracted),
                )));
            }
            if let Some(rsrc) = &retraction.retraction_source {
                lines.push(Line::from(Span::styled(
                    format!("  \u{2551} Source: {:<35}\u{2551}", rsrc),
                    Style::default().fg(theme.retracted),
                )));
            }
            lines.push(Line::from(Span::styled(
                    "  \u{255A}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{255D}",
                    Style::default().fg(theme.retracted),
                )));
        }

        // FAILED DATABASES section
        if !result.failed_dbs.is_empty() {
            lines.push(Line::from(""));
            section_header(&mut lines, "FAILED DATABASES", theme);
            for db in &result.failed_dbs {
                lines.push(Line::from(Span::styled(
                    format!("  - {db}"),
                    Style::default().fg(theme.not_found),
                )));
            }
        }
    } else if matches!(rs.phase, RefPhase::Skipped(_)) {
        // No extra content for skipped refs (links added below)
    } else {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Result pending...",
            Style::default().fg(theme.dim),
        )));
    }

    // LINKS section (inside scrollable content, OSC 8 applied after render)
    if !links.is_empty() {
        lines.push(Line::from(""));
        section_header(&mut lines, "LINKS", theme);
        for link in &links {
            lines.push(Line::from(Span::styled(
                format!("  {}", link.label),
                Style::default().fg(theme.active),
            )));
        }
    }

    let content = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border_style()),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.detail_scroll, 0));

    f.render_widget(content, chunks[1]);

    // Apply OSC 8 hyperlinks to link rows visible in the rendered buffer
    apply_osc8_links(f, chunks[1], &links);

    // --- Footer ---
    render_footer(f, footer_area, theme);
}

fn section_header<'a>(lines: &mut Vec<Line<'a>>, title: &'a str, theme: &Theme) {
    lines.push(Line::from(Span::styled(
        format!("  {title}"),
        Style::default()
            .fg(theme.active)
            .add_modifier(Modifier::BOLD),
    )));
}

fn labeled_line<'a>(lines: &mut Vec<Line<'a>>, label: &'a str, value: &str, theme: &Theme) {
    lines.push(Line::from(vec![
        Span::styled(format!("  {label:<16}"), Style::default().fg(theme.dim)),
        Span::styled(value.to_string(), Style::default().fg(theme.text)),
    ]));
}

/// Scan the rendered buffer inside a bordered content area for link display text
/// and overwrite matching cells with OSC 8 terminal hyperlink escape sequences.
fn apply_osc8_links(f: &mut Frame, content_area: Rect, links: &[LinkEntry]) {
    if links.is_empty() || content_area.height < 3 || content_area.width < 3 {
        return;
    }

    // Inner area (inside the border)
    let inner_x = content_area.x + 1;
    let inner_y = content_area.y + 1;
    let inner_w = content_area.width - 2;
    let inner_h = content_area.height - 2;

    let buf = f.buffer_mut();

    for link in links {
        let display = format!("  {}", link.label);
        let display_chars: Vec<char> = display.chars().collect();

        // Scan visible rows for this link's display text
        for row in 0..inner_h {
            let y = inner_y + row;

            // Check if this row starts with the display text
            let matches = display_chars.iter().enumerate().all(|(col, &ch)| {
                let x = inner_x + col as u16;
                if x >= inner_x + inner_w {
                    return false;
                }
                let sym = buf[(x, y)].symbol();
                sym.len() == 1 && sym.as_bytes()[0] as char == ch
            });

            if !matches || display_chars.is_empty() {
                continue;
            }

            // Overwrite label cells with OSC 8 hyperlink in 2-char chunks
            // (workaround for ratatui#902 — ANSI width miscalculation)
            let label_chars: Vec<char> = link.label.chars().collect();
            let mut pos = 0usize;
            while pos < label_chars.len() {
                let end = (pos + 2).min(label_chars.len());
                let chunk: String = label_chars[pos..end].iter().collect();
                let cell_x = inner_x + 2 + pos as u16;
                if cell_x >= inner_x + inner_w {
                    break;
                }
                let hyperlink = format!("\x1B]8;;{}\x07{}\x1B]8;;\x07", link.url, chunk);
                buf[(cell_x, y)].set_symbol(&hyperlink);
                pos = end;
            }
            break; // Found this link, move to next
        }
    }
}

fn render_footer(f: &mut Frame, area: Rect, theme: &Theme) {
    let footer = Line::from(Span::styled(
        " j/k:scroll  Space:cycle FP reason  Ctrl+r:retry  y:copy ref  e:export  Esc:back  ?:help",
        theme.footer_style(),
    ));
    f.render_widget(Paragraph::new(footer), area);
}

/// Percent-encode a string for use in a URL query parameter.
fn encode_url_param(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push(char::from(HEX[(b >> 4) as usize]));
                out.push(char::from(HEX[(b & 0xf) as usize]));
            }
        }
    }
    out
}

const HEX: &[u8; 16] = b"0123456789ABCDEF";
