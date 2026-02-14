use crate::app::App;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

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

    // Column header + data rows.
    // Both use identical Span structure with a Unicode indicator char so that
    // ratatui computes the same cell widths and positions columns identically.
    let hdr_style = Style::default().fg(theme.dim).add_modifier(Modifier::BOLD);

    if !activity.db_health.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(" \u{25CB} ", hdr_style), // ○ placeholder indicator
            Span::styled(format!("{:<14} ", "Database"), hdr_style),
            Span::styled(format!("{:>4} ", "Qry"), hdr_style),
            Span::styled(format!("{:>4} ", "Hits"), hdr_style),
            Span::styled(format!("{:>6}", "Avg"), hdr_style),
        ]));
    }

    let mut db_names: Vec<&String> = activity.db_health.keys().collect();
    db_names.sort();

    for name in db_names {
        let health = &activity.db_health[name];
        let indicator = health.indicator();
        let avg = if health.total_queries == 0 {
            "\u{2014}".to_string()
        } else if health.avg_response_ms >= 1000.0 {
            format!("{:.1}s", health.avg_response_ms / 1000.0)
        } else {
            format!("{:.0}ms", health.avg_response_ms)
        };
        let display_name: String = if name.chars().count() > 14 {
            let truncated: String = name.chars().take(13).collect();
            format!("{}\u{2026}", truncated)
        } else {
            name.clone()
        };
        lines.push(Line::from(vec![
            Span::styled(format!(" {} ", indicator), Style::default().fg(theme.text)),
            Span::styled(
                format!("{:<14} ", display_name),
                Style::default().fg(theme.text),
            ),
            Span::styled(
                format!("{:>4} ", health.total_queries),
                Style::default().fg(theme.dim),
            ),
            Span::styled(
                format!("{:>4} ", health.hits),
                Style::default().fg(theme.active),
            ),
            Span::styled(format!("{:>6}", avg), Style::default().fg(theme.dim)),
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
    // Trim sparkline to fit panel width (borders=2, leading space=1)
    let max_spark_chars = area.width.saturating_sub(3) as usize;
    let sparkline: String = sparkline
        .chars()
        .rev()
        .take(max_spark_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
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
            let title_short = if q.ref_title.chars().count() > 25 {
                let truncated: String = q.ref_title.chars().take(22).collect();
                format!("{}...", truncated)
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
    lines.push(Line::from(Span::styled(
        format!(" FPS: {:.0}", app.measured_fps),
        Style::default().fg(theme.dim),
    )));

    // Log messages (archive extraction, errors)
    if !activity.messages.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " Messages",
            Style::default()
                .fg(theme.active)
                .add_modifier(Modifier::BOLD),
        )));
        for (msg, is_warning) in activity.messages.iter().rev().take(5) {
            let color = if *is_warning {
                ratatui::style::Color::Yellow
            } else {
                theme.dim
            };
            lines.push(Line::from(Span::styled(
                format!(" {}", msg),
                Style::default().fg(color),
            )));
        }
    }

    let panel = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border_style())
                .title(" Activity (Tab to hide) "),
        )
        .wrap(Wrap { trim: true });

    f.render_widget(panel, area);
}
