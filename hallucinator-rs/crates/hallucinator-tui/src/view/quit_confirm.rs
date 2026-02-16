use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::theme::Theme;

/// Render the quit confirmation dialog as a centered popup.
pub fn render(f: &mut Frame, theme: &Theme) {
    let area = f.area();
    let popup = centered_rect(40, 5, area);

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Quit hallucinator?",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled(
                "  q",
                Style::default()
                    .fg(theme.not_found)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(": quit   ", Style::default().fg(theme.dim)),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(theme.active)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(": cancel", Style::default().fg(theme.dim)),
        ]),
    ];

    let paragraph = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.not_found))
            .title(" Confirm Quit "),
    );

    f.render_widget(Clear, popup);
    f.render_widget(paragraph, popup);
}

/// Create a centered rectangle of the given width (columns) and height (rows).
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .split(area);
    Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .split(vertical[0])[0]
}
