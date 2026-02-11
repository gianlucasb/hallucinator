use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;

use crate::app::{App, InputMode};
use crate::model::queue::PaperPhase;
use crate::theme::Theme;
use crate::view::{spinner_char, truncate};

/// Render the Queue screen into the given area.
pub fn render_in(f: &mut Frame, app: &mut App, area: Rect) {
    let theme = &app.theme;

    let has_search = app.input_mode == InputMode::Search || !app.search_query.is_empty();

    let mut constraints = vec![
        Constraint::Length(1), // header
        Constraint::Length(1), // progress bar
    ];
    if has_search {
        constraints.push(Constraint::Length(1)); // search bar
    }
    constraints.push(Constraint::Min(5)); // table (fills all available space)
    constraints.push(Constraint::Length(1)); // footer / stats

    let chunks = Layout::vertical(constraints).split(area);
    let mut chunk_idx = 0;

    render_header(f, chunks[chunk_idx], app, theme);
    chunk_idx += 1;

    render_progress_bar(f, chunks[chunk_idx], app, theme);
    chunk_idx += 1;

    if has_search {
        render_search_bar(f, chunks[chunk_idx], app, theme);
        chunk_idx += 1;
    }

    let table_area = chunks[chunk_idx];
    render_table(f, table_area, app);
    app.last_table_area = Some(table_area);
    chunk_idx += 1;

    render_footer(f, chunks[chunk_idx], app);
}

fn render_header(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let mut spans = vec![
        Span::styled(" HALLUCINATOR ", theme.header_style()),
        Span::styled(
            " Queue",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
    ];

    // Filter indicator
    if app.queue_filter != crate::model::queue::QueueFilter::All {
        spans.push(Span::styled(
            format!(" [filter: {}]", app.queue_filter.label()),
            Style::default().fg(theme.active),
        ));
    }

    let header = Paragraph::new(Line::from(spans));
    f.render_widget(header, area);
}

fn render_progress_bar(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let total = app.papers.len();
    let done = app.papers.iter().filter(|p| p.phase.is_terminal()).count();
    let ratio = if total > 0 {
        done as f64 / total as f64
    } else {
        0.0
    };

    // Build a text progress bar: ██████░░░░ 12/50
    let bar_width = (area.width as usize).saturating_sub(12);
    let filled = (ratio * bar_width as f64) as usize;
    let empty = bar_width.saturating_sub(filled);

    let bar: String = "\u{2588}".repeat(filled) + &"\u{2591}".repeat(empty);
    let elapsed = app.elapsed();
    let elapsed_str = format!("{}:{:02}", elapsed.as_secs() / 60, elapsed.as_secs() % 60);

    let mut spans = vec![
        Span::styled(" ", Style::default()),
        Span::styled(&bar, Style::default().fg(theme.active)),
        Span::styled(
            format!(" {}/{} ", done, total),
            Style::default().fg(theme.text),
        ),
        Span::styled(elapsed_str, Style::default().fg(theme.dim)),
    ];

    // Show archive extraction indicator if active
    if let Some(archive_name) = &app.extracting_archive {
        let label = if app.extracted_count > 0 {
            format!(
                " {} Extracting {} ({} extracted)...",
                spinner_char(app.tick),
                archive_name,
                app.extracted_count,
            )
        } else {
            format!(" {} Extracting {}...", spinner_char(app.tick), archive_name,)
        };
        spans.push(Span::styled(
            label,
            Style::default()
                .fg(theme.active)
                .add_modifier(Modifier::BOLD),
        ));
    }

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_search_bar(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let cursor = if app.input_mode == InputMode::Search {
        "\u{2588}"
    } else {
        ""
    };
    let line = Line::from(vec![
        Span::styled(
            " /",
            Style::default()
                .fg(theme.active)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(&app.search_query, Style::default().fg(theme.text)),
        Span::styled(cursor, Style::default().fg(theme.active)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn render_table(f: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let wide = area.width >= 100;

    // Build header row
    let header_cells = if wide {
        vec![
            "#", "Paper", "Refs", "OK", "Mis", "NF", "Ret", "%", "Status",
        ]
    } else {
        vec!["#", "Paper", "Refs", "Prob", "Status"]
    };
    let header = Row::new(header_cells.iter().map(|h| {
        Cell::from(*h).style(Style::default().fg(theme.text).add_modifier(Modifier::BOLD))
    }))
    .height(1);

    // Use the pre-computed sorted/filtered indices
    let indices = &app.queue_sorted;

    // Build data rows
    let rows: Vec<Row> = indices
        .iter()
        .enumerate()
        .map(|(display_idx, &paper_idx)| {
            let paper = &app.papers[paper_idx];
            let num = format!("{}", display_idx + 1);
            let name = truncate(&paper.filename, (area.width as usize).saturating_sub(40));

            let phase_style = Style::default().fg(theme.paper_phase_color(&paper.phase));

            let status_text = match &paper.phase {
                PaperPhase::Retrying => {
                    if paper.retry_total > 0 {
                        format!(
                            "{} Retrying {}/{}",
                            spinner_char(app.tick),
                            paper.retry_done,
                            paper.retry_total
                        )
                    } else {
                        format!("{} Retrying...", spinner_char(app.tick))
                    }
                }
                PaperPhase::Checking | PaperPhase::Extracting => {
                    format!("{} {}", spinner_char(app.tick), paper.phase.label())
                }
                _ => paper.phase.label().to_string(),
            };

            if wide {
                let refs = if paper.total_refs > 0 {
                    format!("{}", paper.total_refs)
                } else {
                    "\u{2014}".to_string()
                };
                let pct = paper.problematic_pct();
                let pct_text = if paper.total_refs > 0 && paper.completed_count() > 0 {
                    if pct >= 10.0 {
                        format!("{:.0}", pct)
                    } else {
                        format!("{:.1}", pct)
                    }
                } else {
                    "\u{2014}".to_string()
                };
                let pct_style = if pct > 0.0 {
                    Style::default().fg(theme.not_found)
                } else {
                    Style::default().fg(theme.dim)
                };
                Row::new(vec![
                    Cell::from(num),
                    Cell::from(name),
                    Cell::from(refs),
                    Cell::from(format!("{}", paper.stats.verified))
                        .style(Style::default().fg(theme.verified)),
                    Cell::from(format!("{}", paper.stats.author_mismatch))
                        .style(Style::default().fg(theme.author_mismatch)),
                    Cell::from(format!("{}", paper.stats.not_found))
                        .style(Style::default().fg(theme.not_found)),
                    Cell::from(format!("{}", paper.stats.retracted))
                        .style(Style::default().fg(theme.retracted)),
                    Cell::from(pct_text).style(pct_style),
                    Cell::from(status_text).style(phase_style),
                ])
            } else {
                let problems = paper.problems();
                let prob_style = if problems > 0 {
                    Style::default().fg(theme.not_found)
                } else {
                    Style::default().fg(theme.dim)
                };
                Row::new(vec![
                    Cell::from(num),
                    Cell::from(name),
                    Cell::from(if paper.total_refs > 0 {
                        format!("{}", paper.total_refs)
                    } else {
                        "\u{2014}".to_string()
                    }),
                    Cell::from(format!("{}", problems)).style(prob_style),
                    Cell::from(status_text).style(phase_style),
                ])
            }
        })
        .collect();

    let widths = if wide {
        vec![
            Constraint::Length(4),
            Constraint::Min(20),
            Constraint::Length(5),
            Constraint::Length(5),
            Constraint::Length(5),
            Constraint::Length(5),
            Constraint::Length(5),
            Constraint::Length(5),
            Constraint::Length(14),
        ]
    } else {
        vec![
            Constraint::Length(4),
            Constraint::Min(15),
            Constraint::Length(5),
            Constraint::Length(5),
            Constraint::Length(14),
        ]
    };

    let table = Table::new(rows, &widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border_style())
                .title(format!(" Sort: {} (s) ", app.sort_order.label())),
        )
        .row_highlight_style(theme.highlight_style());

    let mut state = TableState::default();
    state.select(Some(app.queue_cursor));
    f.render_stateful_widget(table, area, &mut state);
}

fn render_footer(f: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let total = app.papers.len();
    let done = app.papers.iter().filter(|p| p.phase.is_terminal()).count();

    let total_verified: usize = app.papers.iter().map(|p| p.stats.verified).sum();
    let total_not_found: usize = app.papers.iter().map(|p| p.stats.not_found).sum();
    let total_mismatch: usize = app.papers.iter().map(|p| p.stats.author_mismatch).sum();
    let total_retracted: usize = app.papers.iter().map(|p| p.stats.retracted).sum();

    let mut spans = vec![
        Span::styled(
            format!(" {}/{} papers ", done, total),
            Style::default().fg(theme.text),
        ),
        Span::styled(
            format!("V:{} ", total_verified),
            Style::default().fg(theme.verified),
        ),
        Span::styled(
            format!("M:{} ", total_mismatch),
            Style::default().fg(theme.author_mismatch),
        ),
        Span::styled(
            format!("NF:{} ", total_not_found),
            Style::default().fg(theme.not_found),
        ),
        Span::styled(
            format!("R:{} ", total_retracted),
            Style::default().fg(theme.retracted),
        ),
    ];

    // Show [Space] Start/Stop indicator
    if !app.processing_started && !app.file_paths.is_empty() {
        spans.push(Span::styled(
            " [Space] Start ",
            Style::default()
                .fg(theme.header_fg)
                .bg(theme.active)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            " j/k:nav  Enter:open  o:add  ,:config  ?:help  q:quit",
            theme.footer_style(),
        ));
    } else if app.processing_started && !app.batch_complete {
        spans.push(Span::styled(
            " [Space] Stop ",
            Style::default()
                .fg(theme.header_fg)
                .bg(theme.not_found)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            " j/k:nav  Enter:open  s:sort  f:filter  /:search  ?:help",
            theme.footer_style(),
        ));
    } else {
        spans.push(Span::styled(
            " | j/k:nav  Enter:open  s:sort  f:filter  /:search  o:add  ?:help  q:quit",
            theme.footer_style(),
        ));
    }

    let footer = Line::from(spans);
    f.render_widget(Paragraph::new(footer), area);
}
