use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;

/// Render the activity panel in the given area.
pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let activity = &app.activity;

    let mut lines: Vec<Line> = Vec::new();

    // Database health section
    lines.push(Line::from(Span::styled(
        " Database Health",
        Style::default()
            .fg(theme.active)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    // Column header
    if !activity.db_health.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(
                format!("   {:<18}", "Database"),
                Style::default().fg(theme.dim).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{:>5}", "Qry"),
                Style::default().fg(theme.dim).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{:>7}", "Avg ms"),
                Style::default().fg(theme.dim).add_modifier(Modifier::BOLD),
            ),
        ]));
    }

    let mut db_names: Vec<&String> = activity.db_health.keys().collect();
    db_names.sort();

    for name in db_names {
        let health = &activity.db_health[name];
        let indicator = health.indicator();
        let avg_ms = if health.total_queries > 0 {
            format!("{:.0}ms", health.avg_response_ms)
        } else {
            "\u{2014}".to_string()
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {} {:<18}", indicator, name),
                Style::default().fg(theme.text),
            ),
            Span::styled(
                format!("{:>4}", health.total_queries),
                Style::default().fg(theme.dim),
            ),
            Span::styled(format!("{:>7}", avg_ms), Style::default().fg(theme.dim)),
        ]));
    }

    if activity.db_health.is_empty() {
        lines.push(Line::from(Span::styled(
            " (no data yet)",
            Style::default().fg(theme.dim),
        )));
    }

    // Throughput sparkline
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Throughput",
        Style::default()
            .fg(theme.active)
            .add_modifier(Modifier::BOLD),
    )));

    let sparkline = activity.sparkline();
    if !sparkline.trim().is_empty() {
        lines.push(Line::from(Span::styled(
            format!(" {}", sparkline),
            Style::default().fg(theme.active),
        )));
        let rate = if activity.throughput_buckets.len() >= 2 {
            let recent: u16 = activity.throughput_buckets.iter().rev().take(5).sum();
            let count = activity.throughput_buckets.len().min(5) as f64;
            recent as f64 / count
        } else {
            0.0
        };
        lines.push(Line::from(Span::styled(
            format!(" {:.1} refs/sec", rate),
            Style::default().fg(theme.dim),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            " (no data yet)",
            Style::default().fg(theme.dim),
        )));
    }

    // Active queries
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Active Queries",
        Style::default()
            .fg(theme.active)
            .add_modifier(Modifier::BOLD),
    )));

    if activity.active_queries.is_empty() {
        lines.push(Line::from(Span::styled(
            " (none)",
            Style::default().fg(theme.dim),
        )));
    } else {
        for q in activity.active_queries.iter().take(10) {
            let title_short = if q.ref_title.len() > 25 {
                format!("{}...", &q.ref_title[..22])
            } else {
                q.ref_title.clone()
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {:<12}", q.db_name),
                    Style::default().fg(theme.spinner),
                ),
                Span::styled(title_short, Style::default().fg(theme.dim)),
            ]));
        }
    }

    // Summary stats
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Summary",
        Style::default()
            .fg(theme.active)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        format!(" Total completed: {}", activity.total_completed),
        Style::default().fg(theme.text),
    )));

    let panel = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border_style())
                .title(" Activity "),
        )
        .wrap(Wrap { trim: true });

    f.render_widget(panel, area);
}
