use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Gauge, Paragraph, Row, Table, TableState, Wrap};
use ratatui::Frame;

use crate::app::{App, InputMode};
use crate::model::paper::{PaperFilter, RefPhase};
use crate::theme::Theme;
use crate::view::{spinner_char, truncate};

/// Render the Paper detail screen into the given area.
pub fn render_in(f: &mut Frame, app: &mut App, paper_index: usize, area: Rect) {
    let theme = &app.theme;
    let paper = &app.papers[paper_index];
    let show_preview = area.height >= 40;
    let has_search = app.input_mode == InputMode::Search || !app.search_query.is_empty();

    let mut constraints = vec![
        Constraint::Length(1), // breadcrumb
        Constraint::Length(3), // progress bar
    ];
    if has_search {
        constraints.push(Constraint::Length(1)); // search bar
    }
    constraints.push(Constraint::Min(8)); // ref table
    if show_preview {
        constraints.push(Constraint::Length(6)); // raw citation preview
    }
    constraints.push(Constraint::Length(1)); // footer

    let chunks = Layout::vertical(constraints).split(area);
    let mut ci = 0;

    render_breadcrumb(f, chunks[ci], &paper.filename, theme);
    ci += 1;
    render_progress(f, chunks[ci], paper, app.tick, theme);
    ci += 1;

    if has_search {
        render_search_bar(f, chunks[ci], app, theme);
        ci += 1;
    }

    let table_area = chunks[ci];
    render_ref_table(f, table_area, app, paper_index);
    app.last_table_area = Some(table_area);
    ci += 1;

    if show_preview {
        render_preview(f, chunks[ci], app, paper_index);
        ci += 1;
    }

    let paper = &app.papers[paper_index];
    render_footer(f, chunks[ci], app, paper, theme);
}

fn render_breadcrumb(f: &mut Frame, area: Rect, filename: &str, theme: &Theme) {
    let breadcrumb = Line::from(vec![
        Span::styled(" HALLUCINATOR ", theme.header_style()),
        Span::styled(" > ", Style::default().fg(theme.dim)),
        Span::styled(
            filename,
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
    ]);
    f.render_widget(Paragraph::new(breadcrumb), area);
}

fn render_progress(
    f: &mut Frame,
    area: Rect,
    paper: &crate::model::queue::PaperState,
    tick: usize,
    theme: &Theme,
) {
    let done = paper.completed_count();
    let total = paper.total_refs;
    let ratio = if total > 0 {
        done as f64 / total as f64
    } else {
        0.0
    };

    let label = format!("{} {} / {} refs", spinner_char(tick), done, total);

    let gauge = Gauge::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border_style()),
        )
        .gauge_style(Style::default().fg(theme.active))
        .ratio(ratio)
        .label(label);

    f.render_widget(gauge, area);
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

fn render_ref_table(f: &mut Frame, area: Rect, app: &App, paper_index: usize) {
    let theme = &app.theme;
    let wide = area.width >= 80;

    let header_cells = if wide {
        vec!["#", "Reference", "Verdict", "Source"]
    } else {
        vec!["#", "Reference", "Verdict"]
    };
    let header = Row::new(header_cells.iter().map(|h| {
        Cell::from(*h).style(Style::default().fg(theme.text).add_modifier(Modifier::BOLD))
    }))
    .height(1);

    let refs = &app.ref_states[paper_index];
    let indices = app.paper_ref_indices(paper_index);

    let rows: Vec<Row> = indices
        .iter()
        .map(|&ri| {
            let rs = &refs[ri];
            let num = format!("{}", rs.index + 1);
            let title_display = match rs.phase {
                RefPhase::Checking | RefPhase::Retrying => {
                    format!("{} {}", spinner_char(app.tick), rs.title)
                }
                _ => rs.title.clone(),
            };
            let title_text = truncate(&title_display, (area.width as usize).saturating_sub(30));
            let phase_style = theme.ref_phase_style(&rs.phase);

            let verdict = rs.verdict_label();
            let verdict_style = if rs.marked_safe {
                Style::default()
                    .fg(theme.verified)
                    .add_modifier(Modifier::DIM)
            } else {
                match &rs.result {
                    Some(r) => {
                        let color = if r.retraction_info.as_ref().is_some_and(|ri| ri.is_retracted)
                        {
                            theme.retracted
                        } else {
                            theme.status_color(&r.status)
                        };
                        Style::default().fg(color).add_modifier(Modifier::BOLD)
                    }
                    None => phase_style,
                }
            };

            let mut cells = vec![
                Cell::from(num).style(phase_style),
                Cell::from(title_text).style(phase_style),
                Cell::from(verdict).style(verdict_style),
            ];

            if wide {
                cells.push(Cell::from(rs.source_label()).style(phase_style));
            }

            Row::new(cells)
        })
        .collect();

    let widths = if wide {
        vec![
            Constraint::Length(4),
            Constraint::Min(20),
            Constraint::Length(14),
            Constraint::Length(18),
        ]
    } else {
        vec![
            Constraint::Length(4),
            Constraint::Min(15),
            Constraint::Length(14),
        ]
    };

    let block_title = format!(" References | sort: {} (s) ", app.paper_sort.label());

    let table = Table::new(rows, &widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border_style())
                .title(block_title),
        )
        .row_highlight_style(theme.highlight_style());

    let mut state = TableState::default();
    state.select(Some(app.paper_cursor));
    f.render_stateful_widget(table, area, &mut state);
}

fn render_preview(f: &mut Frame, area: Rect, app: &App, paper_index: usize) {
    let theme = &app.theme;
    let refs = &app.ref_states[paper_index];
    let indices = app.paper_ref_indices(paper_index);

    let text = if app.paper_cursor < indices.len() {
        let ri = indices[app.paper_cursor];
        let rs = &refs[ri];
        match &rs.result {
            Some(r) => r.raw_citation.clone(),
            None => "Pending...".to_string(),
        }
    } else {
        String::new()
    };

    let preview = Paragraph::new(text)
        .style(Style::default().fg(theme.dim))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border_style())
                .title(" Raw Citation "),
        )
        .wrap(Wrap { trim: true });

    f.render_widget(preview, area);
}

fn render_footer(
    f: &mut Frame,
    area: Rect,
    app: &App,
    paper: &crate::model::queue::PaperState,
    theme: &Theme,
) {
    let mut spans = vec![Span::styled(
        format!(
            " V:{} M:{} NF:{} R:{} ",
            paper.stats.verified,
            paper.stats.author_mismatch,
            paper.stats.not_found,
            paper.stats.retracted
        ),
        Style::default().fg(theme.text),
    )];

    // Filter indicator
    if app.paper_filter != PaperFilter::All {
        spans.push(Span::styled(
            format!("[filter: {}] ", app.paper_filter.label()),
            Style::default().fg(theme.active),
        ));
    }

    spans.push(Span::styled(
        " | j/k:nav  Space:safe  Enter:detail  Ctrl+r:retry  R:retry all  s:sort  f:filter  /:search  Esc:back",
        theme.footer_style(),
    ));

    let footer = Line::from(spans);
    f.render_widget(Paragraph::new(footer), area);
}
