use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use clap::{Parser, Subcommand};
use tokio_util::sync::CancellationToken;

mod output;

use output::ColorMode;

/// Hallucinated Reference Detector - Detect fabricated references in academic PDFs
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Path to config file (default: auto-detect platform config dir)
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Write tracing/debug logs to this file (default: stderr)
    #[arg(long, global = true)]
    log: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)]
enum Command {
    /// Check a PDF, .bbl, or .bib file for hallucinated references
    Check {
        /// Path to the PDF, .bbl, or .bib file to check
        file_path: PathBuf,

        /// Disable colored output
        #[arg(long)]
        no_color: bool,

        /// OpenAlex API key
        #[arg(long)]
        openalex_key: Option<String>,

        /// Semantic Scholar API key
        #[arg(long)]
        s2_api_key: Option<String>,

        /// Path to output log file
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Path to offline DBLP database
        #[arg(long)]
        dblp_offline: Option<PathBuf>,

        /// Path to offline ACL Anthology database
        #[arg(long)]
        acl_offline: Option<PathBuf>,

        /// Path to offline OpenAlex Tantivy index
        #[arg(long)]
        openalex_offline: Option<PathBuf>,

        /// Comma-separated list of databases to disable
        #[arg(long, value_delimiter = ',')]
        disable_dbs: Vec<String>,

        /// Flag author mismatches from OpenAlex (default: skipped)
        #[arg(long)]
        check_openalex_authors: bool,

        /// Number of concurrent reference checks (default: 4)
        #[arg(long)]
        num_workers: Option<usize>,

        /// Max 429 retries per database query (default: 3)
        #[arg(long)]
        max_rate_limit_retries: Option<u32>,

        /// Dry run: extract and print references without querying databases
        #[arg(long)]
        dry_run: bool,

        /// Enable SearxNG web search fallback for unverified citations.
        /// Uses SEARXNG_URL env var or defaults to http://localhost:8080
        #[arg(long)]
        searxng: bool,

        /// Path to persistent query cache database (SQLite)
        #[arg(long)]
        cache_path: Option<PathBuf>,

        /// Clear the query cache and exit
        #[arg(long)]
        clear_cache: bool,

        /// Clear only not-found entries from the cache and exit
        #[arg(long)]
        clear_not_found: bool,
    },

    /// Download and build the offline DBLP database
    UpdateDblp {
        /// Path to store the DBLP SQLite database
        path: PathBuf,
    },

    /// Download and build the offline ACL Anthology database
    UpdateAcl {
        /// Path to store the ACL SQLite database
        path: PathBuf,
    },

    /// Download and build the offline OpenAlex Tantivy index
    UpdateOpenalex {
        /// Path to store the OpenAlex index directory
        path: PathBuf,

        /// Only download S3 partitions newer than this date (YYYY-MM-DD)
        #[arg(long)]
        since: Option<String>,

        /// Only index works published in this year or later (e.g. 2020)
        #[arg(long)]
        min_year: Option<u32>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();

    // Initialize tracing: file (no ANSI) if --log given, otherwise stderr.
    if let Some(ref log_path) = cli.log {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
            .unwrap_or_else(|e| panic!("Cannot open log file {}: {}", log_path.display(), e));
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_writer(std::sync::Mutex::new(file))
            .with_ansi(false)
            .init();
    } else {
        tracing_subscriber::fmt::init();
    }

    // Load config file (explicit --config path, or auto-detect)
    let (file_config, config_source) = match &cli.config {
        Some(path) => match hallucinator_core::config_file::load_from_path(path) {
            Some(cfg) => (cfg, Some(path.clone())),
            None => {
                eprintln!("Warning: config file not found: {}", path.display());
                (hallucinator_core::config_file::ConfigFile::default(), None)
            }
        },
        None => {
            // Auto-detect: try platform config dir, then CWD overlay
            let cwd_path = PathBuf::from(".hallucinator.toml");
            let platform_path = hallucinator_core::config_file::config_path();

            let has_cwd = cwd_path.exists();
            let has_platform = platform_path.as_ref().is_some_and(|p| p.exists());

            let cfg = hallucinator_core::config_file::load_config();
            let source = if has_cwd {
                Some(cwd_path)
            } else if has_platform {
                platform_path
            } else {
                None
            };
            (cfg, source)
        }
    };

    match cli.command {
        Command::UpdateDblp { path } => update_dblp(&path).await,
        Command::UpdateAcl { path } => update_acl(&path).await,
        Command::UpdateOpenalex {
            path,
            since,
            min_year,
        } => update_openalex(&path, since.as_deref(), min_year).await,
        Command::Check {
            file_path,
            no_color,
            openalex_key,
            s2_api_key,
            output,
            dblp_offline,
            acl_offline,
            openalex_offline,
            disable_dbs,
            check_openalex_authors,
            num_workers,
            max_rate_limit_retries,
            dry_run,
            searxng,
            cache_path,
            clear_cache,
            clear_not_found,
        } => {
            if clear_cache || clear_not_found {
                let path = cache_path
                    .or_else(|| {
                        std::env::var("HALLUCINATOR_CACHE_PATH")
                            .ok()
                            .map(PathBuf::from)
                    })
                    .or_else(|| {
                        file_config
                            .databases
                            .as_ref()
                            .and_then(|d| d.cache_path.as_ref())
                            .map(PathBuf::from)
                    });
                return match path {
                    Some(p) if p.exists() => {
                        let cache = hallucinator_core::QueryCache::open(
                            &p,
                            std::time::Duration::from_secs(1),
                            std::time::Duration::from_secs(1),
                        )
                        .map_err(|e| anyhow::anyhow!("{}", e))?;
                        if clear_not_found {
                            let removed = cache.clear_not_found();
                            println!(
                                "Cleared {} not-found entries from cache: {}",
                                removed,
                                p.display()
                            );
                        } else {
                            cache.clear();
                            println!("Cache cleared: {}", p.display());
                        }
                        Ok(())
                    }
                    Some(p) => {
                        println!("No cache file at {}", p.display());
                        Ok(())
                    }
                    None => {
                        anyhow::bail!(
                            "No cache path specified. Use --cache-path or set HALLUCINATOR_CACHE_PATH"
                        );
                    }
                };
            }
            if dry_run {
                dry_run_check(file_path, no_color, output).await
            } else {
                check(
                    file_path,
                    no_color,
                    openalex_key,
                    s2_api_key,
                    output,
                    dblp_offline,
                    acl_offline,
                    openalex_offline,
                    disable_dbs,
                    check_openalex_authors,
                    num_workers,
                    max_rate_limit_retries,
                    searxng,
                    cache_path,
                    file_config,
                    config_source,
                )
                .await
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn check(
    file_path: PathBuf,
    no_color: bool,
    openalex_key: Option<String>,
    s2_api_key: Option<String>,
    output: Option<PathBuf>,
    dblp_offline: Option<PathBuf>,
    acl_offline: Option<PathBuf>,
    openalex_offline: Option<PathBuf>,
    disable_dbs: Vec<String>,
    check_openalex_authors: bool,
    num_workers: Option<usize>,
    max_rate_limit_retries: Option<u32>,
    searxng: bool,
    cache_path: Option<PathBuf>,
    file_config: hallucinator_core::config_file::ConfigFile,
    config_source: Option<PathBuf>,
) -> anyhow::Result<()> {
    // Print config file source
    match &config_source {
        Some(path) => eprintln!("Config file: {}", path.display()),
        None => eprintln!("Config file: none (use --config <path> or create .hallucinator.toml)"),
    }

    // Resolve configuration: CLI flags > env vars > config file > defaults
    let openalex_key = openalex_key
        .or_else(|| std::env::var("OPENALEX_KEY").ok())
        .or_else(|| {
            file_config
                .api_keys
                .as_ref()
                .and_then(|a| a.openalex_key.clone())
        });
    let s2_api_key = s2_api_key
        .or_else(|| std::env::var("S2_API_KEY").ok())
        .or_else(|| {
            file_config
                .api_keys
                .as_ref()
                .and_then(|a| a.s2_api_key.clone())
        });
    let dblp_offline_path = dblp_offline
        .or_else(|| std::env::var("DBLP_OFFLINE_PATH").ok().map(PathBuf::from))
        .or_else(|| {
            file_config
                .databases
                .as_ref()
                .and_then(|d| d.dblp_offline_path.as_ref())
                .map(PathBuf::from)
        });
    let acl_offline_path = acl_offline
        .or_else(|| std::env::var("ACL_OFFLINE_PATH").ok().map(PathBuf::from))
        .or_else(|| {
            file_config
                .databases
                .as_ref()
                .and_then(|d| d.acl_offline_path.as_ref())
                .map(PathBuf::from)
        });
    let openalex_offline_path = openalex_offline
        .or_else(|| {
            std::env::var("OPENALEX_OFFLINE_PATH")
                .ok()
                .map(PathBuf::from)
        })
        .or_else(|| {
            file_config
                .databases
                .as_ref()
                .and_then(|d| d.openalex_offline_path.as_ref())
                .map(PathBuf::from)
        });
    let db_timeout_secs: u64 = std::env::var("DB_TIMEOUT")
        .ok()
        .and_then(|v| v.parse().ok())
        .or_else(|| {
            file_config
                .concurrency
                .as_ref()
                .and_then(|c| c.db_timeout_secs)
        })
        .unwrap_or(10);
    let db_timeout_short_secs: u64 = std::env::var("DB_TIMEOUT_SHORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .or_else(|| {
            file_config
                .concurrency
                .as_ref()
                .and_then(|c| c.db_timeout_short_secs)
        })
        .unwrap_or(5);

    // SearxNG URL: --searxng flag > env var > config file
    let searxng_url = if searxng {
        let url = std::env::var("SEARXNG_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                file_config
                    .databases
                    .as_ref()
                    .and_then(|d| d.searxng_url.clone())
            })
            .unwrap_or_else(|| "http://localhost:8080".to_string());

        // Check connectivity and warn if not reachable
        let searxng = hallucinator_core::db::searxng::Searxng::new(url.clone());
        if let Err(msg) = searxng.check_connectivity().await {
            eprintln!("\x1b[33mWarning:\x1b[0m {}", msg);
        }

        Some(url)
    } else {
        // Even without --searxng flag, config file can enable it
        file_config
            .databases
            .as_ref()
            .and_then(|d| d.searxng_url.clone())
    };

    // Determine color mode and output writer
    let use_color = !no_color && output.is_none();
    let color = ColorMode(use_color);

    let mut writer: Box<dyn Write> = if let Some(ref output_path) = output {
        Box::new(std::fs::File::create(output_path)?)
    } else {
        Box::new(std::io::stdout())
    };

    // Open offline DBLP database if configured
    let dblp_offline_db = if let Some(ref path) = dblp_offline_path {
        if !path.exists() {
            anyhow::bail!(
                "Offline DBLP database not found at {}. Build it with: hallucinator-cli update-dblp {}",
                path.display(),
                path.display()
            );
        }
        let db = hallucinator_dblp::DblpDatabase::open(path)?;

        // Check staleness
        if let Ok(staleness) = db.check_staleness(30)
            && staleness.is_stale
        {
            let msg = if let Some(days) = staleness.age_days {
                format!(
                    "Offline DBLP database is {} days old. Consider running: hallucinator-cli update-dblp {}",
                    days,
                    path.display()
                )
            } else {
                format!(
                    "Offline DBLP database may be stale. Consider running: hallucinator-cli update-dblp {}",
                    path.display()
                )
            };
            if color.enabled() {
                use owo_colors::OwoColorize;
                writeln!(writer, "{}", msg.yellow())?;
            } else {
                writeln!(writer, "{}", msg)?;
            }
            writeln!(writer)?;
        }

        Some(Arc::new(Mutex::new(db)))
    } else {
        None
    };

    // Open offline ACL Anthology database if configured
    let acl_offline_db = if let Some(ref path) = acl_offline_path {
        if !path.exists() {
            anyhow::bail!(
                "Offline ACL database not found at {}. Build it with: hallucinator-cli update-acl {}",
                path.display(),
                path.display()
            );
        }
        let db = hallucinator_acl::AclDatabase::open(path)?;

        if let Ok(staleness) = db.check_staleness(30)
            && staleness.is_stale
        {
            let msg = if let Some(days) = staleness.age_days {
                format!(
                    "Offline ACL database is {} days old. Consider running: hallucinator-cli update-acl {}",
                    days,
                    path.display()
                )
            } else {
                format!(
                    "Offline ACL database may be stale. Consider running: hallucinator-cli update-acl {}",
                    path.display()
                )
            };
            if color.enabled() {
                use owo_colors::OwoColorize;
                writeln!(writer, "{}", msg.yellow())?;
            } else {
                writeln!(writer, "{}", msg)?;
            }
            writeln!(writer)?;
        }

        Some(Arc::new(Mutex::new(db)))
    } else {
        None
    };

    // Open offline OpenAlex index if configured
    let openalex_offline_db = if let Some(ref path) = openalex_offline_path {
        if !path.exists() {
            anyhow::bail!(
                "Offline OpenAlex index not found at {}. Build it with: hallucinator-cli update-openalex {}",
                path.display(),
                path.display()
            );
        }
        let db = hallucinator_openalex::OpenAlexDatabase::open(path)
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        if let Ok(staleness) = db.check_staleness(30)
            && staleness.is_stale
        {
            let msg = if let Some(days) = staleness.age_days {
                format!(
                    "Offline OpenAlex index is {} days old. Consider running: hallucinator-cli update-openalex {}",
                    days,
                    path.display()
                )
            } else {
                format!(
                    "Offline OpenAlex index may be stale. Consider running: hallucinator-cli update-openalex {}",
                    path.display()
                )
            };
            if color.enabled() {
                use owo_colors::OwoColorize;
                writeln!(writer, "{}", msg.yellow())?;
            } else {
                writeln!(writer, "{}", msg)?;
            }
            writeln!(writer)?;
        }

        Some(Arc::new(Mutex::new(db)))
    } else {
        None
    };

    if !file_path.exists() {
        anyhow::bail!("File not found: {}", file_path.display());
    }

    let crossref_mailto: Option<String> = std::env::var("CROSSREF_MAILTO")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            file_config
                .api_keys
                .as_ref()
                .and_then(|a| a.crossref_mailto.clone())
        });

    // Merge disable_dbs: CLI flags + config file disabled list
    let disable_dbs = if disable_dbs.is_empty() {
        file_config
            .databases
            .as_ref()
            .and_then(|d| d.disabled.clone())
            .unwrap_or_default()
    } else {
        disable_dbs
    };

    // Build config: CLI flags > env vars > config file > defaults
    let num_workers = num_workers
        .or_else(|| file_config.concurrency.as_ref().and_then(|c| c.num_workers))
        .unwrap_or(4);
    let max_rate_limit_retries = max_rate_limit_retries
        .or_else(|| {
            file_config
                .concurrency
                .as_ref()
                .and_then(|c| c.max_rate_limit_retries)
        })
        .unwrap_or(3);
    let rate_limiters = std::sync::Arc::new(hallucinator_core::RateLimiters::new(
        crossref_mailto.is_some(),
        s2_api_key.is_some(),
    ));

    let cache_path = cache_path
        .or_else(|| {
            std::env::var("HALLUCINATOR_CACHE_PATH")
                .ok()
                .map(PathBuf::from)
        })
        .or_else(|| {
            file_config
                .databases
                .as_ref()
                .and_then(|d| d.cache_path.as_ref())
                .map(PathBuf::from)
        });
    let positive_ttl = hallucinator_core::DEFAULT_POSITIVE_TTL.as_secs();
    let negative_ttl = hallucinator_core::DEFAULT_NEGATIVE_TTL.as_secs();
    let query_cache =
        hallucinator_core::build_query_cache(cache_path.as_deref(), positive_ttl, negative_ttl);

    let config = hallucinator_core::Config {
        openalex_key: openalex_key.clone(),
        s2_api_key,
        dblp_offline_path: dblp_offline_path.clone(),
        dblp_offline_db,
        acl_offline_path: acl_offline_path.clone(),
        acl_offline_db,
        openalex_offline_path: openalex_offline_path.clone(),
        openalex_offline_db,
        num_workers,
        db_timeout_secs,
        db_timeout_short_secs,
        disabled_dbs: disable_dbs,
        check_openalex_authors,
        crossref_mailto,
        max_rate_limit_retries,
        rate_limiters,
        searxng_url,
        query_cache: Some(query_cache),
        cache_path,
        cache_positive_ttl_secs: positive_ttl,
        cache_negative_ttl_secs: negative_ttl,
    };

    // Handle archives: extract each file and run check on each independently
    if hallucinator_ingest::is_archive_path(&file_path) {
        return run_archive_check(&file_path, config, output, color).await;
    }

    // Single file: extract then check
    let extraction = hallucinator_ingest::extract_references(&file_path)
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    let file_name = file_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| file_path.display().to_string());

    output::print_extraction_summary(
        &mut writer,
        &file_name,
        extraction.references.len(),
        &extraction.skip_stats,
        color,
    )?;

    if extraction.references.is_empty() {
        writeln!(writer, "No references to check.")?;
        return Ok(());
    }

    // Set up progress callback
    let progress_writer: Arc<Mutex<Box<dyn Write + Send>>> = if output.is_some() {
        Arc::new(Mutex::new(Box::new(std::io::stderr())))
    } else {
        Arc::new(Mutex::new(Box::new(std::io::stdout())))
    };

    let progress_color = color;
    let progress_cb = {
        let pw = Arc::clone(&progress_writer);
        move |event: hallucinator_core::ProgressEvent| {
            if let Ok(mut w) = pw.lock() {
                let _ = output::print_progress(&mut *w, &event, progress_color);
                let _ = w.flush();
            }
        }
    };

    let cancel = CancellationToken::new();

    // Set up Ctrl+C handler
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            cancel_clone.cancel();
        }
    });

    let skip_stats = extraction.skip_stats.clone();
    let results =
        hallucinator_core::check_references(extraction.references, config, progress_cb, cancel)
            .await;

    // Print final report
    writeln!(writer)?;

    output::print_hallucination_report(&mut writer, &results, openalex_key.is_some(), color)?;

    output::print_doi_issues(&mut writer, &results, color)?;
    output::print_retraction_warnings(&mut writer, &results, color)?;
    output::print_summary(&mut writer, &results, &skip_stats, color)?;

    Ok(())
}

/// Process all extractable files inside an archive, printing a per-file report for each.
async fn run_archive_check(
    archive_path: &std::path::Path,
    config: hallucinator_core::Config,
    output: Option<PathBuf>,
    color: ColorMode,
) -> anyhow::Result<()> {
    use hallucinator_ingest::archive::{ArchiveItem, extract_archive_streaming};

    let mut writer: Box<dyn Write> = if let Some(ref output_path) = output {
        Box::new(std::fs::File::create(output_path)?)
    } else {
        Box::new(std::io::stdout())
    };

    let archive_name = archive_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| archive_path.display().to_string());

    writeln!(writer, "Archive: {}", archive_name)?;
    writeln!(writer)?;

    let temp_dir = tempfile::tempdir()?;
    let (tx, rx) = std::sync::mpsc::channel::<ArchiveItem>();

    let archive_path = archive_path.to_path_buf();
    let dir = temp_dir.path().to_path_buf();
    let extract_handle =
        std::thread::spawn(move || extract_archive_streaming(&archive_path, &dir, 0, &tx));

    let mut file_count = 0usize;
    let config = Arc::new(config);

    for item in rx {
        match item {
            ArchiveItem::Warning(msg) => {
                writeln!(writer, "Warning: {}", msg)?;
            }
            ArchiveItem::Pdf(extracted) => {
                file_count += 1;

                // Print a header separator for each file
                writeln!(writer, "─── {} ───", extracted.filename)?;
                writeln!(writer)?;

                let path = extracted.path.clone();
                let extraction = match hallucinator_ingest::extract_references(&path) {
                    Ok(e) => e,
                    Err(e) => {
                        writeln!(writer, "  Error: {}", e)?;
                        writeln!(writer)?;
                        continue;
                    }
                };

                output::print_extraction_summary(
                    &mut writer,
                    &extracted.filename,
                    extraction.references.len(),
                    &extraction.skip_stats,
                    color,
                )?;

                if extraction.references.is_empty() {
                    writeln!(writer, "No references to check.")?;
                    writeln!(writer)?;
                    continue;
                }

                let progress_writer: Arc<Mutex<Box<dyn Write + Send>>> = if output.is_some() {
                    Arc::new(Mutex::new(Box::new(std::io::stderr())))
                } else {
                    Arc::new(Mutex::new(Box::new(std::io::stdout())))
                };
                let progress_color = color;
                let progress_cb = {
                    let pw = Arc::clone(&progress_writer);
                    move |event: hallucinator_core::ProgressEvent| {
                        if let Ok(mut w) = pw.lock() {
                            let _ = output::print_progress(&mut *w, &event, progress_color);
                            let _ = w.flush();
                        }
                    }
                };

                let cancel = CancellationToken::new();
                let cancel_clone = cancel.clone();
                tokio::spawn(async move {
                    if tokio::signal::ctrl_c().await.is_ok() {
                        cancel_clone.cancel();
                    }
                });

                let skip_stats = extraction.skip_stats.clone();
                let config_clone = Arc::clone(&config);
                let refs = extraction.references;

                let results = hallucinator_core::check_references(
                    refs,
                    (*config_clone).clone(),
                    progress_cb,
                    cancel,
                )
                .await;

                writeln!(writer)?;
                // Use the first openalex key for the report (openalex_key is in config)
                let has_openalex = config_clone.openalex_key.is_some();
                output::print_hallucination_report(&mut writer, &results, has_openalex, color)?;
                output::print_doi_issues(&mut writer, &results, color)?;
                output::print_retraction_warnings(&mut writer, &results, color)?;
                output::print_summary(&mut writer, &results, &skip_stats, color)?;
                writeln!(writer)?;
            }
            ArchiveItem::Done { total } => {
                writeln!(writer, "Processed {} file(s) from archive.", total)?;
            }
        }
    }

    extract_handle
        .join()
        .map_err(|_| anyhow::anyhow!("Archive extraction thread panicked"))?
        .map_err(|e| anyhow::anyhow!("Archive extraction failed: {}", e))?;

    if file_count == 0 {
        writeln!(writer, "No processable files found in archive.")?;
    }

    Ok(())
}

async fn dry_run_check(
    file_path: PathBuf,
    no_color: bool,
    output: Option<PathBuf>,
) -> anyhow::Result<()> {
    let use_color = !no_color && output.is_none();

    let mut writer: Box<dyn Write> = if let Some(ref output_path) = output {
        Box::new(std::fs::File::create(output_path)?)
    } else {
        Box::new(std::io::stdout())
    };

    if !file_path.exists() {
        anyhow::bail!("File not found: {}", file_path.display());
    }

    let file_name = file_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| file_path.display().to_string());

    let is_bbl = file_path
        .extension()
        .map(|e| e.eq_ignore_ascii_case("bbl"))
        .unwrap_or(false);
    let is_bib = file_path
        .extension()
        .map(|e| e.eq_ignore_ascii_case("bib"))
        .unwrap_or(false);

    if is_bbl || is_bib {
        dry_run_bbl(&file_path, &file_name, use_color, &mut writer)
    } else {
        dry_run_pdf(&file_path, &file_name, use_color, &mut writer)
    }
}

fn dry_run_pdf(
    file_path: &std::path::Path,
    file_name: &str,
    use_color: bool,
    writer: &mut Box<dyn Write>,
) -> anyhow::Result<()> {
    use owo_colors::OwoColorize;

    use hallucinator_pdf::PdfBackend as _;
    let text = hallucinator_pdf_mupdf::MupdfBackend
        .extract_text(file_path)
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    let ref_section = hallucinator_pdf::section::find_references_section(&text)
        .ok_or_else(|| anyhow::anyhow!("No references section found"))?;
    let raw_refs = hallucinator_pdf::section::segment_references(&ref_section);

    if use_color {
        writeln!(
            writer,
            "{} {} ({} raw references segmented)\n",
            "DRY RUN:".bold().cyan(),
            file_name.bold(),
            raw_refs.len()
        )?;
    } else {
        writeln!(
            writer,
            "DRY RUN: {} ({} raw references segmented)\n",
            file_name,
            raw_refs.len()
        )?;
    }

    for (i, ref_text) in raw_refs.iter().enumerate() {
        let doi = hallucinator_pdf::identifiers::extract_doi(ref_text);
        let arxiv_id = hallucinator_pdf::identifiers::extract_arxiv_id(ref_text);
        let (extracted_title, from_quotes) =
            hallucinator_pdf::title::extract_title_from_reference(ref_text);
        let cleaned_title = hallucinator_pdf::title::clean_title(&extracted_title, from_quotes);
        let authors = hallucinator_pdf::authors::extract_authors_from_reference(ref_text);

        // Normalize raw citation for display
        let raw_display: String = ref_text.split_whitespace().collect::<Vec<_>>().join(" ");
        let raw_display = if raw_display.len() > 200 {
            // Find a char boundary at or before position 200
            let boundary = raw_display
                .char_indices()
                .take_while(|(i, _)| *i <= 200)
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
            format!("{}...", &raw_display[..boundary])
        } else {
            raw_display
        };

        if use_color {
            writeln!(writer, "{}", format!("[{}]", i + 1).bold().yellow())?;
        } else {
            writeln!(writer, "[{}]", i + 1)?;
        }

        writeln!(writer, "  Title:   {}", cleaned_title)?;
        writeln!(
            writer,
            "  Authors: {}",
            if authors.is_empty() {
                "(none)".to_string()
            } else {
                authors.join("; ")
            }
        )?;

        if let Some(ref d) = doi {
            writeln!(writer, "  DOI:     {}", d)?;
        }
        if let Some(ref a) = arxiv_id {
            writeln!(writer, "  arXiv:   {}", a)?;
        }

        if use_color {
            writeln!(writer, "  Raw:     {}", raw_display.dimmed())?;
        } else {
            writeln!(writer, "  Raw:     {}", raw_display)?;
        }

        let word_count = cleaned_title.split_whitespace().count();
        if cleaned_title.is_empty() || word_count < 4 {
            // Check for strong signals that override the short-title skip
            let has_signal =
                !cleaned_title.is_empty() && (doi.is_some() || arxiv_id.is_some() || from_quotes);
            if !has_signal {
                if use_color {
                    writeln!(
                        writer,
                        "  {}",
                        format!("SKIPPED (title too short: {} words)", word_count).red()
                    )?;
                } else {
                    writeln!(writer, "  SKIPPED (title too short: {} words)", word_count)?;
                }
            }
        }

        writeln!(writer)?;
    }

    writeln!(writer, "Total: {} raw references", raw_refs.len())?;

    Ok(())
}

fn dry_run_bbl(
    file_path: &std::path::Path,
    file_name: &str,
    use_color: bool,
    writer: &mut Box<dyn Write>,
) -> anyhow::Result<()> {
    use owo_colors::OwoColorize;

    let is_bib = file_path
        .extension()
        .map(|e| e.eq_ignore_ascii_case("bib"))
        .unwrap_or(false);
    let extraction = if is_bib {
        hallucinator_bbl::extract_references_from_bib(file_path)
            .map_err(|e| anyhow::anyhow!("BIB extraction failed: {}", e))?
    } else {
        hallucinator_bbl::extract_references_from_bbl(file_path)
            .map_err(|e| anyhow::anyhow!("BBL extraction failed: {}", e))?
    };

    let total = extraction.skip_stats.total_raw;
    let kept = extraction.references.len();

    if use_color {
        writeln!(
            writer,
            "{} {} ({} entries, {} after filtering)\n",
            "DRY RUN:".bold().cyan(),
            file_name.bold(),
            total,
            kept
        )?;
    } else {
        writeln!(
            writer,
            "DRY RUN: {} ({} entries, {} after filtering)\n",
            file_name, total, kept
        )?;
    }

    for (i, reference) in extraction.references.iter().enumerate() {
        let title = reference.title.as_deref().unwrap_or("");

        if use_color {
            writeln!(writer, "{}", format!("[{}]", i + 1).bold().yellow())?;
        } else {
            writeln!(writer, "[{}]", i + 1)?;
        }

        writeln!(writer, "  Title:   {}", title)?;
        writeln!(
            writer,
            "  Authors: {}",
            if reference.authors.is_empty() {
                "(none)".to_string()
            } else {
                reference.authors.join("; ")
            }
        )?;

        if let Some(ref d) = reference.doi {
            writeln!(writer, "  DOI:     {}", d)?;
        }
        if let Some(ref a) = reference.arxiv_id {
            writeln!(writer, "  arXiv:   {}", a)?;
        }

        // Truncate raw citation for display
        let raw_display = if reference.raw_citation.len() > 200 {
            format!("{}...", &reference.raw_citation[..200])
        } else {
            reference.raw_citation.clone()
        };

        if use_color {
            writeln!(writer, "  Raw:     {}", raw_display.dimmed())?;
        } else {
            writeln!(writer, "  Raw:     {}", raw_display)?;
        }

        writeln!(writer)?;
    }

    let stats = &extraction.skip_stats;
    writeln!(
        writer,
        "Total: {} raw entries ({} kept, {} skipped: {} URL-only, {} short title, {} no title)",
        stats.total_raw,
        kept,
        stats.url_only + stats.short_title + stats.no_title,
        stats.url_only,
        stats.short_title,
        stats.no_title
    )?;

    Ok(())
}

async fn update_dblp(db_path: &PathBuf) -> anyhow::Result<()> {
    use indicatif::{HumanBytes, HumanCount, MultiProgress, ProgressBar, ProgressStyle};
    use std::time::{Duration, Instant};

    let multi = MultiProgress::new();

    let dl_bar_style = ProgressStyle::with_template(
        "{spinner:.cyan} {msg} [{bar:40.cyan/dim}] {bytes}/{total_bytes} ({bytes_per_sec}, eta {eta})",
    )
    .unwrap()
    .progress_chars("=> ");

    let dl_unknown_style =
        ProgressStyle::with_template("{spinner:.cyan} {msg} {bytes} ({bytes_per_sec})").unwrap();

    let parse_bar_style = ProgressStyle::with_template(
        "{spinner:.green} [{elapsed_precise}] {msg} [{bar:40.green/dim}] {percent}% (eta {eta})",
    )
    .unwrap()
    .progress_chars("=> ");

    let parse_spinner_style =
        ProgressStyle::with_template("{spinner:.green} [{elapsed_precise}] {msg}").unwrap();

    let dl_bar = multi.add(ProgressBar::new(0));
    dl_bar.set_style(dl_unknown_style.clone());
    dl_bar.set_message("Connecting to dblp.org...");
    dl_bar.enable_steady_tick(Duration::from_millis(120));

    let parse_bar = multi.add(ProgressBar::new(0));
    parse_bar.set_style(parse_spinner_style.clone());
    parse_bar.set_draw_target(indicatif::ProgressDrawTarget::hidden());

    let finalize_bar = multi.add(ProgressBar::new_spinner());
    finalize_bar.set_style(parse_spinner_style.clone());
    finalize_bar.set_draw_target(indicatif::ProgressDrawTarget::hidden());

    let build_start = Instant::now();
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
                dl_bar.set_message("Downloading dblp.xml.gz");
                if bytes_downloaded >= total && !dl_bar.is_finished() {
                    dl_bar.finish_with_message(format!(
                        "Downloaded {} in {:.0?}",
                        HumanBytes(total),
                        dl_bar.elapsed()
                    ));
                }
            } else {
                dl_bar.set_position(bytes_downloaded);
                dl_bar.set_message("Downloading dblp.xml.gz");
            }
        }
        hallucinator_dblp::BuildProgress::Parsing {
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
                parse_bar.reset_elapsed();
                parse_bar.set_draw_target(indicatif::ProgressDrawTarget::stderr());
                parse_bar.enable_steady_tick(Duration::from_millis(120));
            }
            // Switch to progress bar style on first event with a known total
            if bytes_total > 0 && parse_bar.length() == Some(0) {
                parse_bar.set_length(bytes_total);
                parse_bar.set_style(parse_bar_style.clone());
            }
            parse_bar.set_position(bytes_read);
            let elapsed = parse_start.get().unwrap().elapsed().as_secs_f64();
            let per_sec = if elapsed > 0.0 {
                records_inserted as f64 / elapsed
            } else {
                0.0
            };
            parse_bar.set_message(format!(
                "{} publications ({}/s)",
                HumanCount(records_inserted),
                HumanCount(per_sec as u64),
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
            if !parse_bar.is_finished() {
                let elapsed = parse_start.get().map(|s| s.elapsed());
                parse_bar.finish_with_message(format!(
                    "Inserted publications in {:.0?}",
                    elapsed.unwrap_or_default()
                ));
            }
            finalize_bar.reset_elapsed();
            finalize_bar.set_draw_target(indicatif::ProgressDrawTarget::stderr());
            finalize_bar.enable_steady_tick(Duration::from_millis(120));
            finalize_bar.set_message("Rebuilding FTS search index...");
        }
        hallucinator_dblp::BuildProgress::Compacting => {
            finalize_bar.set_message("Compacting database (VACUUM)...");
        }
        hallucinator_dblp::BuildProgress::Complete {
            publications,
            authors,
            skipped,
        } => {
            if !parse_bar.is_finished() {
                parse_bar.finish_and_clear();
            }
            if skipped {
                finalize_bar
                    .finish_with_message("Database is already up to date (304 Not Modified)");
            } else {
                finalize_bar.finish_with_message(format!(
                    "Indexed {} publications, {} authors (total {:.0?})",
                    HumanCount(publications),
                    HumanCount(authors),
                    build_start.elapsed()
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
    use std::time::{Duration, Instant};

    let multi = MultiProgress::new();

    let dl_bar_style = ProgressStyle::with_template(
        "{spinner:.cyan} {msg} [{bar:40.cyan/dim}] {bytes}/{total_bytes} ({bytes_per_sec}, eta {eta})",
    )
    .unwrap()
    .progress_chars("=> ");

    let dl_unknown_style =
        ProgressStyle::with_template("{spinner:.cyan} {msg} {bytes} ({bytes_per_sec})").unwrap();

    let parse_bar_style = ProgressStyle::with_template(
        "{spinner:.green} [{elapsed_precise}] {msg} [{bar:40.green/dim}] {percent}% (eta {eta})",
    )
    .unwrap()
    .progress_chars("=> ");

    let parse_spinner_style =
        ProgressStyle::with_template("{spinner:.green} [{elapsed_precise}] {msg}").unwrap();

    let dl_bar = multi.add(ProgressBar::new(0));
    dl_bar.set_style(dl_unknown_style.clone());
    dl_bar.set_message("Connecting to GitHub...");
    dl_bar.enable_steady_tick(Duration::from_millis(120));

    let parse_bar = multi.add(ProgressBar::new(0));
    parse_bar.set_style(parse_spinner_style.clone());
    parse_bar.set_draw_target(indicatif::ProgressDrawTarget::hidden());

    let finalize_bar = multi.add(ProgressBar::new_spinner());
    finalize_bar.set_style(parse_spinner_style.clone());
    finalize_bar.set_draw_target(indicatif::ProgressDrawTarget::hidden());

    let build_start = Instant::now();
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
                dl_bar.set_message("Downloading acl-anthology.tar.gz");
            } else {
                dl_bar.set_position(bytes_downloaded);
                dl_bar.set_message("Downloading acl-anthology.tar.gz");
            }
        }
        hallucinator_acl::BuildProgress::Extracting { files_extracted } => {
            if !dl_bar.is_finished() {
                dl_bar.finish_with_message(format!("Downloaded in {:.0?}", dl_bar.elapsed()));
            }
            if parse_bar.is_hidden() {
                parse_bar.reset_elapsed();
                parse_bar.set_draw_target(indicatif::ProgressDrawTarget::stderr());
                parse_bar.enable_steady_tick(Duration::from_millis(120));
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
                if parse_bar.is_hidden() {
                    parse_bar.reset_elapsed();
                    parse_bar.set_draw_target(indicatif::ProgressDrawTarget::stderr());
                    parse_bar.enable_steady_tick(Duration::from_millis(120));
                }
            }
            if files_total > 0 && parse_bar.length() == Some(0) {
                parse_bar.set_length(files_total);
                parse_bar.set_style(parse_bar_style.clone());
            }
            parse_bar.set_position(files_processed);
            let elapsed = parse_start.get().unwrap().elapsed().as_secs_f64();
            let per_sec = if elapsed > 0.0 {
                records_inserted as f64 / elapsed
            } else {
                0.0
            };
            parse_bar.set_message(format!(
                "{} parsed, {} inserted ({}/s)",
                HumanCount(records_parsed),
                HumanCount(records_inserted),
                HumanCount(per_sec as u64),
            ));
        }
        hallucinator_acl::BuildProgress::RebuildingIndex => {
            if !dl_bar.is_finished() {
                dl_bar.finish_with_message(format!("Downloaded in {:.0?}", dl_bar.elapsed()));
            }
            if !parse_bar.is_finished() {
                let elapsed = parse_start.get().map(|s| s.elapsed());
                parse_bar.finish_with_message(format!(
                    "Inserted publications in {:.0?}",
                    elapsed.unwrap_or_default()
                ));
            }
            finalize_bar.reset_elapsed();
            finalize_bar.set_draw_target(indicatif::ProgressDrawTarget::stderr());
            finalize_bar.enable_steady_tick(Duration::from_millis(120));
            finalize_bar.set_message("Rebuilding FTS search index...");
        }
        hallucinator_acl::BuildProgress::Complete {
            publications,
            authors,
            skipped,
        } => {
            if !parse_bar.is_finished() {
                parse_bar.finish_and_clear();
            }
            if skipped {
                finalize_bar
                    .finish_with_message("Database is already up to date (same commit SHA)");
            } else {
                finalize_bar.finish_with_message(format!(
                    "Indexed {} publications, {} authors (total {:.0?})",
                    HumanCount(publications),
                    HumanCount(authors),
                    build_start.elapsed()
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

async fn update_openalex(
    db_path: &PathBuf,
    since: Option<&str>,
    min_year: Option<u32>,
) -> anyhow::Result<()> {
    use indicatif::{HumanBytes, HumanCount, MultiProgress, ProgressBar, ProgressStyle};
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    if since.is_none() && min_year.is_none() {
        eprintln!("Warning: This may use ~15-30 GB of disk space for the full OpenAlex index.");
    }
    if let Some(since) = since {
        eprintln!("Only downloading S3 partitions newer than {since}");
    }
    if let Some(min_year) = min_year {
        eprintln!("Only indexing works published in {min_year} or later");
    }

    let multi = MultiProgress::new();

    let bar_style = ProgressStyle::with_template(
        "{spinner:.cyan} [{bar:40.cyan/dim}] {pos}/{len} files  {msg}",
    )
    .unwrap()
    .progress_chars("=> ");

    let spinner_style = ProgressStyle::with_template("{spinner:.cyan} {msg}").unwrap();
    let file_spinner_style = ProgressStyle::with_template("  {spinner:.dim} {msg:.dim}").unwrap();

    let bar = multi.add(ProgressBar::new(0));
    bar.set_style(spinner_style.clone());
    bar.set_message("Listing OpenAlex S3 partitions...");
    bar.enable_steady_tick(Duration::from_millis(120));

    let mut file_spinners: HashMap<String, ProgressBar> = HashMap::new();

    let build_start = Instant::now();

    let updated = hallucinator_openalex::build_database_filtered(
        db_path,
        since,
        min_year,
        |event| match event {
            hallucinator_openalex::BuildProgress::ListingPartitions { message } => {
                bar.set_message(message);
            }
            hallucinator_openalex::BuildProgress::FileStarted { filename } => {
                let s = multi.add(ProgressBar::new_spinner());
                s.set_style(file_spinner_style.clone());
                s.set_message(filename.clone());
                s.enable_steady_tick(Duration::from_millis(120));
                file_spinners.insert(filename, s);
            }
            hallucinator_openalex::BuildProgress::FileComplete { filename } => {
                if let Some(s) = file_spinners.remove(&filename) {
                    s.finish_and_clear();
                }
            }
            hallucinator_openalex::BuildProgress::FileProgress {
                filename,
                bytes_downloaded,
            } => {
                if let Some(s) = file_spinners.get(&filename) {
                    s.set_message(format!("{} ({})", filename, HumanBytes(bytes_downloaded)));
                }
            }
            hallucinator_openalex::BuildProgress::Downloading {
                files_done,
                files_total,
                bytes_downloaded,
                records_indexed,
            } => {
                if bar.length() == Some(0) && files_total > 0 {
                    bar.set_length(files_total);
                    bar.set_style(bar_style.clone());
                }
                bar.set_position(files_done);
                let elapsed = build_start.elapsed().as_secs_f64();
                let speed = if elapsed > 0.5 {
                    format!(
                        " ({}/s)",
                        HumanBytes((bytes_downloaded as f64 / elapsed) as u64)
                    )
                } else {
                    String::new()
                };
                let rate = if elapsed > 0.5 && records_indexed > 0 {
                    format!(
                        ", {} records ({}/s)",
                        HumanCount(records_indexed),
                        HumanCount((records_indexed as f64 / elapsed) as u64)
                    )
                } else if records_indexed > 0 {
                    format!(", {} records", HumanCount(records_indexed))
                } else {
                    String::new()
                };
                bar.set_message(format!("{}{}{}", HumanBytes(bytes_downloaded), speed, rate));
            }
            hallucinator_openalex::BuildProgress::Committing { records_indexed } => {
                bar.set_message(format!(
                    "Committing... {} records",
                    HumanCount(records_indexed)
                ));
            }
            hallucinator_openalex::BuildProgress::FileSkipped { filename, error } => {
                if let Some(s) = file_spinners.remove(&filename) {
                    s.finish_and_clear();
                }
                bar.suspend(|| {
                    eprintln!("Warning: skipped {} ({error})", filename);
                });
            }
            hallucinator_openalex::BuildProgress::Merging => {
                for (_, s) in file_spinners.drain() {
                    s.finish_and_clear();
                }
                bar.set_message("Merging index segments...");
            }
            hallucinator_openalex::BuildProgress::Complete {
                publications,
                skipped,
                failed_files,
            } => {
                if skipped {
                    bar.finish_with_message("Index is already up to date");
                } else {
                    let warn = if failed_files.is_empty() {
                        String::new()
                    } else {
                        format!(" ({} files failed)", failed_files.len())
                    };
                    bar.finish_with_message(format!(
                        "Indexed {} publications (total {:.0?}){}",
                        HumanCount(publications),
                        build_start.elapsed(),
                        warn,
                    ));
                }
            }
        },
    )
    .await
    .map_err(|e| anyhow::anyhow!("{}", e))?;

    let canonical = std::fs::canonicalize(db_path).unwrap_or_else(|_| db_path.clone());
    if !updated {
        println!("Index is already up to date: {}", canonical.display());
    } else {
        println!("OpenAlex index saved to: {}", canonical.display());
    }

    Ok(())
}
