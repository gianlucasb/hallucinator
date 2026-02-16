pub mod activity;
pub mod banner;
pub mod config;
pub mod detail;
pub mod export;
pub mod file_picker;
pub mod help;
pub mod paper;
pub mod queue;
pub mod quit_confirm;

/// Spinner frames for animated progress indication.
const SPINNER_FRAMES: &[char] = &[
    '\u{280B}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283C}', '\u{2834}', '\u{2826}', '\u{2827}',
    '\u{2807}', '\u{280F}',
];

/// Get the current spinner character based on a tick counter.
pub fn spinner_char(tick: usize) -> char {
    SPINNER_FRAMES[tick % SPINNER_FRAMES.len()]
}

/// Truncate a string to fit in `max_width` columns, appending "\u{2026}" if truncated.
pub fn truncate(s: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if s.len() <= max_width {
        return s.to_string();
    }
    let mut truncated: String = s.chars().take(max_width.saturating_sub(1)).collect();
    truncated.push('\u{2026}');
    truncated
}
