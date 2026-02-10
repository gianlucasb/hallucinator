use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::App;

/// Export format options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Json,
    Csv,
    Markdown,
    Text,
}

impl ExportFormat {
    pub fn all() -> &'static [ExportFormat] {
        &[
            ExportFormat::Json,
            ExportFormat::Csv,
            ExportFormat::Markdown,
            ExportFormat::Text,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Json => "JSON",
            Self::Csv => "CSV",
            Self::Markdown => "Markdown",
            Self::Text => "Plain Text",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Csv => "csv",
            Self::Markdown => "md",
            Self::Text => "txt",
        }
    }
}

/// Scope of export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportScope {
    ThisPaper,
    AllPapers,
}

impl ExportScope {
    pub fn label(self) -> &'static str {
        match self {
            Self::ThisPaper => "This paper",
            Self::AllPapers => "All papers",
        }
    }
}

/// State for the export modal.
#[derive(Debug, Clone)]
pub struct ExportState {
    pub active: bool,
    pub format: ExportFormat,
    pub scope: ExportScope,
    pub output_path: String,
    pub cursor: usize, // 0=format, 1=scope, 2=path, 3=confirm
    pub message: Option<String>,
}

impl Default for ExportState {
    fn default() -> Self {
        Self {
            active: false,
            format: ExportFormat::Json,
            scope: ExportScope::AllPapers,
            output_path: "hallucinator-results".to_string(),
            cursor: 0,
            message: None,
        }
    }
}

/// Render the export modal overlay.
pub fn render(f: &mut Frame, app: &App) {
    let theme = &app.theme;
    let export = &app.export_state;
    let area = f.area();
    let popup = centered_rect(50, 14, area);

    let mut lines = vec![
        Line::from(Span::styled(
            " Export Results ",
            Style::default()
                .fg(theme.header_fg)
                .bg(theme.header_bg)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    // Format
    let fmt_indicator = if export.cursor == 0 { "> " } else { "  " };
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {}Format:  ", fmt_indicator),
            Style::default().fg(theme.text),
        ),
        Span::styled(export.format.label(), Style::default().fg(theme.active)),
    ]));

    // Scope
    let scope_indicator = if export.cursor == 1 { "> " } else { "  " };
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {}Scope:   ", scope_indicator),
            Style::default().fg(theme.text),
        ),
        Span::styled(export.scope.label(), Style::default().fg(theme.active)),
    ]));

    // Output path
    let path_indicator = if export.cursor == 2 { "> " } else { "  " };
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {}Output:  ", path_indicator),
            Style::default().fg(theme.text),
        ),
        Span::styled(
            format!("{}.{}", export.output_path, export.format.extension()),
            Style::default().fg(theme.dim),
        ),
    ]));

    lines.push(Line::from(""));

    // Confirm button
    let confirm_style = if export.cursor == 3 {
        Style::default()
            .fg(theme.header_fg)
            .bg(theme.active)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.active)
    };
    lines.push(Line::from(vec![
        Span::styled("          ", Style::default()),
        Span::styled(" Export ", confirm_style),
    ]));

    if let Some(msg) = &export.message {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  {}", msg),
            Style::default().fg(theme.verified),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  j/k:navigate  Enter:select/cycle  Esc:cancel",
        Style::default().fg(theme.dim),
    )));

    let paragraph = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.active))
            .title(" Export "),
    );

    f.render_widget(Clear, popup);
    f.render_widget(paragraph, popup);
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .split(area);
    Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .split(vertical[0])[0]
}
