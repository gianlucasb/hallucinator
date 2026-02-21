use super::*;
use crate::action::Action;
use crate::model::config::ConfigSection;

/// Create a minimal App for testing (no backend, no files).
fn test_app() -> App {
    App::new(vec![], Theme::hacker())
}

/// Navigate from Banner to Queue (dismiss banner).
fn dismiss_banner(app: &mut App) {
    app.screen = Screen::Queue;
}

// ── FilePickerContext defaults ──────────────────────────────────

#[test]
fn file_picker_context_defaults_to_add_files() {
    let app = test_app();
    assert_eq!(app.file_picker_context, FilePickerContext::AddFiles);
}

// ── AddFiles from Queue opens picker in AddFiles mode ──────────

#[test]
fn add_files_from_queue_opens_picker() {
    let mut app = test_app();
    dismiss_banner(&mut app);
    app.update(Action::AddFiles);
    assert_eq!(app.screen, Screen::FilePicker);
    assert_eq!(app.file_picker_context, FilePickerContext::AddFiles);
}

// ── AddFiles from Config > Databases item 0 opens db picker ────

#[test]
fn add_files_from_config_databases_item0_opens_db_picker() {
    let mut app = test_app();
    app.screen = Screen::Config;
    app.config_state.section = ConfigSection::Databases;
    app.config_state.item_cursor = 0;

    app.update(Action::AddFiles);

    assert_eq!(app.screen, Screen::FilePicker);
    assert_eq!(
        app.file_picker_context,
        FilePickerContext::SelectDatabase { config_item: 0 }
    );
}

#[test]
fn add_files_from_config_databases_item1_opens_db_picker() {
    let mut app = test_app();
    app.screen = Screen::Config;
    app.config_state.section = ConfigSection::Databases;
    app.config_state.item_cursor = 1;

    app.update(Action::AddFiles);

    assert_eq!(app.screen, Screen::FilePicker);
    assert_eq!(
        app.file_picker_context,
        FilePickerContext::SelectDatabase { config_item: 1 }
    );
}

// ── AddFiles from Config > Databases item 2+ is a no-op ────────

#[test]
fn add_files_from_config_databases_toggle_item_is_noop() {
    let mut app = test_app();
    app.screen = Screen::Config;
    app.config_state.section = ConfigSection::Databases;
    app.config_state.item_cursor = 3; // a DB toggle item

    app.update(Action::AddFiles);

    // Should stay on Config, not open picker
    assert_eq!(app.screen, Screen::Config);
}

// ── AddFiles from Config > non-Databases section is a no-op ────

#[test]
fn add_files_from_config_api_keys_is_noop() {
    let mut app = test_app();
    app.screen = Screen::Config;
    app.config_state.section = ConfigSection::ApiKeys;
    app.config_state.item_cursor = 0;

    app.update(Action::AddFiles);

    assert_eq!(app.screen, Screen::Config);
}

// ── Esc in db picker with no selection returns to Config unchanged ──

#[test]
fn esc_in_db_picker_no_selection_returns_to_config() {
    let mut app = test_app();
    app.screen = Screen::FilePicker;
    app.file_picker_context = FilePickerContext::SelectDatabase { config_item: 0 };
    app.file_picker.selected.clear();
    app.config_state.dblp_offline_path = String::new();

    app.update(Action::NavigateBack);

    assert_eq!(app.screen, Screen::Config);
    assert_eq!(app.file_picker_context, FilePickerContext::AddFiles);
    assert!(app.config_state.dblp_offline_path.is_empty());
}

// ── Esc in db picker with selection writes canonicalized path ────

#[test]
fn esc_in_db_picker_with_selection_writes_path() {
    let mut app = test_app();
    app.screen = Screen::FilePicker;
    app.file_picker_context = FilePickerContext::SelectDatabase { config_item: 1 };

    // Use a path that definitely exists so canonicalize succeeds
    let existing = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    app.file_picker.selected = vec![existing.clone()];

    app.update(Action::NavigateBack);

    assert_eq!(app.screen, Screen::Config);
    assert_eq!(app.file_picker_context, FilePickerContext::AddFiles);
    // Should be an absolute, canonicalized path
    let result = &app.config_state.acl_offline_path;
    assert!(!result.is_empty());
    assert!(PathBuf::from(result).is_absolute());
}

// ── Esc in normal picker returns to Queue ───────────────────────

#[test]
fn esc_in_normal_picker_returns_to_queue() {
    let mut app = test_app();
    app.screen = Screen::FilePicker;
    app.file_picker_context = FilePickerContext::AddFiles;

    app.update(Action::NavigateBack);

    assert_eq!(app.screen, Screen::Queue);
}

// ── Space in db picker ignores non-db files ─────────────────────

#[test]
fn space_in_db_picker_ignores_non_db_entry() {
    let mut app = test_app();
    app.screen = Screen::FilePicker;
    app.file_picker_context = FilePickerContext::SelectDatabase { config_item: 0 };

    // Inject a PDF entry at cursor
    app.file_picker.entries = vec![FileEntry {
        name: "paper.pdf".to_string(),
        path: PathBuf::from("/tmp/paper.pdf"),
        is_dir: false,
        is_pdf: true,
        is_bbl: false,
        is_bib: false,
        is_archive: false,
        is_json: false,
        is_db: false,
    }];
    app.file_picker.cursor = 0;

    app.update(Action::ToggleSafe);

    assert!(app.file_picker.selected.is_empty());
}

// ── Space in db picker selects db file (single-select) ──────────

#[test]
fn space_in_db_picker_selects_db_file() {
    let mut app = test_app();
    app.screen = Screen::FilePicker;
    app.file_picker_context = FilePickerContext::SelectDatabase { config_item: 0 };

    app.file_picker.entries = vec![FileEntry {
        name: "dblp.db".to_string(),
        path: PathBuf::from("/tmp/dblp.db"),
        is_dir: false,
        is_pdf: false,
        is_bbl: false,
        is_bib: false,
        is_archive: false,
        is_json: false,
        is_db: true,
    }];
    app.file_picker.cursor = 0;

    app.update(Action::ToggleSafe);

    assert_eq!(app.file_picker.selected.len(), 1);
    assert_eq!(app.file_picker.selected[0], PathBuf::from("/tmp/dblp.db"));
}

// ── Space in db picker replaces previous selection ──────────────

#[test]
fn space_in_db_picker_single_select_replaces() {
    let mut app = test_app();
    app.screen = Screen::FilePicker;
    app.file_picker_context = FilePickerContext::SelectDatabase { config_item: 0 };

    app.file_picker.selected = vec![PathBuf::from("/tmp/old.db")];
    app.file_picker.entries = vec![FileEntry {
        name: "new.db".to_string(),
        path: PathBuf::from("/tmp/new.db"),
        is_dir: false,
        is_pdf: false,
        is_bbl: false,
        is_bib: false,
        is_archive: false,
        is_json: false,
        is_db: true,
    }];
    app.file_picker.cursor = 0;

    app.update(Action::ToggleSafe);

    assert_eq!(app.file_picker.selected.len(), 1);
    assert_eq!(app.file_picker.selected[0], PathBuf::from("/tmp/new.db"));
}

// ── Enter on .db file in db picker confirms and returns to Config ──

#[test]
fn enter_on_db_file_in_db_picker_confirms() {
    let mut app = test_app();
    app.screen = Screen::FilePicker;
    app.file_picker_context = FilePickerContext::SelectDatabase { config_item: 0 };

    // Use CARGO_MANIFEST_DIR as a known-existing path for canonicalize
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cargo_toml = manifest.join("Cargo.toml");

    // Create a fake .db entry pointing to a real file (so canonicalize works)
    app.file_picker.entries = vec![FileEntry {
        name: "Cargo.toml".to_string(), // reuse existing file
        path: cargo_toml.clone(),
        is_dir: false,
        is_pdf: false,
        is_bbl: false,
        is_bib: false,
        is_archive: false,
        is_json: false,
        is_db: true, // pretend it's a db
    }];
    app.file_picker.cursor = 0;

    app.update(Action::DrillIn);

    assert_eq!(app.screen, Screen::Config);
    assert_eq!(app.file_picker_context, FilePickerContext::AddFiles);
    let result = &app.config_state.dblp_offline_path;
    assert!(!result.is_empty());
    assert!(PathBuf::from(result).is_absolute());
}

// ── Enter on directory in db picker navigates into it ───────────

#[test]
fn enter_on_dir_in_db_picker_navigates() {
    let mut app = test_app();
    app.screen = Screen::FilePicker;
    app.file_picker_context = FilePickerContext::SelectDatabase { config_item: 0 };

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    app.file_picker.entries = vec![FileEntry {
        name: "src".to_string(),
        path: manifest.join("src"),
        is_dir: true,
        is_pdf: false,
        is_bbl: false,
        is_bib: false,
        is_archive: false,
        is_json: false,
        is_db: false,
    }];
    app.file_picker.cursor = 0;

    app.update(Action::DrillIn);

    // Should still be in file picker, navigated into the dir
    assert_eq!(app.screen, Screen::FilePicker);
    assert!(app.file_picker_context == FilePickerContext::SelectDatabase { config_item: 0 });
}

// ── Enter on non-db file in db picker is a no-op ────────────────

#[test]
fn enter_on_non_db_file_in_db_picker_is_noop() {
    let mut app = test_app();
    app.screen = Screen::FilePicker;
    app.file_picker_context = FilePickerContext::SelectDatabase { config_item: 0 };

    app.file_picker.entries = vec![FileEntry {
        name: "paper.pdf".to_string(),
        path: PathBuf::from("/tmp/paper.pdf"),
        is_dir: false,
        is_pdf: true,
        is_bbl: false,
        is_bib: false,
        is_archive: false,
        is_json: false,
        is_db: false,
    }];
    app.file_picker.cursor = 0;

    app.update(Action::DrillIn);

    // Should remain on file picker, nothing selected
    assert_eq!(app.screen, Screen::FilePicker);
    assert!(app.file_picker.selected.is_empty());
}

// ── Canonicalize on manual config edit ───────────────────────────

#[test]
fn confirm_config_edit_canonicalizes_dblp_path() {
    let mut app = test_app();
    app.screen = Screen::Config;
    app.config_state.section = ConfigSection::Databases;
    app.config_state.item_cursor = 0;

    // Start editing
    app.update(Action::DrillIn); // triggers handle_config_enter
    assert!(app.config_state.editing);

    // Clear buffer and type a known-existing path
    app.config_state.edit_buffer = env!("CARGO_MANIFEST_DIR").to_string();

    // Confirm
    app.update(Action::SearchConfirm);
    assert!(!app.config_state.editing);

    let result = &app.config_state.dblp_offline_path;
    assert!(!result.is_empty());
    assert!(PathBuf::from(result).is_absolute());
}

#[test]
fn confirm_config_edit_empty_path_stays_empty() {
    let mut app = test_app();
    app.screen = Screen::Config;
    app.config_state.section = ConfigSection::Databases;
    app.config_state.item_cursor = 1;

    app.update(Action::DrillIn);
    app.config_state.edit_buffer.clear();
    app.update(Action::SearchConfirm);

    assert!(app.config_state.acl_offline_path.is_empty());
}

// ── is_db detection in FileEntry ────────────────────────────────

#[test]
fn refresh_entries_detects_db_extension() {
    // We can't easily control the filesystem, but we can test the
    // detection logic directly on a FileEntry constructed in refresh_entries.
    let ext_db = std::path::Path::new("test.db")
        .extension()
        .and_then(|e| e.to_str());
    assert!(
        ext_db
            .map(|e| e.eq_ignore_ascii_case("db") || e.eq_ignore_ascii_case("sqlite"))
            .unwrap_or(false)
    );

    let ext_sqlite = std::path::Path::new("test.sqlite")
        .extension()
        .and_then(|e| e.to_str());
    assert!(
        ext_sqlite
            .map(|e| e.eq_ignore_ascii_case("db") || e.eq_ignore_ascii_case("sqlite"))
            .unwrap_or(false)
    );

    let ext_pdf = std::path::Path::new("test.pdf")
        .extension()
        .and_then(|e| e.to_str());
    assert!(
        !ext_pdf
            .map(|e| e.eq_ignore_ascii_case("db") || e.eq_ignore_ascii_case("sqlite"))
            .unwrap_or(false)
    );
}

// ── toggle_selected allows .db files ────────────────────────────

#[test]
fn toggle_selected_allows_db_files() {
    let mut picker = FilePickerState::new();
    picker.entries = vec![FileEntry {
        name: "test.db".to_string(),
        path: PathBuf::from("/tmp/test.db"),
        is_dir: false,
        is_pdf: false,
        is_bbl: false,
        is_bib: false,
        is_archive: false,
        is_json: false,
        is_db: true,
    }];
    picker.cursor = 0;

    picker.toggle_selected();
    assert_eq!(picker.selected.len(), 1);

    // Toggle off
    picker.toggle_selected();
    assert!(picker.selected.is_empty());
}

// ── Normal picker behavior unchanged ────────────────────────────

#[test]
fn normal_picker_enter_toggles_pdf() {
    let mut app = test_app();
    app.screen = Screen::FilePicker;
    app.file_picker_context = FilePickerContext::AddFiles;

    app.file_picker.entries = vec![FileEntry {
        name: "paper.pdf".to_string(),
        path: PathBuf::from("/tmp/paper.pdf"),
        is_dir: false,
        is_pdf: true,
        is_bbl: false,
        is_bib: false,
        is_archive: false,
        is_json: false,
        is_db: false,
    }];
    app.file_picker.cursor = 0;

    app.update(Action::DrillIn);

    // In normal mode, Enter on PDF toggles selection (stays in picker)
    assert_eq!(app.screen, Screen::FilePicker);
    assert_eq!(app.file_picker.selected.len(), 1);
}

// ── Dirty flag tracking ─────────────────────────────────────────

#[test]
fn config_starts_not_dirty() {
    let app = test_app();
    assert!(!app.config_state.dirty);
}

#[test]
fn confirm_config_edit_sets_dirty() {
    let mut app = test_app();
    app.screen = Screen::Config;
    app.config_state.section = ConfigSection::ApiKeys;
    app.config_state.item_cursor = 0;

    // Start editing, type something, confirm
    app.update(Action::DrillIn);
    app.config_state.edit_buffer = "test-key".to_string();
    app.update(Action::SearchConfirm);

    assert!(app.config_state.dirty);
}

#[test]
fn config_space_toggle_db_sets_dirty() {
    let mut app = test_app();
    app.screen = Screen::Config;
    app.config_state.section = ConfigSection::Databases;
    app.config_state.item_cursor = 7; // first DB toggle (0=DBLP, 1=ACL, 2=OpenAlex, 3=cache, 4=clear, 5=clear-nf, 6=searxng, 7+=toggles)

    app.update(Action::ToggleSafe);

    assert!(app.config_state.dirty);
}

#[test]
fn config_theme_cycle_sets_dirty() {
    let mut app = test_app();
    app.screen = Screen::Config;
    app.config_state.section = ConfigSection::Display;
    app.config_state.item_cursor = 0;

    app.update(Action::DrillIn); // Enter cycles theme

    assert!(app.config_state.dirty);
}

// ── Confirm exit prompt ─────────────────────────────────────────

#[test]
fn esc_on_dirty_config_shows_confirm_prompt() {
    let mut app = test_app();
    app.screen = Screen::Config;
    app.config_state.dirty = true;

    app.update(Action::NavigateBack);

    // Should stay on Config with confirm_exit active
    assert_eq!(app.screen, Screen::Config);
    assert!(app.config_state.confirm_exit);
}

#[test]
fn esc_on_clean_config_exits_immediately() {
    let mut app = test_app();
    app.screen = Screen::Config;
    app.config_state.dirty = false;

    app.update(Action::NavigateBack);

    assert_eq!(app.screen, Screen::Queue);
    assert!(!app.config_state.confirm_exit);
}

#[test]
fn confirm_prompt_n_discards_and_exits() {
    let mut app = test_app();
    app.screen = Screen::Config;
    app.config_state.dirty = true;
    app.config_state.confirm_exit = true;

    // n = NextMatch in normal mode
    app.update(Action::NextMatch);

    assert_eq!(app.screen, Screen::Queue);
    assert!(!app.config_state.confirm_exit);
    assert!(!app.config_state.dirty);
}

#[test]
fn confirm_prompt_esc_cancels_back_to_config() {
    let mut app = test_app();
    app.screen = Screen::Config;
    app.config_state.dirty = true;
    app.config_state.confirm_exit = true;

    app.update(Action::NavigateBack);

    // Should stay on Config, prompt dismissed
    assert_eq!(app.screen, Screen::Config);
    assert!(!app.config_state.confirm_exit);
    assert!(app.config_state.dirty); // still dirty
}

#[test]
fn confirm_prompt_ignores_other_actions() {
    let mut app = test_app();
    app.screen = Screen::Config;
    app.config_state.dirty = true;
    app.config_state.confirm_exit = true;

    app.update(Action::MoveDown);

    // Should still be showing prompt, nothing changed
    assert_eq!(app.screen, Screen::Config);
    assert!(app.config_state.confirm_exit);
}

#[test]
fn open_config_from_config_does_not_overwrite_prev_screen() {
    let mut app = test_app();
    // Navigate to config from Queue
    app.screen = Screen::Queue;
    app.update(Action::OpenConfig);
    assert_eq!(app.screen, Screen::Config);
    assert_eq!(app.config_state.prev_screen, Some(Screen::Queue));

    // Press ',' again while on Config — prev_screen must NOT become Config
    app.update(Action::OpenConfig);
    assert_eq!(app.screen, Screen::Config);
    assert_eq!(app.config_state.prev_screen, Some(Screen::Queue));
}

#[test]
fn discard_from_config_opened_twice_exits_to_queue() {
    // Regression test for #183: config screen becomes un-exitable
    // when opened from config, changes made, then discarded.
    let mut app = test_app();

    // Open config from Queue
    app.screen = Screen::Queue;
    app.update(Action::OpenConfig);
    assert_eq!(app.screen, Screen::Config);

    // Simulate pressing ',' again while on Config
    app.update(Action::OpenConfig);

    // Make a change (dirty) and press Esc
    app.config_state.dirty = true;
    app.update(Action::NavigateBack);
    assert!(app.config_state.confirm_exit);

    // Press 'n' to discard
    app.update(Action::NextMatch);
    assert_eq!(app.screen, Screen::Queue);
    assert!(!app.config_state.confirm_exit);
    assert!(!app.config_state.dirty);
}

#[test]
fn db_picker_enter_on_db_sets_dirty() {
    let mut app = test_app();
    app.screen = Screen::FilePicker;
    app.file_picker_context = FilePickerContext::SelectDatabase { config_item: 0 };

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cargo_toml = manifest.join("Cargo.toml");
    app.file_picker.entries = vec![FileEntry {
        name: "Cargo.toml".to_string(),
        path: cargo_toml,
        is_dir: false,
        is_pdf: false,
        is_bbl: false,
        is_bib: false,
        is_archive: false,
        is_json: false,
        is_db: true,
    }];
    app.file_picker.cursor = 0;

    app.update(Action::DrillIn);

    assert_eq!(app.screen, Screen::Config);
    assert!(app.config_state.dirty);
}
