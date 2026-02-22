use std::sync::OnceLock;
use std::time::Duration;

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

// Targeting reticle for T-800 logo bar
const RETICLE: &[&str] = &["  ┌─ ─ ─┐ ", "  │  +  │ ", "  └─ ─ ─┘ "];

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

// Glitch characters used during the corruption phase
const GLITCH_CHARS: &[char] = &['▓', '█', '░', '▒', '╳', '▄', '▀', '■'];

// Boot diagnostic lines — grounded in what the tool actually does
const DIAGNOSTICS: &[(&str, &str)] = &[
    ("CITATION ANALYSIS ENGINE", "LOADED"),
    ("FUZZY MATCH THRESHOLD", "95%"),
    ("CONCURRENT VALIDATORS", "10"),
    ("SLOP TARGETING", "ONLINE"),
];

// Targeting brackets for the splash screen — single-line while seeking, double-line on lock
const BRACKET_SEEK: &[&str] = &["┌──    ──┐", "    ·     ", "└──    ──┘"];
const BRACKET_LOCK: &[&str] = &["╔══    ══╗", "    ·     ", "╚══    ══╝"];
const CROSSHAIR_WIDTH: u16 = 10;
const CROSSHAIR_CENTER_COL: u16 = 4; // column of the '·' within brackets

/// Seeker crosshair phase for T-800 splash.
#[derive(Clone, Copy, PartialEq)]
pub enum SeekerPhase {
    Seeking,
    Locked,
}

/// Labels displayed when the crosshair locks onto a target.
const SEEKER_LABELS: &[&str] = &[
    "MATCH: 0%",
    "STATUS: NOT FOUND",
    "REF: UNVERIFIED",
    "SCANNING...",
    "NO DATA",
    "HALLUCINATION?",
    "CHECKING...",
    "SLOP DETECTED",
    "CITATION NEEDED",
    "DOI: INVALID",
    "TRUST: LOW",
    "FABRICATED?",
    "SOURCE: MISSING",
    "CONFIDENCE: 12%",
    "AUTHOR MISMATCH",
    "ARXIV: 404",
    "SUSPECT REF",
    "HUMAN SUPERVISION: MINIMAL",
    "DESK REJECT",
];

/// Persistent state for the T-800 seeking crosshair animation.
pub struct T800Splash {
    cx: f32,
    cy: f32,
    tx: f32,
    ty: f32,
    pub phase: SeekerPhase,
    lock_timer: u32,
    label_idx: usize,
    lock_hit: bool, // true = label shown + double brackets; false = false alarm
    rng: u64,
}

impl T800Splash {
    pub fn new(width: u16, height: u16) -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(42);
        let mut rng = seed;
        let w = (width as f32).max(4.0);
        let h = (height as f32).max(4.0);

        // Sprite-aware safe range so render clamp never fires
        let min_x = CROSSHAIR_CENTER_COL as f32;
        let max_x = (w - (CROSSHAIR_WIDTH - CROSSHAIR_CENTER_COL) as f32).max(min_x);
        let min_y = 2.0_f32;
        let max_y = (h - 3.0).max(min_y);

        rng = rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let cx = ((rng >> 33) as f32 % w).clamp(min_x, max_x);
        rng = rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let cy = ((rng >> 33) as f32 % h).clamp(min_y, max_y);
        rng = rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let tx = ((rng >> 33) as f32 % w).clamp(min_x, max_x);
        rng = rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let ty = ((rng >> 33) as f32 % h).clamp(min_y, max_y);

        Self {
            cx,
            cy,
            tx,
            ty,
            phase: SeekerPhase::Seeking,
            lock_timer: 0,
            label_idx: 0,
            lock_hit: false,
            rng,
        }
    }

    /// Advance the seeker state machine by one frame.
    pub fn tick(&mut self, width: u16, height: u16) {
        let w = (width as f32).max(1.0);
        let h = (height as f32).max(1.0);

        // Sprite-aware safe range so render clamp never fires
        let min_x = CROSSHAIR_CENTER_COL as f32;
        let max_x = (w - (CROSSHAIR_WIDTH - CROSSHAIR_CENTER_COL) as f32).max(min_x);
        let min_y = 2.0_f32;
        let max_y = (h - 3.0).max(min_y);

        // Re-clamp stale targets (from new() or terminal resize)
        self.tx = self.tx.clamp(min_x, max_x);
        self.ty = self.ty.clamp(min_y, max_y);

        match self.phase {
            SeekerPhase::Seeking => {
                let dx = self.tx - self.cx;
                let dy = self.ty - self.cy;
                let dist = (dx * dx + dy * dy).sqrt();
                if dist < 0.5 {
                    self.phase = SeekerPhase::Locked;
                    // ~60% chance of a "hit" (label + double brackets)
                    self.rng = self
                        .rng
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    self.lock_hit = (self.rng >> 33) % 5 < 3;
                    self.lock_timer = if self.lock_hit { 20 } else { 8 };
                } else {
                    let step = 1.2_f32.min(dist);
                    self.cx += dx / dist * step;
                    self.cy += dy / dist * step;
                }
            }
            SeekerPhase::Locked => {
                if self.lock_timer > 0 {
                    self.lock_timer -= 1;
                } else {
                    // ~25% chance of a long-range snap, otherwise scan nearby
                    self.rng = self
                        .rng
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    let far = (self.rng >> 33).is_multiple_of(4);
                    self.rng = self
                        .rng
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    if far {
                        self.tx = ((self.rng >> 33) as f32 % w).clamp(min_x, max_x);
                    } else {
                        let rx = (w * 0.3).max(4.0);
                        let ox = ((self.rng >> 33) as f32 % (rx * 2.0)) - rx;
                        self.tx = (self.cx + ox).clamp(min_x, max_x);
                    }
                    self.rng = self
                        .rng
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    if far {
                        self.ty = ((self.rng >> 33) as f32 % h).clamp(min_y, max_y);
                    } else {
                        let ry = (h * 0.3).max(3.0);
                        let oy = ((self.rng >> 33) as f32 % (ry * 2.0)) - ry;
                        self.ty = (self.cy + oy).clamp(min_y, max_y);
                    }
                    self.rng = self
                        .rng
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    self.label_idx = (self.rng >> 33) as usize % SEEKER_LABELS.len();
                    self.phase = SeekerPhase::Seeking;
                }
            }
        }

        self.cx = self.cx.clamp(min_x, max_x);
        self.cy = self.cy.clamp(min_y, max_y);
    }
}

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

/// Build a logo line that progressively corrupts: rainbow shifts toward red,
/// random chars replaced with glitch blocks. `intensity` ranges 0.0–1.0.
fn glitch_line(text: &str, row: usize, tick: usize, intensity: f32) -> Line<'static> {
    // Deterministic pseudo-random per character based on position + tick
    let mut rng = (row as u64)
        .wrapping_mul(6364136223846793005)
        .wrapping_add(tick as u64)
        .wrapping_mul(1442695040888963407);

    let spans: Vec<Span> = text
        .chars()
        .enumerate()
        .map(|(col, ch)| {
            // Advance PRNG per character
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(col as u64);
            let rand_val = ((rng >> 33) & 0xFF) as f32 / 255.0;

            if rand_val < intensity {
                // Replace with glitch character
                let glitch_idx = ((rng >> 40) as usize) % GLITCH_CHARS.len();
                let glitch_ch = GLITCH_CHARS[glitch_idx];

                // Color shifts toward red as intensity increases
                let red = 180 + ((rng >> 48) % 76) as u8;
                let other = (40.0 * (1.0 - intensity)) as u8;
                Span::styled(
                    glitch_ch.to_string(),
                    Style::default()
                        .fg(Color::Rgb(red, other, other))
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                // Normal rainbow but increasingly red-shifted
                let idx = (col / 2 + row * 3 + tick) % RAINBOW.len();
                let (r, g, b) = RAINBOW[idx];
                // Blend toward red: reduce green/blue by intensity
                let keep = 1.0 - intensity * 0.8;
                let g_adj = (g as f32 * keep) as u8;
                let b_adj = (b as f32 * keep) as u8;
                let r_adj = r.max((r as f32 + intensity * 60.0).min(255.0) as u8);
                let color = if ch == '░' {
                    Color::Rgb(r_adj / 5, g_adj / 5, b_adj / 5)
                } else {
                    Color::Rgb(r_adj, g_adj, b_adj)
                };
                Span::styled(
                    ch.to_string(),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                )
            }
        })
        .collect();
    Line::from(spans)
}

/// Scramble tagline text character by character based on intensity.
fn glitch_tagline(text: &str, tick: usize, intensity: f32) -> Line<'static> {
    let hex_chars = b"0123456789ABCDEF";
    let mut rng = (tick as u64).wrapping_mul(2862933555777941757);
    let spans: Vec<Span> = text
        .chars()
        .enumerate()
        .map(|(i, ch)| {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(i as u64);
            let rand_val = ((rng >> 33) & 0xFF) as f32 / 255.0;

            if ch == ' ' || rand_val >= intensity {
                Span::styled(
                    ch.to_string(),
                    Style::default().fg(Color::Rgb(
                        200 - (intensity * 100.0) as u8,
                        200 - (intensity * 180.0) as u8,
                        200 - (intensity * 180.0) as u8,
                    )),
                )
            } else {
                let hex_idx = ((rng >> 40) as usize) % hex_chars.len();
                Span::styled(
                    (hex_chars[hex_idx] as char).to_string(),
                    Style::default().fg(Color::Rgb(150 + ((rng >> 48) % 106) as u8, 0, 0)),
                )
            }
        })
        .collect();
    Line::from(spans).alignment(Alignment::Center)
}

/// Render the T-800 boot sequence splash.
///
/// Phase 1 (0.0–0.75s): Normal rainbow logo + tagline (small centered popup)
/// Phase 2 (0.75–1.75s): Glitch corruption (small centered popup)
/// Phase 3 (1.75–3.75s): Centered HUD boot — diagnostics cascade, logo appears
/// Phase 4 (3.75s+): Centered HUD — logo, seeking crosshair, pulsing prompt
fn render_t800(
    f: &mut Frame,
    theme: &Theme,
    tick: usize,
    elapsed: Duration,
    splash: Option<&mut T800Splash>,
) {
    let area = f.area();
    if area.width < 40 || area.height < 10 {
        return;
    }

    let ms = elapsed.as_millis() as u64;
    let show_logo = area.width >= LOGO_WIDTH + 4;
    let tagline = "Finding hallucinated references in academic papers";

    // ── Phases 1–2: Small centered popup (normal → glitch) ──
    if ms < 750 {
        let box_w = if show_logo {
            (LOGO_WIDTH + 4).min(area.width)
        } else {
            area.width.min(66)
        };
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
        lines.push(
            Line::from(Span::styled(tagline, Style::default().fg(theme.text)))
                .alignment(Alignment::Center),
        );

        let paragraph = Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        );
        f.render_widget(Clear, popup);
        f.render_widget(paragraph, popup);
        return;
    }

    if ms < 1750 {
        let intensity = (ms - 750) as f32 / 1000.0;

        let box_w = if show_logo {
            (LOGO_WIDTH + 4).min(area.width)
        } else {
            area.width.min(66)
        };
        let box_h: u16 = if show_logo { 8 } else { 5 };
        let popup = centered_rect(box_w, box_h, area);

        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(""));
        if show_logo {
            for (row, art_line) in LOGO.iter().enumerate() {
                lines.push(glitch_line(art_line, row, tick, intensity));
            }
        }
        lines.push(Line::from(""));
        lines.push(glitch_tagline(tagline, tick, intensity));

        let border_r = (60.0 + intensity * 195.0) as u8;
        let border_g = ((1.0 - intensity) * 140.0) as u8;
        let border_b = ((1.0 - intensity) * 255.0) as u8;
        let paragraph = Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(border_r, border_g, border_b))),
        );
        f.render_widget(Clear, popup);
        f.render_widget(paragraph, popup);
        return;
    }

    // ── Phases 3–4: Centered HUD container ──

    f.render_widget(Clear, area);

    let boot_ms = ms.saturating_sub(1750);
    let in_phase4 = ms >= 3750;
    // 0.5s fade-in so the HUD doesn't pop in abruptly after the glitch
    let fade = (boot_ms as f32 / 500.0).min(1.0);

    let box_w = area.width.saturating_sub(4).min(68);
    let box_h = area.height.saturating_sub(2).min(16);
    let container = centered_rect(box_w, box_h, area);
    let hud = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb((100.0 * fade) as u8, 0, 0)));
    let inner = hud.inner(container);
    f.render_widget(hud, container);

    // All remaining rendering via direct buffer writes for precise positioning
    let buf = f.buffer_mut();
    let iw = inner.width as usize;

    // Scale an RGB color by the HUD fade-in factor
    let fc = |r: u8, g: u8, b: u8| -> Color {
        Color::Rgb(
            (r as f32 * fade) as u8,
            (g as f32 * fade) as u8,
            (b as f32 * fade) as u8,
        )
    };

    // ── Background hex noise: sparse random 0xABCD at random positions ──
    {
        let mut hex_rng = (tick as u64).wrapping_mul(2862933555777941757);
        for _ in 0..6 {
            hex_rng = hex_rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let hx = inner.x + ((hex_rng >> 33) as u16 % inner.width.saturating_sub(7).max(1));
            hex_rng = hex_rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let hy = inner.y + ((hex_rng >> 33) as u16 % inner.height.max(1));
            let val = (hex_rng >> 48) & 0xFFFF;
            let hex_str = format!("0x{:04X}", val);
            buf.set_string(hx, hy, &hex_str, Style::default().fg(fc(50, 25, 25)));
        }
    }

    // ── Scan line: subtle red glow sweeping down ──
    let scan_period = (inner.height as usize).max(1) * 3;
    let scan_row = (tick % scan_period) as u16;
    if scan_row < inner.height {
        let sy = inner.y + scan_row;
        for col in inner.x..inner.x + inner.width {
            if let Some(cell) = buf.cell_mut((col, sy)) {
                cell.set_bg(fc(20, 0, 0));
            }
        }
    }

    // ── Margin: hex address top-left ──
    let hex_tl = format!("0x{:04X}", (tick.wrapping_mul(0x1337) >> 4) & 0xFFFF);
    buf.set_string(
        inner.x + 1,
        inner.y,
        &hex_tl,
        Style::default().fg(fc(100, 60, 60)),
    );

    // ── Margin: status label top-right (white) ──
    let status_label = if in_phase4 { "SYS.READY" } else { "SYS.INIT" };
    if iw > status_label.len() + 2 {
        let lx = inner.x + inner.width - status_label.len() as u16 - 1;
        buf.set_string(
            lx,
            inner.y,
            status_label,
            Style::default()
                .fg(fc(255, 255, 255))
                .add_modifier(Modifier::BOLD),
        );
    }

    // ── Margin: hex address bottom-left ──
    let bottom_y = inner.y + inner.height.saturating_sub(1);
    let hex_bl = format!("0x{:04X}", (tick.wrapping_mul(0xBEEF) >> 8) & 0xFFFF);
    buf.set_string(
        inner.x + 1,
        bottom_y,
        &hex_bl,
        Style::default().fg(fc(100, 60, 60)),
    );

    // ── Margin: version bottom-right ──
    let version = "HALLUCINATOR v0.1.1";
    if iw > version.len() + 2 {
        let vx = inner.x + inner.width - version.len() as u16 - 1;
        buf.set_string(vx, bottom_y, version, Style::default().fg(fc(100, 50, 50)));
    }

    // ── Logo (top of HUD, appears immediately on phase 3 entry) ──
    let logo_y = inner.y + 2;
    if show_logo && logo_y + 3 < inner.y + inner.height {
        let logo_x = inner.x + inner.width.saturating_sub(LOGO_WIDTH) / 2;
        for (row, art_line) in LOGO.iter().enumerate() {
            let y = logo_y + row as u16;
            for (col, ch) in art_line.chars().enumerate() {
                let color = if ch == '░' {
                    fc(40, 0, 0)
                } else {
                    fc(255, 0, 0)
                };
                buf.set_string(
                    logo_x + col as u16,
                    y,
                    ch.to_string(),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                );
            }
        }
    }

    // ── Diagnostics (cascade below logo) ──
    let diag_start_y = logo_y + LOGO.len() as u16 + 1;
    let spinner_frames: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    let diag_col = inner.x + 3;
    let dot_width: usize = 30;

    for (i, (label, value)) in DIAGNOSTICS.iter().enumerate() {
        let line_start_ms = i as u64 * 400;
        if boot_ms < line_start_ms {
            break;
        }

        let y = diag_start_y + i as u16;
        if y >= inner.y + inner.height.saturating_sub(1) {
            break;
        }

        let line_elapsed = boot_ms - line_start_ms;

        // Hex prefix
        let hex_prefix = format!("0x{:04X}  ", i * 0x10);
        buf.set_string(
            diag_col,
            y,
            &hex_prefix,
            Style::default().fg(fc(100, 60, 60)),
        );

        // Label + dots
        let pad = dot_width.saturating_sub(label.len());
        let label_dots = format!("{}{} ", label, ".".repeat(pad));
        let label_x = diag_col + hex_prefix.len() as u16;
        buf.set_string(label_x, y, &label_dots, Style::default().fg(fc(160, 0, 0)));

        // Value (spinner while resolving, white when done)
        let value_x = label_x + label_dots.len() as u16;
        if line_elapsed < 400 {
            let spin = spinner_frames[tick % spinner_frames.len()];
            buf.set_string(
                value_x,
                y,
                spin.to_string(),
                Style::default()
                    .fg(fc(255, 0, 0))
                    .add_modifier(Modifier::BOLD),
            );
        } else {
            buf.set_string(
                value_x,
                y,
                value,
                Style::default()
                    .fg(fc(255, 255, 255))
                    .add_modifier(Modifier::BOLD),
            );
        }
    }

    // ── Phase 4: Seeking crosshair + prompt ──
    if in_phase4 {
        // Pulsing prompt first (lower z) so reticle draws on top
        let prompt_y = inner.y + inner.height.saturating_sub(2);
        if prompt_y > inner.y + 1 {
            let pulse = (ms / 500).is_multiple_of(2);
            let prompt_color = if pulse {
                Color::Rgb(255, 0, 0)
            } else {
                Color::Rgb(140, 0, 0)
            };
            let prompt = "> PRESS ENTER TO INITIATE SCAN";
            let px = inner.x + inner.width.saturating_sub(prompt.len() as u16) / 2;
            buf.set_string(
                px,
                prompt_y,
                prompt,
                Style::default()
                    .fg(prompt_color)
                    .add_modifier(Modifier::BOLD),
            );
        }

        if let Some(splash) = splash {
            // Tick with real inner dimensions so targets stay in visible bounds
            splash.tick(inner.width, inner.height);
            // Map seeker grid coords to screen coords, centering '·' on position
            let raw_x = inner.x as i16 + splash.cx as i16 - CROSSHAIR_CENTER_COL as i16;
            let raw_y = inner.y as i16 + splash.cy as i16 - 1;
            let cx = raw_x.clamp(
                inner.x as i16,
                (inner.x + inner.width).saturating_sub(CROSSHAIR_WIDTH) as i16,
            ) as u16;
            let cy = raw_y.clamp(
                inner.y as i16 + 1,
                (inner.y + inner.height).saturating_sub(4) as i16,
            ) as u16;

            let is_locked = splash.phase == SeekerPhase::Locked;
            let is_hit = is_locked && splash.lock_hit;
            let bracket = if is_hit { &BRACKET_LOCK } else { &BRACKET_SEEK };

            let bracket_style = Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD);

            for (row, line) in bracket.iter().enumerate() {
                let y = cy + row as u16;
                if y >= inner.y + inner.height.saturating_sub(1) {
                    break;
                }
                for (col, ch) in line.chars().enumerate() {
                    if ch == ' ' {
                        continue;
                    }
                    let x = cx + col as u16;
                    if x >= inner.x + inner.width {
                        break;
                    }
                    buf.set_string(x, y, ch.to_string(), bracket_style);
                }
            }

            // Label next to brackets only on a hit — if it won't fit, downgrade to no-hit
            if is_hit {
                let label = SEEKER_LABELS[splash.label_idx % SEEKER_LABELS.len()];
                let label_x = cx + CROSSHAIR_WIDTH + 1;
                let label_y = cy + 1; // Same row as '·'
                if label_x + label.len() as u16 <= inner.x + inner.width
                    && label_y < inner.y + inner.height.saturating_sub(1)
                {
                    buf.set_string(
                        label_x,
                        label_y,
                        label,
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    );
                } else {
                    // Redraw as single-line brackets since label can't fit
                    for (row, line) in BRACKET_SEEK.iter().enumerate() {
                        let y = cy + row as u16;
                        if y >= inner.y + inner.height.saturating_sub(1) {
                            break;
                        }
                        for (col, ch) in line.chars().enumerate() {
                            if ch == ' ' {
                                continue;
                            }
                            let x = cx + col as u16;
                            if x >= inner.x + inner.width {
                                break;
                            }
                            buf.set_string(x, y, ch.to_string(), bracket_style);
                        }
                    }
                }
            }
        }
    }
}

/// Render the startup banner as a centered overlay with trippy rainbow logo,
/// or the T-800 boot sequence if the T-800 theme is active.
pub fn render(
    f: &mut Frame,
    theme: &Theme,
    tick: usize,
    elapsed: Duration,
    t800_splash: Option<&mut T800Splash>,
) {
    if theme.is_t800() {
        render_t800(f, theme, tick, elapsed, t800_splash);
        return;
    }

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
/// Left side: logo + icon (magnifying glass or targeting reticle).
/// Right side: bordered tip pane with rotating, word-wrapped tip text.
///
/// Returns the remaining `Rect` below the bar for content.
pub fn render_logo_bar(
    f: &mut Frame,
    area: Rect,
    theme: &Theme,
    tip_index: usize,
    tick: usize,
    tip_change_tick: usize,
    stats_line: Option<Line<'static>>,
) -> Rect {
    // For very small terminals, skip entirely
    if area.height < 8 {
        return area;
    }

    let tips = shuffled_tips();
    let tip_idx = tip_index % tips.len();
    let tip_text = tips[tip_idx];
    // Strip prefix for the pane (header already says the pane title)
    let tip_content = tip_text.strip_prefix("Pro-tip: ").unwrap_or(tip_text);

    let logo_glass_width = LOGO_WIDTH + GLASS_WIDTH;

    // Choose icon and pane title based on theme
    let icon = if theme.is_t800() { &RETICLE } else { &GLASS };
    let pane_title = if theme.is_t800() {
        " Intel "
    } else {
        " Pro-tips "
    };

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

    let rows = Layout::vertical([Constraint::Length(5), Constraint::Min(0)]).split(area);

    let cols = Layout::horizontal([Constraint::Length(logo_glass_width), Constraint::Min(15)])
        .split(rows[0]);

    // ── Left: logo + icon ──
    let mut logo_lines: Vec<Line> = Vec::new();
    for (i, art_line) in LOGO.iter().enumerate() {
        let icon_line = icon.get(i).copied().unwrap_or("");
        logo_lines.push(Line::from(vec![
            Span::styled(
                art_line.to_string(),
                Style::default()
                    .fg(theme.active)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(icon_line.to_string(), Style::default().fg(theme.text)),
        ]));
    }

    // Stats line below logo (fits within the 5-row column)
    if let Some(stats) = stats_line {
        logo_lines.push(stats);
    }

    f.render_widget(Paragraph::new(logo_lines), cols[0]);

    // ── Right: Tip pane ──
    let tip_block = Block::default()
        .title(Line::from(Span::styled(
            pane_title,
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        )))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));

    // T-800: typewriter effect — reveal characters over time (resets on tip change)
    let visible_tip = if theme.is_t800() {
        let chars_visible = tick.saturating_sub(tip_change_tick) / 2; // ~15 chars/sec at 30fps
        let s: String = tip_content.chars().take(chars_visible).collect();
        s
    } else {
        tip_content.to_string()
    };

    let tip_para = Paragraph::new(visible_tip)
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
