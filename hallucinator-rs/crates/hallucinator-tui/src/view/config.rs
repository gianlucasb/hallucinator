use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::App;
use crate::model::config::{ConfigSection, ConfigState};
use crate::theme::Theme;

/// Render the config screen into the given area.
/// `footer_area` is a full-width row below the main content + activity panel.
pub fn render_in(f: &mut Frame, app: &App, area: Rect, footer_area: Rect) {
    let theme = &app.theme;
    let config = &app.config_state;

    let chunks = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Length(1), // config file path
        Constraint::Min(5),    // content
    ])
    .split(area);

    // Header with section tabs
    let mut header_spans = vec![
        Span::styled(" Config ", theme.header_style()),
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

    // Config file path hint
    let path_text = crate::config_file::config_path()
        .map(|p| format!("  Config: {}", p.display()))
        .unwrap_or_else(|| "  Config: (no config directory)".to_string());
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            path_text,
            Style::default().fg(theme.dim),
        ))),
        chunks[1],
    );

    // Content: only show the current section
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(""));

    match config.section {
        ConfigSection::ApiKeys => {
            render_api_keys(&mut lines, config, theme);
        }
        ConfigSection::Databases => {
            render_databases(&mut lines, config, theme, area.width);
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

    f.render_widget(content, chunks[2]);

    // Footer — context-aware per section
    let footer_text = if config.confirm_exit {
        " Unsaved config changes. Save before leaving?  y:save  n:discard  Esc:cancel".to_string()
    } else if config.editing {
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
            " j/k:navigate  Tab:section  {}  Ctrl+S:save  Esc:back{}",
            section_hint, active_note
        )
    };
    let footer_style = if config.confirm_exit {
        Style::default()
            .fg(theme.not_found)
            .add_modifier(Modifier::BOLD)
    } else {
        theme.footer_style()
    };
    let footer = Line::from(Span::styled(&footer_text, footer_style));
    f.render_widget(Paragraph::new(footer), footer_area);
}

fn render_api_keys(lines: &mut Vec<Line>, config: &ConfigState, theme: &Theme) {
    let items: Vec<(&str, String)> = vec![
        ("OpenAlex", ConfigState::mask_key(&config.openalex_key)),
        (
            "Semantic Scholar",
            ConfigState::mask_key(&config.s2_api_key),
        ),
        (
            "CrossRef Mailto",
            if config.crossref_mailto.is_empty() {
                "(not set)".to_string()
            } else {
                config.crossref_mailto.clone()
            },
        ),
    ];
    for (i, (label, display_default)) in items.iter().enumerate() {
        let cursor = if config.item_cursor == i { "> " } else { "  " };
        let display_val = if config.editing && config.item_cursor == i {
            format!("{}\u{2588}", config.edit_buffer)
        } else {
            display_default.clone()
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

fn render_databases(lines: &mut Vec<Line>, config: &ConfigState, theme: &Theme, width: u16) {
    // Explanation text
    lines.push(Line::from(Span::styled(
        "  Offline databases speed up validation by avoiding network requests to DBLP",
        Style::default().fg(theme.dim),
    )));
    lines.push(Line::from(Span::styled(
        "  and ACL Anthology. Press b to download and build a database, or set a path",
        Style::default().fg(theme.dim),
    )));
    lines.push(Line::from(Span::styled(
        "  to an existing one. Toggle individual sources on/off below.",
        Style::default().fg(theme.dim),
    )));
    lines.push(Line::from(""));

    // Label prefix is 24 chars ("  > " + padded label), hints are ~23 chars,
    // plus 2 for border. Calculate max path display width from terminal width.
    let label_width: usize = 24;
    let hint_width: usize = 23; // "  (o:browse  b:build)"
    let border: usize = 2;
    let max_path_len = (width as usize).saturating_sub(label_width + hint_width + border);

    // Item 0: DBLP offline path (editable)
    let cursor = if config.item_cursor == 0 { "> " } else { "  " };
    let display_val = if config.editing && config.item_cursor == 0 {
        format!("{}\u{2588}", config.edit_buffer)
    } else if config.dblp_offline_path.is_empty() {
        "(not set)".to_string()
    } else {
        truncate_path(&config.dblp_offline_path, max_path_len)
    };
    let val_style = if config.editing && config.item_cursor == 0 {
        Style::default().fg(theme.active)
    } else {
        Style::default().fg(theme.dim)
    };
    let mut spans = vec![
        Span::styled(
            format!("  {}{:<20}", cursor, "DBLP Offline Path"),
            Style::default().fg(theme.text),
        ),
        Span::styled(display_val, val_style),
    ];
    if config.item_cursor == 0 && !config.editing {
        spans.push(Span::styled(
            "  (o:browse  b:build)",
            Style::default().fg(theme.dim),
        ));
    }
    lines.push(Line::from(spans));

    // Show DBLP build status inline
    if let Some(ref status) = config.dblp_build_status {
        let style = if config.dblp_building {
            Style::default().fg(theme.active)
        } else if status.starts_with("Failed") {
            Style::default().fg(theme.not_found)
        } else {
            Style::default().fg(theme.verified)
        };
        lines.push(Line::from(Span::styled(format!("      {}", status), style)));
    }

    // Item 1: ACL offline path (editable)
    let cursor = if config.item_cursor == 1 { "> " } else { "  " };
    let display_val = if config.editing && config.item_cursor == 1 {
        format!("{}\u{2588}", config.edit_buffer)
    } else if config.acl_offline_path.is_empty() {
        "(not set)".to_string()
    } else {
        truncate_path(&config.acl_offline_path, max_path_len)
    };
    let val_style = if config.editing && config.item_cursor == 1 {
        Style::default().fg(theme.active)
    } else {
        Style::default().fg(theme.dim)
    };
    let mut spans = vec![
        Span::styled(
            format!("  {}{:<20}", cursor, "ACL Offline Path"),
            Style::default().fg(theme.text),
        ),
        Span::styled(display_val, val_style),
    ];
    if config.item_cursor == 1 && !config.editing {
        spans.push(Span::styled(
            "  (o:browse  b:build)",
            Style::default().fg(theme.dim),
        ));
    }
    lines.push(Line::from(spans));

    // Show ACL build status inline
    if let Some(ref status) = config.acl_build_status {
        let style = if config.acl_building {
            Style::default().fg(theme.active)
        } else if status.starts_with("Failed") {
            Style::default().fg(theme.not_found)
        } else {
            Style::default().fg(theme.verified)
        };
        lines.push(Line::from(Span::styled(format!("      {}", status), style)));
    }

    lines.push(Line::from(""));

    // Items 2..N: DB toggles
    for (i, (name, enabled)) in config.disabled_dbs.iter().enumerate() {
        let item_idx = i + 2; // offset by 2 for the DBLP + ACL path fields
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
    let archive_limit = if config.max_archive_size_mb == 0 {
        "unlimited".to_string()
    } else {
        format!("{}", config.max_archive_size_mb)
    };
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
        ("Archive Size Limit (MB)", archive_limit),
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
    // Item 0: Theme
    let cursor = if config.item_cursor == 0 { "> " } else { "  " };
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {}{:<22}", cursor, "Theme"),
            Style::default().fg(theme.text),
        ),
        Span::styled(config.theme_name.clone(), Style::default().fg(theme.active)),
        Span::styled("  (Enter to cycle)", Style::default().fg(theme.dim)),
    ]));

    // Item 1: FPS
    let cursor = if config.item_cursor == 1 { "> " } else { "  " };
    let display_val = if config.editing && config.item_cursor == 1 {
        format!("{}\u{2588}", config.edit_buffer)
    } else {
        config.fps.to_string()
    };
    let val_style = if config.editing && config.item_cursor == 1 {
        Style::default().fg(theme.active)
    } else {
        Style::default().fg(theme.dim)
    };
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {}{:<22}", cursor, "FPS"),
            Style::default().fg(theme.text),
        ),
        Span::styled(display_val, val_style),
    ]));
}

/// Truncate a path string for display. If longer than `max_len`, show `...` + the tail.
fn truncate_path(path: &str, max_len: usize) -> String {
    if max_len < 8 {
        // Too narrow to truncate usefully — just show the filename
        return std::path::Path::new(path)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string());
    }
    if path.len() <= max_len {
        return path.to_string();
    }
    let tail = &path[path.len() - (max_len - 3)..];
    format!("...{}", tail)
}
