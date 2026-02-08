use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use clap::Parser;
use tokio_util::sync::CancellationToken;

mod output;

use output::ColorMode;

/// Hallucinated Reference Detector - Detect fabricated references in academic PDFs
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to the PDF file to check
    pdf_path: PathBuf,

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
    #[arg(long)]
    output: Option<PathBuf>,

    /// Path to offline DBLP database
    #[arg(long)]
    dblp_offline: Option<PathBuf>,

    /// Download and build offline DBLP database at the given path
    #[arg(long)]
    update_dblp: Option<PathBuf>,

    /// Comma-separated list of databases to disable
    #[arg(long, value_delimiter = ',')]
    disable_dbs: Vec<String>,

    /// Flag author mismatches from OpenAlex (default: skipped)
    #[arg(long)]
    check_openalex_authors: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let args = Args::parse();

    // Handle --update-dblp (exclusive mode)
    if let Some(ref db_path) = args.update_dblp {
        return update_dblp(db_path);
    }

    // Resolve configuration: CLI flags > env vars > defaults
    let openalex_key = args
        .openalex_key
        .or_else(|| std::env::var("OPENALEX_KEY").ok());
    let s2_api_key = args
        .s2_api_key
        .or_else(|| std::env::var("S2_API_KEY").ok());
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

    // Determine color mode and output writer
    let use_color = !args.no_color && args.output.is_none();
    let color = ColorMode(use_color);

    let mut writer: Box<dyn Write> = if let Some(ref output_path) = args.output {
        Box::new(std::fs::File::create(output_path)?)
    } else {
        Box::new(std::io::stdout())
    };

    // Open offline DBLP database if configured
    let dblp_offline_db = if let Some(ref path) = dblp_offline_path {
        if !path.exists() {
            anyhow::bail!(
                "Offline DBLP database not found at {}. Use --update-dblp={} to build it.",
                path.display(),
                path.display()
            );
        }
        let db = hallucinator_dblp::DblpDatabase::open(path)?;

        // Check staleness
        if let Ok(staleness) = db.check_staleness(30) {
            if staleness.is_stale {
                let msg = if let Some(days) = staleness.age_days {
                    format!(
                        "Offline DBLP database is {} days old. Consider running --update-dblp={} to refresh.",
                        days,
                        path.display()
                    )
                } else {
                    format!(
                        "Offline DBLP database may be stale. Consider running --update-dblp={} to refresh.",
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
        }

        Some(Arc::new(Mutex::new(db)))
    } else {
        None
    };

    // Extract references from PDF
    let pdf_path = &args.pdf_path;
    if !pdf_path.exists() {
        anyhow::bail!("PDF file not found: {}", pdf_path.display());
    }

    let extraction = hallucinator_pdf::extract_references(pdf_path)?;
    let pdf_name = pdf_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| pdf_path.display().to_string());

    output::print_extraction_summary(
        &mut writer,
        &pdf_name,
        extraction.references.len(),
        &extraction.skip_stats,
        color,
    )?;

    if extraction.references.is_empty() {
        writeln!(writer, "No references to check.")?;
        return Ok(());
    }

    // Build config
    let config = hallucinator_core::Config {
        openalex_key: openalex_key.clone(),
        s2_api_key,
        dblp_offline_path: dblp_offline_path.clone(),
        dblp_offline_db,
        max_concurrent_refs: 4,
        db_timeout_secs,
        db_timeout_short_secs,
        disabled_dbs: args.disable_dbs,
        check_openalex_authors: args.check_openalex_authors,
    };

    // Set up progress callback
    // We use a Mutex<Box<dyn Write>> so the callback can write progress
    let progress_writer: Arc<Mutex<Box<dyn Write + Send>>> = if args.output.is_some() {
        // When writing to file, progress goes to the file too
        // But we already consumed `writer`, so reopen
        // Actually, let's write progress to stderr when output is a file
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
    let results = hallucinator_core::check_references(
        extraction.references,
        config,
        progress_cb,
        cancel,
    )
    .await;

    // Print final report
    writeln!(writer)?;

    output::print_hallucination_report(
        &mut writer,
        &results,
        openalex_key.is_some(),
        color,
    )?;

    output::print_doi_issues(&mut writer, &results, color)?;
    output::print_retraction_warnings(&mut writer, &results, color)?;
    output::print_summary(&mut writer, &results, &skip_stats, color)?;

    Ok(())
}

fn update_dblp(db_path: &PathBuf) -> anyhow::Result<()> {
    println!("Building offline DBLP database at {}...", db_path.display());
    println!("This will download ~4.6 GB and may take a while.");
    println!();

    let updated = hallucinator_dblp::build_database(db_path, |event| match event {
        hallucinator_dblp::BuildProgress::Downloading {
            bytes_downloaded,
            total_bytes,
        } => {
            let mb = bytes_downloaded / (1024 * 1024);
            if let Some(total) = total_bytes {
                let total_mb = total / (1024 * 1024);
                eprint!("\rDownloading: {} / {} MB", mb, total_mb);
            } else {
                eprint!("\rDownloading: {} MB", mb);
            }
        }
        hallucinator_dblp::BuildProgress::Parsing {
            lines_processed,
            records_inserted,
        } => {
            if lines_processed % 1_000_000 == 0 {
                eprint!(
                    "\rParsing: {}M lines, {} records",
                    lines_processed / 1_000_000,
                    records_inserted
                );
            }
        }
        hallucinator_dblp::BuildProgress::RebuildingIndex => {
            eprintln!();
            eprintln!("Rebuilding FTS index...");
        }
        hallucinator_dblp::BuildProgress::Complete {
            publications,
            authors,
            skipped,
        } => {
            eprintln!();
            if skipped {
                println!("Database is already up to date (server returned 304).");
            } else {
                println!(
                    "Done! {} publications, {} authors indexed.",
                    publications, authors
                );
            }
        }
    })?;

    if !updated {
        println!("Database is already up to date.");
    }

    Ok(())
}
