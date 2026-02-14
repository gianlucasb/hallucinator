use std::sync::OnceLock;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

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

// Tips are loaded from tips.txt at compile time and shuffled once at startup.
// Edit tips.txt to add/remove/reorder tips without touching Rust code.
static TIPS_RAW: &str = include_str!("../tips.txt");

/// Parse tips.txt (skip comments and blank lines) and return a shuffled order.
/// The shuffle uses a simple LCG seeded from the process start time so that
/// tip order varies between runs but stays stable within a single session.
pub(crate) fn shuffled_tips() -> &'static [&'static str] {
    static TIPS: OnceLock<Vec<&'static str>> = OnceLock::new();
    TIPS.get_or_init(|| {
        let mut tips: Vec<&str> = TIPS_RAW
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();

        // Fisher-Yates shuffle with a simple LCG PRNG seeded from time
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(42);
        let mut rng = seed;
        for i in (1..tips.len()).rev() {
            // LCG: rng = rng * 6364136223846793005 + 1442695040888963407
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let j = (rng >> 33) as usize % (i + 1);
            tips.swap(i, j);
        }
        tips
    })
}

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
    // logo(3) + blank + tagline + borders(2) + top padding
    let box_h: u16 = if show_logo { 8 } else { 5 };

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
pub fn render_logo_bar(f: &mut Frame, area: Rect, theme: &Theme, tick: usize, fps: u32) -> Rect {
    // For very small terminals, skip entirely
    if area.height < 8 {
        return area;
    }

    // Rotate tips every ~10 seconds
    let tips = shuffled_tips();
    let tip_idx = (tick / (fps as usize * 10).max(1)) % tips.len();
    let tip_text = tips[tip_idx];
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
