use ratatui::style::{Color, Modifier, Style};

use crate::model::paper::RefPhase;
use crate::model::queue::PaperPhase;
use hallucinator_core::Status;

/// Color theme for the TUI.
pub struct Theme {
    pub verified: Color,
    pub not_found: Color,
    pub author_mismatch: Color,
    pub retracted: Color,

    pub header_fg: Color,
    pub header_bg: Color,
    pub border: Color,
    pub text: Color,
    pub dim: Color,
    pub highlight_bg: Color,
    pub active: Color,
    pub queued: Color,
    pub spinner: Color,
    pub footer_fg: Color,
    pub footer_bg: Color,
}

impl Theme {
    /// Hacker-green terminal theme.
    pub fn hacker() -> Self {
        Self {
            verified: Color::Rgb(0, 210, 0),
            not_found: Color::Red,
            author_mismatch: Color::Yellow,
            retracted: Color::Magenta,

            header_fg: Color::Black,
            header_bg: Color::Rgb(0, 210, 0),
            border: Color::DarkGray,
            text: Color::White,
            dim: Color::DarkGray,
            highlight_bg: Color::Rgb(30, 50, 30),
            active: Color::Cyan,
            queued: Color::DarkGray,
            spinner: Color::Cyan,
            footer_fg: Color::DarkGray,
            footer_bg: Color::Reset,
        }
    }

    /// Modern theme: white text, electric blue accents, dark blue header.
    pub fn modern() -> Self {
        Self {
            verified: Color::Rgb(0, 200, 80),
            not_found: Color::Rgb(255, 80, 80),
            author_mismatch: Color::Rgb(255, 200, 0),
            retracted: Color::Rgb(200, 50, 200),

            header_fg: Color::White,
            header_bg: Color::Rgb(30, 60, 120),
            border: Color::Rgb(60, 60, 80),
            text: Color::White,
            dim: Color::Rgb(120, 120, 140),
            highlight_bg: Color::Rgb(30, 40, 80),
            active: Color::Rgb(60, 140, 255),
            queued: Color::Rgb(80, 80, 100),
            spinner: Color::Rgb(60, 140, 255),
            footer_fg: Color::Rgb(120, 120, 140),
            footer_bg: Color::Reset,
        }
    }

    pub fn status_color(&self, status: &Status) -> Color {
        match status {
            Status::Verified => self.verified,
            Status::NotFound => self.not_found,
            Status::AuthorMismatch => self.author_mismatch,
        }
    }

    pub fn paper_phase_color(&self, phase: &PaperPhase) -> Color {
        match phase {
            PaperPhase::Queued => self.queued,
            PaperPhase::Extracting => self.active,
            PaperPhase::ExtractionFailed => self.not_found,
            PaperPhase::Checking => self.active,
            PaperPhase::Retrying => self.author_mismatch,
            PaperPhase::Complete => self.verified,
        }
    }

    pub fn ref_phase_style(&self, phase: &RefPhase) -> Style {
        match phase {
            RefPhase::Pending => Style::default().fg(self.dim),
            RefPhase::Checking => Style::default()
                .fg(self.spinner)
                .add_modifier(Modifier::BOLD),
            RefPhase::Retrying => Style::default()
                .fg(self.author_mismatch)
                .add_modifier(Modifier::BOLD),
            RefPhase::Done => Style::default().fg(self.text),
            RefPhase::Skipped(_) => Style::default().fg(self.dim).add_modifier(Modifier::DIM),
        }
    }

    pub fn header_style(&self) -> Style {
        Style::default()
            .fg(self.header_fg)
            .bg(self.header_bg)
            .add_modifier(Modifier::BOLD)
    }

    pub fn highlight_style(&self) -> Style {
        Style::default()
            .bg(self.highlight_bg)
            .add_modifier(Modifier::BOLD)
    }

    pub fn border_style(&self) -> Style {
        Style::default().fg(self.border)
    }

    pub fn footer_style(&self) -> Style {
        Style::default().fg(self.footer_fg).bg(self.footer_bg)
    }
}
