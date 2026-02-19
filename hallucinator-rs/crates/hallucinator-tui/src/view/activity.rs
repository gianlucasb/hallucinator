use crate::app::App;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
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

    // Find the max in-flight across all DBs for scaling the bar
    let max_in_flight = activity
        .db_health
        .values()
        .map(|h| h.in_flight)
        .max()
        .unwrap_or(0)
        .max(1);

    if !activity.db_health.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(" \u{25CB} ", hdr_style), // ○ placeholder indicator
            Span::styled(format!("{:<14} ", "Database"), hdr_style),
            Span::styled(format!("{:>4} ", "Qry"), hdr_style),
            Span::styled(format!("{:>4} ", "Hits"), hdr_style),
            Span::styled(format!("{:>6} ", "Avg"), hdr_style),
            Span::styled("Load", hdr_style),
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
        let name_color = if theme.is_t800() {
            theme.dim
        } else {
            theme.text
        };
        let hits_color = if theme.is_t800() {
            Color::White
        } else {
            theme.active
        };

        // Avg column: gray when idle/normal, yellow mild 429 backoff (2-4x), red severe (8x+)
        let backoff = app
            .current_rate_limiters
            .as_ref()
            .map(|rl| rl.backoff_factor(name))
            .unwrap_or(1);
        let avg_color = if health.in_flight == 0 {
            theme.dim
        } else if backoff >= 8 {
            Color::Red
        } else if backoff >= 2 {
            Color::Yellow
        } else {
            theme.dim
        };

        // Build a tiny load bar (4 chars max) + count
        let bar_width: usize = 4;
        let filled = if max_in_flight > 0 && health.in_flight > 0 {
            ((health.in_flight as f64 / max_in_flight as f64) * bar_width as f64).ceil() as usize
        } else {
            0
        };
        let bar_label = if health.in_flight > 0 {
            let bar: String = "\u{2588}".repeat(filled.min(bar_width));
            format!("{}{}", bar, health.in_flight)
        } else {
            String::new()
        };
        let bar_color = if health.in_flight > max_in_flight.max(8) / 2 {
            Color::Yellow
        } else {
            theme.active
        };

        lines.push(Line::from(vec![
            Span::styled(format!(" {} ", indicator), Style::default().fg(name_color)),
            Span::styled(
                format!("{:<14} ", display_name),
                Style::default().fg(name_color),
            ),
            Span::styled(
                format!("{:>4} ", health.total_queries),
                Style::default().fg(theme.dim),
            ),
            Span::styled(
                format!("{:>4} ", health.hits),
                Style::default().fg(hits_color),
            ),
            Span::styled(format!("{:>6} ", avg), Style::default().fg(avg_color)),
            Span::styled(bar_label, Style::default().fg(bar_color)),
        ]));
    }

    // Cache stats row (if a query cache is active)
    if let Some(cache) = &app.current_query_cache {
        let hits = cache.hits();
        let total = hits + cache.misses();
        let cache_color = if theme.is_t800() {
            theme.dim
        } else {
            theme.text
        };
        lines.push(Line::from(vec![
            Span::styled(" \u{229A} ", Style::default().fg(cache_color)), // ⊚
            Span::styled(
                format!("{:<14} ", "Cache"),
                Style::default().fg(cache_color),
            ),
            Span::styled(format!("{:>4} ", total), Style::default().fg(theme.dim)),
            Span::styled(
                format!("{:>4} ", hits),
                Style::default().fg(if theme.is_t800() {
                    Color::White
                } else {
                    theme.active
                }),
            ),
            Span::styled(
                format!(
                    "{:>6} ",
                    if total > 0 {
                        let ms = cache.avg_lookup_ms();
                        if ms >= 1000.0 {
                            format!("{:.1}s", ms / 1000.0)
                        } else {
                            format!("{:.0}ms", ms)
                        }
                    } else {
                        "\u{2014}".to_string()
                    }
                ),
                Style::default().fg(theme.dim),
            ),
        ]));
    }

    if activity.db_health.is_empty() && app.current_query_cache.is_none() {
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
        let spark_color = if theme.is_t800() {
            Color::White
        } else {
            theme.active
        };
        lines.push(Line::from(Span::styled(
            format!(" {}", sparkline),
            Style::default().fg(spark_color),
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
    let num_active = activity.active_queries.len();

    let max_visible = 8;

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        if num_active > 0 {
            format!(" Checking ({})", num_active)
        } else {
            " Checking".to_string()
        },
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
        for q in activity.active_queries.iter().take(max_visible) {
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
        if num_active > max_visible {
            lines.push(Line::from(Span::styled(
                format!(" +{} more...", num_active - max_visible),
                Style::default().fg(theme.dim),
            )));
        }
    }

    // Summary stats
    let num_workers = app.config_state.num_workers;
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Summary",
        Style::default()
            .fg(theme.active)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        format!(" Workers: {}", num_workers),
        Style::default().fg(theme.text),
    )));
    lines.push(Line::from(Span::styled(
        format!(
            " In-flight: {} | Completed: {}",
            num_active, activity.total_completed
        ),
        Style::default().fg(theme.text),
    )));
    // Cache detail
    if let Some(cache) = &app.current_query_cache {
        let (l1_found, l1_nf) = cache.l1_counts();
        let (l2_found, l2_nf) = cache.l2_counts();
        let total = cache.disk_len();
        lines.push(Line::from(Span::styled(
            format!(" Cache  {} entries", total),
            Style::default().fg(theme.dim),
        )));
        lines.push(Line::from(vec![
            Span::styled("   mem:  ", Style::default().fg(theme.dim)),
            Span::styled(format!("{}", l1_found), Style::default().fg(theme.verified)),
            Span::styled(" found  ", Style::default().fg(theme.dim)),
            Span::styled(format!("{}", l1_nf), Style::default().fg(theme.not_found)),
            Span::styled(" not-found", Style::default().fg(theme.dim)),
        ]));
        if cache.has_persistence() {
            lines.push(Line::from(vec![
                Span::styled("   disk: ", Style::default().fg(theme.dim)),
                Span::styled(format!("{}", l2_found), Style::default().fg(theme.verified)),
                Span::styled(" found  ", Style::default().fg(theme.dim)),
                Span::styled(format!("{}", l2_nf), Style::default().fg(theme.not_found)),
                Span::styled(" not-found", Style::default().fg(theme.dim)),
            ]));
        }
    }
    let rss_mb = app.measured_rss_bytes as f64 / (1024.0 * 1024.0);
    lines.push(Line::from(Span::styled(
        format!(" FPS: {:.0}  RSS: {:.1} MB", app.measured_fps, rss_mb),
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
