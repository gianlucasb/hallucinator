use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::Parser;
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
mod export;
mod input;
mod model;
mod persistence;
mod theme;
mod tui_event;
mod view;

use app::{App, Screen};

/// Hallucinator TUI — batch academic reference validation with a terminal interface.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// PDF files to check
    pdf_paths: Vec<PathBuf>,

    /// OpenAlex API key
    #[arg(long)]
    openalex_key: Option<String>,

    /// Semantic Scholar API key
    #[arg(long)]
    s2_api_key: Option<String>,

    /// Path to offline DBLP database
    #[arg(long)]
    dblp_offline: Option<PathBuf>,

    /// Comma-separated list of databases to disable
    #[arg(long, value_delimiter = ',')]
    disable_dbs: Vec<String>,

    /// Flag author mismatches from OpenAlex (default: skipped)
    #[arg(long)]
    check_openalex_authors: bool,

    /// Color theme: hacker (default) or modern
    #[arg(long, default_value = "hacker")]
    theme: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let args = Args::parse();

    // Validate any PDF paths provided on the command line
    for path in &args.pdf_paths {
        if !path.exists() {
            anyhow::bail!("PDF file not found: {}", path.display());
        }
    }

    // Resolve config from CLI flags > env vars > defaults
    let openalex_key = args
        .openalex_key
        .or_else(|| std::env::var("OPENALEX_KEY").ok());
    let s2_api_key = args.s2_api_key.or_else(|| std::env::var("S2_API_KEY").ok());
    let dblp_offline_path = args
        .dblp_offline
        .or_else(|| std::env::var("DBLP_OFFLINE_PATH").ok().map(PathBuf::from));

    let db_timeout_secs: u64 = std::env::var("DB_TIMEOUT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let db_timeout_short_secs: u64 = std::env::var("DB_TIMEOUT_SHORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);

    // Open DBLP database if configured
    let dblp_offline_db: Option<Arc<Mutex<hallucinator_dblp::DblpDatabase>>> =
        if let Some(ref path) = dblp_offline_path {
            Some(backend::open_dblp_db(path)?)
        } else {
            None
        };

    // Select theme
    let theme = match args.theme.as_str() {
        "modern" => theme::Theme::modern(),
        _ => theme::Theme::hacker(),
    };

    // Build filenames for display
    let filenames: Vec<String> = args
        .pdf_paths
        .iter()
        .map(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| p.display().to_string())
        })
        .collect();

    // Initialize terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

    // Install panic hook that restores terminal before printing panic
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original_hook(panic_info);
    }));

    let backend_terminal = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend_terminal)?;

    // Drain any stray input events (e.g. Enter keypress from launching the command)
    while event::poll(Duration::from_millis(50)).unwrap_or(false) {
        let _ = event::read();
    }

    let mut app = App::new(filenames, theme);

    // Store PDF paths for deferred processing
    app.pdf_paths = args.pdf_paths.clone();

    // Populate config state from resolved CLI/env values
    app.config_state.openalex_key = openalex_key.clone().unwrap_or_default();
    app.config_state.s2_api_key = s2_api_key.clone().unwrap_or_default();
    app.config_state.max_concurrent_refs = 4;
    app.config_state.db_timeout_secs = db_timeout_secs;
    app.config_state.db_timeout_short_secs = db_timeout_short_secs;
    app.config_state.dblp_offline_path = dblp_offline_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    app.config_state.theme_name = args.theme.clone();

    // Mark disabled DBs from CLI args
    for (name, enabled) in &mut app.config_state.disabled_dbs {
        if args
            .disable_dbs
            .iter()
            .any(|d| d.eq_ignore_ascii_case(name))
        {
            *enabled = false;
        }
    }

    // Initialize results persistence directory
    let run_dir = persistence::run_dir();

    // Single-paper mode: if exactly one PDF, skip the queue and go directly to paper view
    if args.pdf_paths.len() == 1 {
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
    let check_openalex_authors = args.check_openalex_authors;
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

                    config.dblp_offline_path = cached_dblp_path.clone();
                    config.dblp_offline_db = cached_dblp_db.clone();
                    config.check_openalex_authors = check_openalex_authors;

                    let tx = event_tx_for_backend.clone();
                    let cancel = batch_cancel.clone();
                    // Spawn batch as a separate task so we can still receive commands
                    tokio::spawn(async move {
                        backend::run_batch_with_offset(
                            files,
                            config,
                            tx,
                            cancel,
                            starting_index,
                            max_concurrent_papers,
                        )
                        .await;
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
    let tick_rate = Duration::from_millis(100);

    loop {
        // Draw
        terminal.draw(|f| app.view(f))?;

        // Poll for events with timeout for tick
        let timeout = tick_rate;

        tokio::select! {
            // Backend events (non-blocking drain)
            maybe_event = event_rx.recv() => {
                match maybe_event {
                    Some(backend_event) => {
                        // Persist paper results on completion
                        if let tui_event::BackendEvent::PaperComplete { paper_index, .. } = &backend_event {
                            if let Some(ref dir) = run_dir {
                                let pi = *paper_index;
                                if let Some(paper) = app.papers.get(pi) {
                                    persistence::save_paper_results(
                                        dir,
                                        pi,
                                        &paper.filename,
                                        &paper.results,
                                    );
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
                        // Backend channel closed — processing done
                    }
                }
            }
            // Terminal input events
            _ = async {
                if event::poll(timeout).unwrap_or(false) {
                    if let Ok(evt) = event::read() {
                        // Map Tab differently on Config screen
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
                        if app.update(action) {
                            // Quit requested
                        }
                    }
                }
            } => {}
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
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;

    Ok(())
}
