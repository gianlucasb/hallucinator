use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::{Parser, Subcommand};
use ratatui::crossterm::event;
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::prelude::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

mod action;
mod app;
mod backend;
mod config_file;
mod export;
mod input;
mod load;
mod model;
mod persistence;
mod theme;
mod tui_event;
mod view;

use app::{App, Screen};

/// Hallucinator TUI — batch academic reference validation with a terminal interface.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// PDF or .bbl files to check
    file_paths: Vec<PathBuf>,

    /// OpenAlex API key
    #[arg(long)]
    openalex_key: Option<String>,

    /// Semantic Scholar API key
    #[arg(long)]
    s2_api_key: Option<String>,

    /// Path to offline DBLP database
    #[arg(long)]
    dblp_offline: Option<PathBuf>,

    /// Path to offline ACL Anthology database
    #[arg(long)]
    acl_offline: Option<PathBuf>,

    /// Comma-separated list of databases to disable
    #[arg(long, value_delimiter = ',')]
    disable_dbs: Vec<String>,

    /// Flag author mismatches from OpenAlex (default: skipped)
    #[arg(long)]
    check_openalex_authors: bool,

    /// Color theme: hacker (default) or modern
    #[arg(long, default_value = "hacker")]
    theme: String,

    /// Load previously saved results (.json) instead of processing PDFs
    #[arg(long)]
    load: Option<PathBuf>,

    /// Enable mouse support (click to select rows, scroll)
    #[arg(long)]
    mouse: bool,

    /// Target frames per second (default: 30)
    #[arg(long, default_value = "30")]
    fps: u32,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Download and build the offline DBLP database
    UpdateDblp {
        /// Path to store the DBLP SQLite database (default: ./dblp.db)
        path: Option<PathBuf>,
    },
    /// Download and build the offline ACL Anthology database
    UpdateAcl {
        /// Path to store the ACL SQLite database (default: ./acl.db)
        path: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();

    // Handle subcommands
    if let Some(command) = cli.command {
        return match command {
            Command::UpdateDblp { path } => {
                let db_path = path.unwrap_or_else(|| PathBuf::from("dblp.db"));
                update_dblp(&db_path).await
            }
            Command::UpdateAcl { path } => {
                let db_path = path.unwrap_or_else(|| PathBuf::from("acl.db"));
                update_acl(&db_path).await
            }
        };
    }

    // --- TUI mode (default, no subcommand) ---

    // Validate any file paths provided on the command line
    for path in &cli.file_paths {
        if !path.exists() {
            anyhow::bail!("File not found: {}", path.display());
        }
    }

    // Load config file (CWD .hallucinator.toml > platform config dir)
    let file_config = config_file::load_config();

    // Start with defaults, apply file config
    let mut config_state = model::config::ConfigState::default();
    config_file::apply_to_config_state(&file_config, &mut config_state);

    // Apply env vars (override file config)
    if let Ok(key) = std::env::var("OPENALEX_KEY") {
        if !key.is_empty() {
            config_state.openalex_key = key;
        }
    }
    if let Ok(key) = std::env::var("S2_API_KEY") {
        if !key.is_empty() {
            config_state.s2_api_key = key;
        }
    }
    if let Ok(path) = std::env::var("DBLP_OFFLINE_PATH") {
        if !path.is_empty() {
            config_state.dblp_offline_path = path;
        }
    }
    if let Ok(path) = std::env::var("ACL_OFFLINE_PATH") {
        if !path.is_empty() {
            config_state.acl_offline_path = path;
        }
    }
    if let Ok(v) = std::env::var("DB_TIMEOUT") {
        if let Ok(secs) = v.parse::<u64>() {
            config_state.db_timeout_secs = secs;
        }
    }
    if let Ok(v) = std::env::var("DB_TIMEOUT_SHORT") {
        if let Ok(secs) = v.parse::<u64>() {
            config_state.db_timeout_short_secs = secs;
        }
    }

    // Apply CLI args (highest priority)
    if let Some(key) = cli.openalex_key {
        config_state.openalex_key = key;
    }
    if let Some(key) = cli.s2_api_key {
        config_state.s2_api_key = key;
    }
    if let Some(ref path) = cli.dblp_offline {
        config_state.dblp_offline_path = path.display().to_string();
    }
    if let Some(ref path) = cli.acl_offline {
        config_state.acl_offline_path = path.display().to_string();
    }
    config_state.theme_name = cli.theme.clone();
    config_state.fps = cli.fps.clamp(1, 120);

    // Mark disabled DBs from CLI args
    for (name, enabled) in &mut config_state.disabled_dbs {
        if cli.disable_dbs.iter().any(|d| d.eq_ignore_ascii_case(name)) {
            *enabled = false;
        }
    }

    // Auto-detect default DBLP DB if no explicit path configured
    // Check CWD first (default update-dblp location), then platform data dir
    if config_state.dblp_offline_path.is_empty() {
        let candidates = [
            PathBuf::from("dblp.db"),
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("hallucinator")
                .join("dblp.db"),
        ];
        for candidate in &candidates {
            if candidate.exists() {
                config_state.dblp_offline_path = candidate.display().to_string();
                break;
            }
        }
    }
    // Auto-detect default ACL DB if no explicit path configured
    if config_state.acl_offline_path.is_empty() {
        let candidates = [
            PathBuf::from("acl.db"),
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("hallucinator")
                .join("acl.db"),
        ];
        for candidate in &candidates {
            if candidate.exists() {
                config_state.acl_offline_path = candidate.display().to_string();
                break;
            }
        }
    }

    // Resolve DBLP offline path from config state
    let dblp_offline_path: Option<PathBuf> = if config_state.dblp_offline_path.is_empty() {
        None
    } else {
        Some(PathBuf::from(&config_state.dblp_offline_path))
    };

    // Open DBLP database if configured
    let dblp_offline_db: Option<Arc<Mutex<hallucinator_dblp::DblpDatabase>>> =
        if let Some(ref path) = dblp_offline_path {
            Some(backend::open_dblp_db(path)?)
        } else {
            None
        };

    // Resolve ACL offline path from config state
    let acl_offline_path: Option<PathBuf> = if config_state.acl_offline_path.is_empty() {
        None
    } else {
        Some(PathBuf::from(&config_state.acl_offline_path))
    };

    // Open ACL database if configured
    let acl_offline_db: Option<Arc<Mutex<hallucinator_acl::AclDatabase>>> =
        if let Some(ref path) = acl_offline_path {
            Some(backend::open_acl_db(path)?)
        } else {
            None
        };

    // Select theme
    let theme = match config_state.theme_name.as_str() {
        "modern" => theme::Theme::modern(),
        _ => theme::Theme::hacker(),
    };

    // Build filenames for display
    let filenames: Vec<String> = cli
        .file_paths
        .iter()
        .map(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| p.display().to_string())
        })
        .collect();

    // Initialize terminal
    let mouse_enabled = cli.mouse;
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    if mouse_enabled {
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    } else {
        execute!(stdout, EnterAlternateScreen)?;
    }

    // Install panic hook that restores terminal before printing panic
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        if mouse_enabled {
            let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        } else {
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
        }
        original_hook(panic_info);
    }));

    let backend_terminal = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend_terminal)?;

    // Drain any stray input events (e.g. Enter keypress from launching the command)
    while event::poll(Duration::from_millis(50)).unwrap_or(false) {
        let _ = event::read();
    }

    let mut app = App::new(filenames, theme);

    // Store file paths for deferred processing
    app.file_paths = cli.file_paths.clone();

    // Apply the fully-resolved config state
    app.config_state = config_state;

    // Auto-dismiss the splash banner after 2 seconds (fps * 2 ticks)
    app.banner_dismiss_tick = Some(app.config_state.fps.max(1) as usize * 2);

    // Show config file path if one was loaded
    if let Some(path) = config_file::config_path() {
        if path.exists() {
            app.activity
                .log(format!("Config loaded from {}", path.display()));
        }
    }

    // Startup hints if no offline DBs configured (logged last so they show first)
    if app.config_state.acl_offline_path.is_empty() {
        app.activity.log_warn(
            "No offline ACL DB. Run 'hallucinator-tui update-acl' for faster lookups.".to_string(),
        );
    }
    if app.config_state.dblp_offline_path.is_empty() {
        app.activity.log_warn(
            "No offline DBLP DB. Run 'hallucinator-tui update-dblp' for faster lookups."
                .to_string(),
        );
    }
    if app.config_state.crossref_mailto.is_empty() {
        app.activity.log_warn(
            "No CrossRef mailto set. Set your email in Config > API Keys for better rate limits."
                .to_string(),
        );
    }

    // Initialize results persistence directory
    let run_dir = persistence::run_dir();

    // Load previously saved results if --load is provided
    if let Some(ref load_path) = cli.load {
        match load::load_results_file(load_path) {
            Ok(loaded) => {
                let count = loaded.len();
                for (paper, refs, paper_refs) in loaded {
                    app.papers.push(paper);
                    app.ref_states.push(refs);
                    app.paper_refs.push(paper_refs);
                    app.file_paths.push(PathBuf::new()); // placeholder
                }
                app.batch_complete = true;
                app.processing_started = true;
                app.recompute_sorted_indices();
                app.activity.log(format!(
                    "Loaded {} paper{} from {}",
                    count,
                    if count == 1 { "" } else { "s" },
                    load_path.display()
                ));
                if count == 1 {
                    app.single_paper_mode = true;
                    app.screen = Screen::Paper(0);
                }
            }
            Err(e) => {
                app.activity
                    .log_warn(format!("Failed to load results: {}", e));
            }
        }
    }

    // Single-paper mode: if exactly one PDF, skip the queue and go directly to paper view
    if cli.file_paths.len() == 1 && cli.load.is_none() {
        app.screen = Screen::Paper(0);
        app.single_paper_mode = true;
    }

    // Set up backend command channel for deferred processing
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<tui_event::BackendCommand>();
    let cancel = CancellationToken::new();

    app.backend_cmd_tx = Some(cmd_tx);

    // Spawn backend command listener
    let event_tx_for_backend = event_tx.clone();
    let mut cached_dblp_path = dblp_offline_path.clone();
    let mut cached_dblp_db = dblp_offline_db.clone();
    let mut cached_acl_path = acl_offline_path.clone();
    let mut cached_acl_db = acl_offline_db.clone();
    let check_openalex_authors = cli.check_openalex_authors;
    tokio::spawn(async move {
        // Per-batch cancel token — cancelled when user requests stop
        let mut batch_cancel = CancellationToken::new();

        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                tui_event::BackendCommand::ProcessFiles {
                    files,
                    starting_index,
                    max_concurrent_papers,
                    mut config,
                } => {
                    // Fresh token for this batch
                    batch_cancel = CancellationToken::new();

                    // If user changed the DBLP path in config, try to open the new DB
                    if config.dblp_offline_path != cached_dblp_path {
                        cached_dblp_path = config.dblp_offline_path.clone();
                        cached_dblp_db = if let Some(ref path) = cached_dblp_path {
                            backend::open_dblp_db(path).ok()
                        } else {
                            None
                        };
                    }

                    // If user changed the ACL path in config, try to open the new DB
                    if config.acl_offline_path != cached_acl_path {
                        cached_acl_path = config.acl_offline_path.clone();
                        cached_acl_db = if let Some(ref path) = cached_acl_path {
                            backend::open_acl_db(path).ok()
                        } else {
                            None
                        };
                    }

                    config.dblp_offline_path = cached_dblp_path.clone();
                    config.dblp_offline_db = cached_dblp_db.clone();
                    config.acl_offline_path = cached_acl_path.clone();
                    config.acl_offline_db = cached_acl_db.clone();
                    config.check_openalex_authors = check_openalex_authors;

                    let tx = event_tx_for_backend.clone();
                    let cancel = batch_cancel.clone();
                    // Spawn batch as a separate task so we can still receive commands
                    tokio::spawn(async move {
                        backend::run_batch_with_offset(
                            files,
                            *config,
                            tx,
                            cancel,
                            starting_index,
                            max_concurrent_papers,
                        )
                        .await;
                    });
                }
                tui_event::BackendCommand::RetryReferences {
                    paper_index,
                    refs_to_retry,
                    mut config,
                } => {
                    // Inject cached DB handles
                    config.dblp_offline_path = cached_dblp_path.clone();
                    config.dblp_offline_db = cached_dblp_db.clone();
                    config.acl_offline_path = cached_acl_path.clone();
                    config.acl_offline_db = cached_acl_db.clone();
                    config.check_openalex_authors = check_openalex_authors;

                    let tx = event_tx_for_backend.clone();
                    tokio::spawn(async move {
                        backend::retry_references(paper_index, refs_to_retry, *config, tx).await;
                    });
                }
                tui_event::BackendCommand::CancelProcessing => {
                    batch_cancel.cancel();
                }
            }
        }
    });

    // Also handle Ctrl+C at the OS level for clean shutdown
    let cancel_for_signal = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            cancel_for_signal.cancel();
        }
    });

    // Main event loop
    let tick_rate = Duration::from_millis(1000 / app.config_state.fps.max(1) as u64);

    loop {
        // Draw
        terminal.draw(|f| app.view(f))?;

        // Always process terminal input first (non-blocking) so user actions
        // like cancel are never starved by backend event floods.
        while event::poll(Duration::ZERO).unwrap_or(false) {
            if let Ok(evt) = event::read() {
                let action = if app.screen == Screen::Config {
                    match &evt {
                        ratatui::crossterm::event::Event::Key(key)
                            if key.kind == ratatui::crossterm::event::KeyEventKind::Press
                                && key.code == ratatui::crossterm::event::KeyCode::Tab
                                && !app.config_state.editing =>
                        {
                            action::Action::CycleConfigSection
                        }
                        _ => input::map_event(&evt, &app.input_mode),
                    }
                } else {
                    input::map_event(&evt, &app.input_mode)
                };
                app.update(action);
            }
        }

        // Then wait for backend events or tick timeout
        tokio::select! {
            maybe_event = event_rx.recv() => {
                match maybe_event {
                    Some(backend_event) => {
                        // Persist paper results on completion
                        if let tui_event::BackendEvent::PaperComplete { paper_index, .. } = &backend_event {
                            if let Some(ref dir) = run_dir {
                                let pi = *paper_index;
                                if let Some(paper) = app.papers.get(pi) {
                                    persistence::save_paper_results(dir, pi, paper);
                                }
                            }
                        }
                        app.handle_backend_event(backend_event);
                        // Drain any additional queued backend events
                        while let Ok(evt) = event_rx.try_recv() {
                            app.handle_backend_event(evt);
                        }
                    }
                    None => {
                        // Backend channel closed
                    }
                }
            }
            _ = tokio::time::sleep(tick_rate) => {}
        }

        // Process tick
        app.update(action::Action::Tick);

        if app.should_quit {
            cancel.cancel();
            break;
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    if mouse_enabled {
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
    } else {
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    }

    Ok(())
}

async fn update_dblp(db_path: &PathBuf) -> anyhow::Result<()> {
    use indicatif::{HumanBytes, HumanCount, MultiProgress, ProgressBar, ProgressStyle};
    use std::time::Instant;

    // Ensure parent directory exists
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    println!("Building offline DBLP database at: {}", db_path.display());

    let multi = MultiProgress::new();

    let dl_bar_style = ProgressStyle::with_template(
        "{spinner:.cyan} {msg} [{bar:40.cyan/dim}] {bytes}/{total_bytes} ({bytes_per_sec}, eta {eta})",
    )
    .unwrap()
    .progress_chars("=> ");

    let dl_unknown_style =
        ProgressStyle::with_template("{spinner:.cyan} {msg} {bytes} ({bytes_per_sec})").unwrap();

    let parse_bar_style = ProgressStyle::with_template(
        "{spinner:.green} {msg} [{bar:40.green/dim}] {percent}% (eta {eta})",
    )
    .unwrap()
    .progress_chars("=> ");

    let parse_spinner_style = ProgressStyle::with_template("{spinner:.green} {msg}").unwrap();

    let dl_bar = multi.add(ProgressBar::new(0));
    dl_bar.set_style(dl_unknown_style.clone());
    dl_bar.set_message("Connecting to dblp.org...");
    dl_bar.enable_steady_tick(Duration::from_millis(120));

    let parse_bar = multi.add(ProgressBar::new(0));
    parse_bar.set_style(parse_spinner_style.clone());
    parse_bar.enable_steady_tick(Duration::from_millis(120));

    let parse_start = std::cell::Cell::new(None::<Instant>);

    let updated = hallucinator_dblp::build_database(db_path, |event| match event {
        hallucinator_dblp::BuildProgress::Downloading {
            bytes_downloaded,
            total_bytes,
            ..
        } => {
            if let Some(total) = total_bytes {
                if dl_bar.length() == Some(0) {
                    dl_bar.set_length(total);
                    dl_bar.set_style(dl_bar_style.clone());
                }
                dl_bar.set_position(bytes_downloaded);
                dl_bar.set_message("dblp.xml.gz");
                if bytes_downloaded >= total && !dl_bar.is_finished() {
                    dl_bar.finish_with_message(format!(
                        "Downloaded {} in {:.0?}",
                        HumanBytes(total),
                        dl_bar.elapsed()
                    ));
                }
            } else {
                dl_bar.set_position(bytes_downloaded);
                dl_bar.set_message("dblp.xml.gz");
            }
        }
        hallucinator_dblp::BuildProgress::Parsing {
            records_parsed,
            records_inserted,
            bytes_read,
            bytes_total,
        } => {
            if !dl_bar.is_finished() {
                dl_bar.finish_with_message(format!(
                    "Downloaded {} in {:.0?}",
                    HumanBytes(dl_bar.position()),
                    dl_bar.elapsed()
                ));
            }
            if parse_start.get().is_none() {
                parse_start.set(Some(Instant::now()));
            }
            if bytes_total > 0 && parse_bar.length() == Some(0) {
                parse_bar.set_length(bytes_total);
                parse_bar.set_style(parse_bar_style.clone());
            }
            parse_bar.set_position(bytes_read);
            let elapsed = parse_start.get().unwrap().elapsed().as_secs_f64();
            let inserted_per_sec = if elapsed > 0.0 {
                records_inserted as f64 / elapsed
            } else {
                0.0
            };
            parse_bar.set_message(format!(
                "{} parsed, {} inserted ({}/s)",
                HumanCount(records_parsed),
                HumanCount(records_inserted),
                HumanCount(inserted_per_sec as u64),
            ));
        }
        hallucinator_dblp::BuildProgress::RebuildingIndex => {
            if !dl_bar.is_finished() {
                dl_bar.finish_with_message(format!(
                    "Downloaded {} in {:.0?}",
                    HumanBytes(dl_bar.position()),
                    dl_bar.elapsed()
                ));
            }
            parse_bar.set_style(parse_spinner_style.clone());
            parse_bar.set_message("Rebuilding FTS search index...");
        }
        hallucinator_dblp::BuildProgress::Complete {
            publications,
            authors,
            skipped,
        } => {
            let total_elapsed = parse_start
                .get()
                .map(|s| format!(" in {:.0?}", s.elapsed()))
                .unwrap_or_default();
            if skipped {
                parse_bar.finish_with_message("Database is already up to date (304 Not Modified)");
            } else {
                parse_bar.finish_with_message(format!(
                    "Indexed {} publications, {} authors{}",
                    HumanCount(publications),
                    HumanCount(authors),
                    total_elapsed
                ));
            }
        }
    })
    .await?;

    let canonical = std::fs::canonicalize(db_path).unwrap_or_else(|_| db_path.clone());
    if !updated {
        println!("Database is already up to date: {}", canonical.display());
    } else {
        println!("DBLP database saved to: {}", canonical.display());
    }

    Ok(())
}

async fn update_acl(db_path: &PathBuf) -> anyhow::Result<()> {
    use indicatif::{HumanCount, MultiProgress, ProgressBar, ProgressStyle};
    use std::time::Instant;

    // Ensure parent directory exists
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    println!(
        "Building offline ACL Anthology database at: {}",
        db_path.display()
    );

    let multi = MultiProgress::new();

    let dl_bar_style = ProgressStyle::with_template(
        "{spinner:.cyan} {msg} [{bar:40.cyan/dim}] {bytes}/{total_bytes} ({bytes_per_sec}, eta {eta})",
    )
    .unwrap()
    .progress_chars("=> ");

    let dl_unknown_style =
        ProgressStyle::with_template("{spinner:.cyan} {msg} {bytes} ({bytes_per_sec})").unwrap();

    let parse_bar_style = ProgressStyle::with_template(
        "{spinner:.green} {msg} [{bar:40.green/dim}] {percent}% (eta {eta})",
    )
    .unwrap()
    .progress_chars("=> ");

    let parse_spinner_style = ProgressStyle::with_template("{spinner:.green} {msg}").unwrap();

    let dl_bar = multi.add(ProgressBar::new(0));
    dl_bar.set_style(dl_unknown_style.clone());
    dl_bar.set_message("Connecting to GitHub...");
    dl_bar.enable_steady_tick(Duration::from_millis(120));

    let parse_bar = multi.add(ProgressBar::new(0));
    parse_bar.set_style(parse_spinner_style.clone());
    parse_bar.enable_steady_tick(Duration::from_millis(120));

    let parse_start = std::cell::Cell::new(None::<Instant>);

    let updated = hallucinator_acl::build_database(db_path, |event| match event {
        hallucinator_acl::BuildProgress::Downloading {
            bytes_downloaded,
            total_bytes,
        } => {
            if let Some(total) = total_bytes {
                if dl_bar.length() == Some(0) {
                    dl_bar.set_length(total);
                    dl_bar.set_style(dl_bar_style.clone());
                }
                dl_bar.set_position(bytes_downloaded);
                dl_bar.set_message("acl-anthology.tar.gz");
            } else {
                dl_bar.set_position(bytes_downloaded);
                dl_bar.set_message("acl-anthology.tar.gz");
            }
        }
        hallucinator_acl::BuildProgress::Extracting { files_extracted } => {
            if !dl_bar.is_finished() {
                dl_bar.finish_with_message(format!("Downloaded in {:.0?}", dl_bar.elapsed()));
            }
            parse_bar.set_message(format!("Extracting XML files... ({})", files_extracted));
        }
        hallucinator_acl::BuildProgress::Parsing {
            records_parsed,
            records_inserted,
            files_processed,
            files_total,
        } => {
            if !dl_bar.is_finished() {
                dl_bar.finish_with_message(format!("Downloaded in {:.0?}", dl_bar.elapsed()));
            }
            if parse_start.get().is_none() {
                parse_start.set(Some(Instant::now()));
            }
            if files_total > 0 && parse_bar.length() == Some(0) {
                parse_bar.set_length(files_total);
                parse_bar.set_style(parse_bar_style.clone());
            }
            parse_bar.set_position(files_processed);
            let elapsed = parse_start.get().unwrap().elapsed().as_secs_f64();
            let inserted_per_sec = if elapsed > 0.0 {
                records_inserted as f64 / elapsed
            } else {
                0.0
            };
            parse_bar.set_message(format!(
                "{} parsed, {} inserted ({}/s)",
                HumanCount(records_parsed),
                HumanCount(records_inserted),
                HumanCount(inserted_per_sec as u64),
            ));
        }
        hallucinator_acl::BuildProgress::RebuildingIndex => {
            if !dl_bar.is_finished() {
                dl_bar.finish_with_message(format!("Downloaded in {:.0?}", dl_bar.elapsed()));
            }
            parse_bar.set_style(parse_spinner_style.clone());
            parse_bar.set_message("Rebuilding FTS search index...");
        }
        hallucinator_acl::BuildProgress::Complete {
            publications,
            authors,
            skipped,
        } => {
            let total_elapsed = parse_start
                .get()
                .map(|s| format!(" in {:.0?}", s.elapsed()))
                .unwrap_or_default();
            if skipped {
                parse_bar.finish_with_message("Database is already up to date (same commit SHA)");
            } else {
                parse_bar.finish_with_message(format!(
                    "Indexed {} publications, {} authors{}",
                    HumanCount(publications),
                    HumanCount(authors),
                    total_elapsed
                ));
            }
        }
    })
    .await?;

    let canonical = std::fs::canonicalize(db_path).unwrap_or_else(|_| db_path.clone());
    if !updated {
        println!("Database is already up to date: {}", canonical.display());
    } else {
        println!("ACL database saved to: {}", canonical.display());
    }

    Ok(())
}
