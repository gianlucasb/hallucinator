use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::action::Action;
use crate::app::InputMode;

/// Map a crossterm terminal event to a TUI action, respecting input mode.
pub fn map_event(event: &Event, input_mode: &InputMode) -> Action {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            // Ctrl+C always quits regardless of mode
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                return Action::Quit;
            }

            match input_mode {
                InputMode::Normal => map_key_normal(key),
                InputMode::Search => map_key_search(key),
                InputMode::TextInput => map_key_text_input(key),
            }
        }
        Event::Mouse(mouse) => map_mouse(mouse),
        Event::Resize(w, h) => Action::Resize(*w, *h),
        _ => Action::None,
    }
}

fn map_mouse(mouse: &MouseEvent) -> Action {
    match mouse.kind {
        MouseEventKind::ScrollDown => Action::MoveDown,
        MouseEventKind::ScrollUp => Action::MoveUp,
        MouseEventKind::Down(MouseButton::Left) => Action::ClickAt(mouse.column, mouse.row),
        _ => Action::None,
    }
}

fn map_key_normal(key: &KeyEvent) -> Action {
    match key.code {
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Char('j') | KeyCode::Down => Action::MoveDown,
        KeyCode::Char('k') | KeyCode::Up => Action::MoveUp,
        KeyCode::Enter => Action::DrillIn,
        KeyCode::Esc => Action::NavigateBack,
        KeyCode::Char('g') => Action::GoTop,
        KeyCode::Char('G') => Action::GoBottom,
        KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::SaveConfig,
        KeyCode::Char('s') => Action::CycleSort,
        KeyCode::Char('f') => Action::CycleFilter,
        KeyCode::Char('/') => Action::StartSearch,
        KeyCode::Char('n') => Action::NextMatch,
        KeyCode::Char('N') => Action::PrevMatch,
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::Retry,
        KeyCode::Char('r') => Action::StartProcessing,
        KeyCode::Char('R') => Action::RetryAll,
        KeyCode::Char('e') => Action::Export,
        KeyCode::Char('o') | KeyCode::Char('a') => Action::AddFiles,
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::PageDown,
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::PageUp,
        KeyCode::Char('y') => Action::CopyToClipboard,
        KeyCode::Char(',') | KeyCode::Char('c') => Action::OpenConfig,
        KeyCode::Char(' ') => Action::ToggleSafe,
        KeyCode::Tab => Action::ToggleActivityPanel,
        KeyCode::Char('b') => Action::BuildDatabase,
        KeyCode::Char('?') => Action::ToggleHelp,
        KeyCode::PageDown => Action::PageDown,
        KeyCode::PageUp => Action::PageUp,
        KeyCode::Home => Action::GoTop,
        KeyCode::End => Action::GoBottom,
        _ => Action::None,
    }
}

fn map_key_search(key: &KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::SearchCancel,
        KeyCode::Enter => Action::SearchConfirm,
        KeyCode::Char(c) => Action::SearchInput(c),
        KeyCode::Backspace => Action::SearchInput('\x08'), // sentinel for backspace
        _ => Action::None,
    }
}

fn map_key_text_input(key: &KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::SearchCancel,
        KeyCode::Enter => Action::SearchConfirm,
        KeyCode::Char(c) => Action::SearchInput(c),
        KeyCode::Backspace => Action::SearchInput('\x08'),
        _ => Action::None,
    }
}
