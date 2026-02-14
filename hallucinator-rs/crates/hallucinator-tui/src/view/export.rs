use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::App;

/// Export format options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Json,
    Csv,
    Markdown,
    Text,
    Html,
}

impl ExportFormat {
    pub fn all() -> &'static [ExportFormat] {
        &[
            ExportFormat::Json,
            ExportFormat::Csv,
            ExportFormat::Markdown,
            ExportFormat::Text,
            ExportFormat::Html,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Json => "JSON",
            Self::Csv => "CSV",
            Self::Markdown => "Markdown",
            Self::Text => "Plain Text",
            Self::Html => "HTML",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Csv => "csv",
            Self::Markdown => "md",
            Self::Text => "txt",
            Self::Html => "html",
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
    pub editing_path: bool,
    pub edit_buffer: String,
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
            editing_path: false,
            edit_buffer: String::new(),
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
    let path_display = if export.editing_path {
        format!("{}\u{2588}", export.edit_buffer)
    } else {
        export.output_path.clone()
    };
    let path_style = if export.editing_path {
        Style::default().fg(theme.active)
    } else {
        Style::default().fg(theme.dim)
    };
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {}Output:  ", path_indicator),
            Style::default().fg(theme.text),
        ),
        Span::styled(path_display, path_style),
        Span::styled(
            format!(".{}", export.format.extension()),
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
    let hint = if export.editing_path {
        "  Type filename, Enter:confirm, Esc:cancel"
    } else {
        "  j/k:navigate  Enter:select/cycle  Esc:cancel"
    };
    lines.push(Line::from(Span::styled(
        hint,
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
