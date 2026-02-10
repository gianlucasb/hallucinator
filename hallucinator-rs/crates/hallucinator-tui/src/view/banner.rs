use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::theme::Theme;

// Compact 3-line logo from the design doc
const LOGO: &[&str] = &[
    "░█░█░█▀█░█░░░█░░░█░█░█▀▀░▀█▀░█▀█░█▀█░▀█▀░█▀█░█▀▄",
    "░█▀█░█▀█░█░░░█░░░█░█░█░░░░█░░█░█░█▀█░░█░░█░█░█▀▄",
    "░▀░▀░▀░▀░▀▀▀░▀▀▀░▀▀▀░▀▀▀░▀▀▀░▀░▀░▀░▀░░▀░░▀▀▀░▀░▀",
];

const LOGO_WIDTH: u16 = 48;

// Magnifying glass suffix for persistent logo bar — box-drawing style, 10 chars each
const GLASS: &[&str] = &["  ╭─────╮ ", "  │  ·  │ ", "  ╰─────╯╲"];

const GLASS_WIDTH: u16 = 11;

// 12-stop rainbow palette for the trippy splash effect
const RAINBOW: &[(u8, u8, u8)] = &[
    (255, 0, 0),   // Red
    (255, 127, 0), // Orange
    (255, 255, 0), // Yellow
    (127, 255, 0), // Chartreuse
    (0, 255, 0),   // Green
    (0, 255, 127), // Spring
    (0, 255, 255), // Cyan
    (0, 127, 255), // Azure
    (0, 0, 255),   // Blue
    (127, 0, 255), // Violet
    (255, 0, 255), // Magenta
    (255, 0, 127), // Rose
];

// Tip strings — the "Pro-tip: " prefix is stripped when displayed in the pane
// (the pane header already reads "Pro-tips"), but kept for narrow-terminal fallback.
const TIPS: &[&str] = &[
    "Pro-tip: Press , to open config -- set API keys, concurrency, timeouts",
    "Pro-tip: Set an OpenAlex key for broader coverage (--openalex-key or config)",
    "Pro-tip: Build an offline DBLP database for instant local lookups (--update-dblp)",
    "Pro-tip: Increase concurrent papers in config to process batches faster",
    "Pro-tip: Press Space on a reference to mark false positives as safe",
    "Pro-tip: Use s to cycle sort order, f to filter by status on queue/paper views",
    "Pro-tip: The Semantic Scholar API key removes rate limits (--s2-api-key)",
    "Pro-tip: Press e to export results as Markdown, JSON, or CSV",
    "Pro-tip: References with < 5 words are auto-skipped (too short for reliable matching)",
    "Pro-tip: Press ? anywhere for a full keybinding reference",
];

/// Build a single logo line with flowing rainbow colors.
/// Block characters (█▀▄) get full brightness; light shade (░) gets dimmed
/// for contrast, creating a psychedelic wave that shifts each tick.
fn rainbow_line(text: &str, row: usize, tick: usize) -> Line<'static> {
    let spans: Vec<Span> = text
        .chars()
        .enumerate()
        .map(|(col, ch)| {
            let idx = (col / 2 + row * 3 + tick) % RAINBOW.len();
            let (r, g, b) = RAINBOW[idx];
            let color = if ch == '░' {
                // Dim background shade — still tinted but low brightness
                Color::Rgb(r / 5, g / 5, b / 5)
            } else {
                Color::Rgb(r, g, b)
            };
            Span::styled(
                ch.to_string(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )
        })
        .collect();
    Line::from(spans)
}

/// Render the startup banner as a centered overlay with trippy rainbow logo.
pub fn render(f: &mut Frame, theme: &Theme, tick: usize) {
    let area = f.area();

    // Don't render if terminal too small
    if area.width < 40 || area.height < 10 {
        return;
    }

    let show_logo = area.width >= LOGO_WIDTH + 4;
    let box_w = if show_logo {
        (LOGO_WIDTH + 4).min(area.width)
    } else {
        area.width.min(66)
    };
    // logo(3) + blank + tagline + blank + tip + borders(2) + top padding
    let box_h: u16 = if show_logo { 10 } else { 7 };

    let popup = centered_rect(box_w, box_h, area);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(""));

    if show_logo {
        for (row, art_line) in LOGO.iter().enumerate() {
            lines.push(rainbow_line(art_line, row, tick));
        }
    }

    lines.push(Line::from(""));

    // Tagline
    lines.push(
        Line::from(Span::styled(
            "Finding hallucinated references in academic papers",
            Style::default().fg(theme.text),
        ))
        .alignment(Alignment::Center),
    );

    lines.push(Line::from(""));

    // Rotating tip
    let tip_idx = (tick / 40) % TIPS.len();
    lines.push(
        Line::from(Span::styled(
            TIPS[tip_idx].to_string(),
            Style::default().fg(theme.dim),
        ))
        .alignment(Alignment::Center),
    );

    let paragraph = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.active)),
    );

    f.render_widget(Clear, popup);
    f.render_widget(paragraph, popup);
}

/// Render a persistent logo bar at the top of the screen.
/// Left side: logo + magnifying glass (left-aligned).
/// Right side: bordered "Pro-tips" pane with rotating, word-wrapped tip text.
///
/// Returns the remaining `Rect` below the bar for content.
pub fn render_logo_bar(f: &mut Frame, area: Rect, theme: &Theme, tick: usize) -> Rect {
    // For very small terminals, skip entirely
    if area.height < 8 {
        return area;
    }

    // Rotate tips every ~10 seconds (100 ticks at 10 ticks/sec)
    let tip_idx = (tick / 100) % TIPS.len();
    let tip_text = TIPS[tip_idx];
    // Strip prefix for the pane (header already says "Pro-tips")
    let tip_content = tip_text.strip_prefix("Pro-tip: ").unwrap_or(tip_text);

    let logo_glass_width = LOGO_WIDTH + GLASS_WIDTH;

    // Narrow terminal: just show a 1-line tip
    if area.width < logo_glass_width + 15 {
        let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
        let tip_line = Line::from(Span::styled(
            tip_text.to_string(),
            Style::default().fg(theme.dim),
        ))
        .alignment(Alignment::Center);
        f.render_widget(Paragraph::new(vec![tip_line]), chunks[0]);
        return chunks[1];
    }

    // 5-line bar: logo+glass left, pro-tips pane right
    let rows = Layout::vertical([Constraint::Length(5), Constraint::Min(0)]).split(area);
    let cols = Layout::horizontal([Constraint::Length(logo_glass_width), Constraint::Min(15)])
        .split(rows[0]);

    // ── Left: logo + magnifying glass ──
    let mut logo_lines: Vec<Line> = Vec::new();
    for (i, art_line) in LOGO.iter().enumerate() {
        let glass = GLASS.get(i).copied().unwrap_or("");
        logo_lines.push(Line::from(vec![
            Span::styled(
                art_line.to_string(),
                Style::default()
                    .fg(theme.active)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(glass.to_string(), Style::default().fg(theme.text)),
        ]));
    }
    f.render_widget(Paragraph::new(logo_lines), cols[0]);

    // ── Right: Pro-tips pane ──
    let tip_block = Block::default()
        .title(Line::from(Span::styled(
            " Pro-tips ",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        )))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));

    let tip_para = Paragraph::new(tip_content.to_string())
        .style(Style::default().fg(theme.dim))
        .block(tip_block)
        .wrap(Wrap { trim: true });

    f.render_widget(tip_para, cols[1]);

    rows[1]
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .split(area);
    Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .split(vertical[0])[0]
}
