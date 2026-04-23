//! Confirmation popup for mark-safe propagation across the queue.
//!
//! Only shown when the sweep would flip refs in at least
//! [`PROPAGATION_CONFIRM_THRESHOLD`] distinct other papers. Below
//! that threshold the sweep fires silently — interrupting the user
//! on every Space press would be noisy, and small sweeps are
//! usually exactly what they want.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::PendingPropagation;
use crate::theme::Theme;

/// Maximum number of affected-paper rows shown inline. Anything
/// beyond this is summarized as `"… + K more"`.
const MAX_LIST_ROWS: usize = 8;

/// Render the dialog as a centered popup. Height grows with the
/// number of affected-paper rows (capped at `MAX_LIST_ROWS` + a
/// few chrome lines).
pub fn render(f: &mut Frame, theme: &Theme, pending: &PendingPropagation) {
    let area = f.area();
    let shown_rows = pending.affected_summary.len().min(MAX_LIST_ROWS);
    let extra = pending.affected_summary.len().saturating_sub(MAX_LIST_ROWS);
    // Chrome: title/border + 2 header lines + blank + list + blank + prompt = ~7 + N
    let height = (7 + shown_rows + if extra > 0 { 1 } else { 0 }).min(area.height as usize) as u16;
    let width = 70.min(area.width);
    let popup = centered_rect(width, height, area);

    let header = Line::from(Span::styled(
        format!(
            "  Mark safe across {} other paper{}?",
            distinct_papers(pending),
            if distinct_papers(pending) == 1 {
                ""
            } else {
                "s"
            }
        ),
        Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
    ));
    let subheader = Line::from(Span::styled(
        format!(
            "  {} refs share this title + author identity",
            pending.total_refs
        ),
        Style::default().fg(theme.dim),
    ));

    let mut lines: Vec<Line<'static>> = vec![Line::from(""), header, subheader, Line::from("")];

    for (fname, count) in pending.affected_summary.iter().take(MAX_LIST_ROWS) {
        lines.push(Line::from(vec![
            Span::styled("  • ", Style::default().fg(theme.dim)),
            Span::styled(
                truncate_middle(fname, width.saturating_sub(12) as usize),
                Style::default().fg(theme.text),
            ),
            Span::styled(
                format!("  ({} ref{})", count, if *count == 1 { "" } else { "s" }),
                Style::default().fg(theme.dim),
            ),
        ]));
    }
    if extra > 0 {
        lines.push(Line::from(Span::styled(
            format!("  … + {extra} more"),
            Style::default().fg(theme.dim),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            "  Space/Enter",
            Style::default()
                .fg(theme.active)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(": apply   ", Style::default().fg(theme.dim)),
        Span::styled(
            "Esc",
            Style::default()
                .fg(theme.not_found)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(": skip (origin only)", Style::default().fg(theme.dim)),
    ]));

    let paragraph = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.active))
            .title(" Propagate mark-safe? "),
    );

    f.render_widget(Clear, popup);
    f.render_widget(paragraph, popup);
}

/// Count of distinct papers in the affected summary. The summary
/// includes same-paper-as-origin siblings, so this differs from the
/// threshold test in `update.rs` (which counts *other* papers).
fn distinct_papers(pending: &PendingPropagation) -> usize {
    pending.affected_summary.len()
}

/// Shrink a long filename with `…` in the middle so the first and
/// last few characters remain visible.
fn truncate_middle(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max < 5 {
        return s.chars().take(max).collect();
    }
    let keep = max - 1; // budget for the ellipsis
    let head = keep / 2;
    let tail = keep - head;
    let chars: Vec<char> = s.chars().collect();
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(chars[chars.len() - tail..].iter());
    out
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .split(area);
    Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .split(vertical[0])[0]
}
