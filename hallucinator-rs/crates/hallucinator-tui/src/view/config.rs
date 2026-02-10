use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;
use crate::model::config::{ConfigSection, ConfigState};
use crate::theme::Theme;

/// Render the config screen into the given area.
pub fn render_in(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let config = &app.config_state;

    let chunks = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Min(5),    // content
        Constraint::Length(1), // footer
    ])
    .split(area);

    // Header with section tabs
    let mut header_spans = vec![
        Span::styled(" HALLUCINATOR ", theme.header_style()),
        Span::styled(
            " > Config  ",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
    ];

    for section in ConfigSection::all() {
        let is_active = *section == config.section;
        if is_active {
            header_spans.push(Span::styled(
                format!(" [{}] ", section.label()),
                Style::default()
                    .fg(theme.header_fg)
                    .bg(theme.active)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            header_spans.push(Span::styled(
                format!("  {}  ", section.label()),
                Style::default().fg(theme.dim),
            ));
        }
    }

    f.render_widget(Paragraph::new(Line::from(header_spans)), chunks[0]);

    // Content: only show the current section
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(""));

    match config.section {
        ConfigSection::ApiKeys => {
            render_api_keys(&mut lines, config, theme);
        }
        ConfigSection::Databases => {
            render_databases(&mut lines, config, theme);
        }
        ConfigSection::Concurrency => {
            render_concurrency(&mut lines, config, theme);
        }
        ConfigSection::Display => {
            render_display(&mut lines, config, theme);
        }
    }

    let content = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border_style()),
        )
        .wrap(Wrap { trim: false });

    f.render_widget(content, chunks[1]);

    // Footer — context-aware per section
    let footer_text = if config.editing {
        " Type value, Enter:confirm, Esc:cancel".to_string()
    } else {
        let section_hint = match config.section {
            ConfigSection::ApiKeys => "Enter:edit value",
            ConfigSection::Databases => "Enter:edit/toggle  Space:toggle",
            ConfigSection::Concurrency => "Enter:edit value",
            ConfigSection::Display => "Space/Enter:cycle theme",
        };
        let active_note = if app.processing_started && !app.batch_complete {
            "  \u{26A0} changes apply to next batch"
        } else {
            ""
        };
        format!(
            " j/k:navigate  Tab:section  {}  Esc:back{}",
            section_hint, active_note
        )
    };
    let footer = Line::from(Span::styled(&footer_text, theme.footer_style()));
    f.render_widget(Paragraph::new(footer), chunks[2]);
}

fn render_api_keys(lines: &mut Vec<Line>, config: &ConfigState, theme: &Theme) {
    let items = [
        ("OpenAlex", &config.openalex_key),
        ("Semantic Scholar", &config.s2_api_key),
    ];
    for (i, (label, value)) in items.iter().enumerate() {
        let cursor = if config.item_cursor == i { "> " } else { "  " };
        let display_val = if config.editing && config.item_cursor == i {
            format!("{}\u{2588}", config.edit_buffer)
        } else {
            ConfigState::mask_key(value)
        };
        let val_style = if config.editing && config.item_cursor == i {
            Style::default().fg(theme.active)
        } else {
            Style::default().fg(theme.dim)
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {}{:<20}", cursor, label),
                Style::default().fg(theme.text),
            ),
            Span::styled(display_val, val_style),
        ]));
    }
}

fn render_databases(lines: &mut Vec<Line>, config: &ConfigState, theme: &Theme) {
    // Item 0: DBLP offline path (editable)
    let cursor = if config.item_cursor == 0 { "> " } else { "  " };
    let display_val = if config.editing && config.item_cursor == 0 {
        format!("{}\u{2588}", config.edit_buffer)
    } else if config.dblp_offline_path.is_empty() {
        "(not set)".to_string()
    } else {
        config.dblp_offline_path.clone()
    };
    let val_style = if config.editing && config.item_cursor == 0 {
        Style::default().fg(theme.active)
    } else {
        Style::default().fg(theme.dim)
    };
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {}{:<20}", cursor, "DBLP Offline Path"),
            Style::default().fg(theme.text),
        ),
        Span::styled(display_val, val_style),
    ]));
    lines.push(Line::from(""));

    // Items 1..N: DB toggles
    for (i, (name, enabled)) in config.disabled_dbs.iter().enumerate() {
        let item_idx = i + 1; // offset by 1 for the DBLP path field
        let cursor = if config.item_cursor == item_idx {
            "> "
        } else {
            "  "
        };
        let check = if *enabled { "[\u{2713}]" } else { "[ ]" };
        let style = if *enabled {
            Style::default().fg(theme.verified)
        } else {
            Style::default().fg(theme.dim)
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {}{} ", cursor, check), style),
            Span::styled(name.to_string(), Style::default().fg(theme.text)),
        ]));
    }
}

fn render_concurrency(lines: &mut Vec<Line>, config: &ConfigState, theme: &Theme) {
    let items = [
        (
            "Concurrent Papers",
            config.max_concurrent_papers.to_string(),
        ),
        (
            "Concurrent Refs/Paper",
            config.max_concurrent_refs.to_string(),
        ),
        ("DB Timeout (s)", config.db_timeout_secs.to_string()),
        (
            "Short Timeout (s)",
            config.db_timeout_short_secs.to_string(),
        ),
    ];
    for (i, (label, value)) in items.iter().enumerate() {
        let cursor = if config.item_cursor == i { "> " } else { "  " };
        let display_val = if config.editing && config.item_cursor == i {
            format!("{}\u{2588}", config.edit_buffer)
        } else {
            value.to_string()
        };
        let val_style = if config.editing && config.item_cursor == i {
            Style::default().fg(theme.active)
        } else {
            Style::default().fg(theme.dim)
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {}{:<22}", cursor, label),
                Style::default().fg(theme.text),
            ),
            Span::styled(display_val, val_style),
        ]));
    }
}

fn render_display(lines: &mut Vec<Line>, config: &ConfigState, theme: &Theme) {
    let cursor = if config.item_cursor == 0 { "> " } else { "  " };
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {}Theme: ", cursor),
            Style::default().fg(theme.text),
        ),
        Span::styled(config.theme_name.clone(), Style::default().fg(theme.active)),
        Span::styled("  (Enter to cycle)", Style::default().fg(theme.dim)),
    ]));
}
