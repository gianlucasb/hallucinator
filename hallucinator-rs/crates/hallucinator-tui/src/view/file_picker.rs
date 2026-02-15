use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use crate::app::{App, FilePickerContext};
use crate::view::spinner_char;

/// Render the file picker screen into the given area.
pub fn render_in(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let picker = &app.file_picker;
    let is_db_mode = matches!(
        app.file_picker_context,
        FilePickerContext::SelectDatabase { .. }
    );

    let has_extracting = app.extracting_archive.is_some() && !is_db_mode;

    let mut constraints = vec![
        Constraint::Length(1), // header
        Constraint::Length(1), // current dir
        Constraint::Min(5),    // file list
        Constraint::Length(3), // selected summary
    ];
    if has_extracting {
        constraints.push(Constraint::Length(1)); // extracting indicator
    }
    constraints.push(Constraint::Length(1)); // footer

    let chunks = Layout::vertical(constraints).split(area);

    // Header — context-aware
    let header_text =
        if let FilePickerContext::SelectDatabase { config_item } = &app.file_picker_context {
            let db_name = if *config_item == 0 { "DBLP" } else { "ACL" };
            format!(" > Select {} Database (.db / .sqlite)", db_name)
        } else {
            " > Select PDFs / .bbl / .bib / Archives / Results (.json)".to_string()
        };
    let header = Line::from(vec![
        Span::styled(" HALLUCINATOR ", theme.header_style()),
        Span::styled(
            header_text,
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
    ]);
    f.render_widget(Paragraph::new(header), chunks[0]);

    // Current directory
    let dir_display = picker.current_dir.display().to_string();
    let dir_line = Line::from(vec![
        Span::styled(" \u{1F4C1} ", Style::default().fg(theme.active)),
        Span::styled(dir_display, Style::default().fg(theme.dim)),
    ]);
    f.render_widget(Paragraph::new(dir_line), chunks[1]);

    // File list
    let visible_height = chunks[2].height.saturating_sub(2) as usize; // borders
    let scroll_offset = if picker.cursor >= visible_height {
        picker.cursor - visible_height + 1
    } else {
        0
    };

    let items: Vec<ListItem> = picker
        .entries
        .iter()
        .skip(scroll_offset)
        .take(visible_height)
        .map(|entry| {
            let (icon, style) = if entry.is_dir {
                ("\u{1F4C1} ", Style::default().fg(theme.active))
            } else if is_db_mode {
                // In database selection mode: only .db files are selectable
                if entry.is_db {
                    let selected = picker.is_selected(&entry.path);
                    if selected {
                        (
                            "\u{2713} ",
                            Style::default()
                                .fg(theme.verified)
                                .add_modifier(Modifier::BOLD),
                        )
                    } else {
                        ("\u{1F5C3} ", Style::default().fg(theme.text))
                    }
                } else {
                    ("  ", Style::default().fg(theme.dim))
                }
            } else if entry.is_pdf
                || entry.is_bbl
                || entry.is_bib
                || entry.is_archive
                || entry.is_json
            {
                let selected = picker.is_selected(&entry.path);
                if selected {
                    (
                        "\u{2713} ",
                        Style::default()
                            .fg(theme.verified)
                            .add_modifier(Modifier::BOLD),
                    )
                } else if entry.is_json {
                    ("\u{1F4CA} ", Style::default().fg(theme.active))
                } else if entry.is_archive {
                    ("\u{1F4E6} ", Style::default().fg(theme.active))
                } else {
                    ("\u{1F4C4} ", Style::default().fg(theme.text))
                }
            } else {
                ("  ", Style::default().fg(theme.dim))
            };

            ListItem::new(Line::from(vec![
                Span::styled(icon, style),
                Span::styled(&entry.name, style),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border_style())
                .title(" Files "),
        )
        .highlight_style(theme.highlight_style());

    let adjusted_cursor = picker.cursor.saturating_sub(scroll_offset);
    let mut state = ListState::default();
    state.select(Some(adjusted_cursor));
    f.render_stateful_widget(list, chunks[2], &mut state);

    // Selected summary — context-aware
    let selected_count = picker.selected.len();
    let summary_lines = if is_db_mode {
        if selected_count == 0 {
            vec![
                Line::from(Span::styled(
                    "  No database selected",
                    Style::default().fg(theme.dim),
                )),
                Line::from(Span::styled(
                    "  Navigate to a .db or .sqlite file and press Enter to select",
                    Style::default().fg(theme.dim),
                )),
            ]
        } else {
            let name = picker
                .selected
                .first()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                .unwrap_or_default();
            vec![
                Line::from(vec![Span::styled(
                    "  Selected: ",
                    Style::default()
                        .fg(theme.verified)
                        .add_modifier(Modifier::BOLD),
                )]),
                Line::from(Span::styled(
                    format!("  {}", name),
                    Style::default().fg(theme.text),
                )),
            ]
        }
    } else if selected_count == 0 {
        vec![
            Line::from(Span::styled(
                "  No files selected",
                Style::default().fg(theme.dim),
            )),
            Line::from(Span::styled(
                "  Navigate to PDFs, .bbl, .bib, archives, or .json results and press Space to select",
                Style::default().fg(theme.dim),
            )),
        ]
    } else {
        let names: Vec<String> = picker
            .selected
            .iter()
            .map(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| p.display().to_string())
            })
            .collect();
        vec![
            Line::from(vec![Span::styled(
                format!(
                    "  {} file{} selected: ",
                    selected_count,
                    if selected_count == 1 { "" } else { "s" }
                ),
                Style::default()
                    .fg(theme.verified)
                    .add_modifier(Modifier::BOLD),
            )]),
            Line::from(Span::styled(
                format!("  {}", names.join(", ")),
                Style::default().fg(theme.text),
            )),
        ]
    };
    let summary = Paragraph::new(summary_lines).block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(theme.border_style()),
    );
    f.render_widget(summary, chunks[3]);

    // Extracting indicator (only in AddFiles mode)
    let mut footer_idx = 4;
    if has_extracting && let Some(archive_name) = &app.extracting_archive {
        let remaining = app.pending_archive_extractions.len();
        let count_part = if app.extracted_count > 0 {
            format!("{} extracted", app.extracted_count)
        } else {
            String::new()
        };
        let label = if remaining > 1 {
            if count_part.is_empty() {
                format!(
                    " {} Extracting {} ({} more queued)...",
                    spinner_char(app.tick),
                    archive_name,
                    remaining - 1,
                )
            } else {
                format!(
                    " {} Extracting {} ({}, {} more queued)...",
                    spinner_char(app.tick),
                    archive_name,
                    count_part,
                    remaining - 1,
                )
            }
        } else if count_part.is_empty() {
            format!(" {} Extracting {}...", spinner_char(app.tick), archive_name,)
        } else {
            format!(
                " {} Extracting {} ({})...",
                spinner_char(app.tick),
                archive_name,
                count_part,
            )
        };
        let line = Line::from(Span::styled(
            label,
            Style::default()
                .fg(theme.active)
                .add_modifier(Modifier::BOLD),
        ));
        f.render_widget(Paragraph::new(line), chunks[footer_idx]);
        footer_idx += 1;
    }

    // Footer — context-aware
    let footer_text = if is_db_mode {
        " j/k:navigate  Enter:select & confirm  Esc:cancel  ?:help  q:quit"
    } else {
        " j/k:navigate  Space/Enter:select  Enter:open dir  Esc:done  ?:help  q:quit"
    };
    let footer = Line::from(Span::styled(footer_text, theme.footer_style()));
    f.render_widget(Paragraph::new(footer), chunks[footer_idx]);
}
