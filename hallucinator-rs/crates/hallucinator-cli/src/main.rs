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
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Check a PDF for hallucinated references
    Check {
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
        #[arg(short, long)]
        output: Option<PathBuf>,

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

        /// Dry run: extract and print references without querying databases
        #[arg(long)]
        dry_run: bool,
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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();

    match cli.command {
        Command::UpdateDblp { path } => update_dblp(&path).await,
        Command::UpdateAcl { path } => update_acl(&path).await,
        Command::Check {
            pdf_path,
            no_color,
            openalex_key,
            s2_api_key,
            output,
            dblp_offline,
            acl_offline,
            disable_dbs,
            check_openalex_authors,
            dry_run,
        } => {
            if dry_run {
                dry_run_check(pdf_path, no_color, output).await
            } else {
                check(
                    pdf_path,
                    no_color,
                    openalex_key,
                    s2_api_key,
                    output,
                    dblp_offline,
                    acl_offline,
                    disable_dbs,
                    check_openalex_authors,
                )
                .await
            }
        }
    }
}

async fn check(
    pdf_path: PathBuf,
    no_color: bool,
    openalex_key: Option<String>,
    s2_api_key: Option<String>,
    output: Option<PathBuf>,
    dblp_offline: Option<PathBuf>,
    acl_offline: Option<PathBuf>,
    disable_dbs: Vec<String>,
    check_openalex_authors: bool,
) -> anyhow::Result<()> {
    // Resolve configuration: CLI flags > env vars > defaults
    let openalex_key = openalex_key.or_else(|| std::env::var("OPENALEX_KEY").ok());
    let s2_api_key = s2_api_key.or_else(|| std::env::var("S2_API_KEY").ok());
    let dblp_offline_path =
        dblp_offline.or_else(|| std::env::var("DBLP_OFFLINE_PATH").ok().map(PathBuf::from));
    let acl_offline_path =
        acl_offline.or_else(|| std::env::var("ACL_OFFLINE_PATH").ok().map(PathBuf::from));
    let db_timeout_secs: u64 = std::env::var("DB_TIMEOUT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let db_timeout_short_secs: u64 = std::env::var("DB_TIMEOUT_SHORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);

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
        if let Ok(staleness) = db.check_staleness(30) {
            if staleness.is_stale {
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

        if let Ok(staleness) = db.check_staleness(30) {
            if staleness.is_stale {
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
        }

        Some(Arc::new(Mutex::new(db)))
    } else {
        None
    };

    // Extract references from PDF
    if !pdf_path.exists() {
        anyhow::bail!("PDF file not found: {}", pdf_path.display());
    }

    let extraction = hallucinator_pdf::extract_references(&pdf_path)?;
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
        acl_offline_path: acl_offline_path.clone(),
        acl_offline_db,
        max_concurrent_refs: 4,
        db_timeout_secs,
        db_timeout_short_secs,
        disabled_dbs: disable_dbs,
        check_openalex_authors,
        crossref_mailto: None,
    };

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

async fn dry_run_check(
    pdf_path: PathBuf,
    no_color: bool,
    output: Option<PathBuf>,
) -> anyhow::Result<()> {
    use owo_colors::OwoColorize;

    let use_color = !no_color && output.is_none();

    let mut writer: Box<dyn Write> = if let Some(ref output_path) = output {
        Box::new(std::fs::File::create(output_path)?)
    } else {
        Box::new(std::io::stdout())
    };

    if !pdf_path.exists() {
        anyhow::bail!("PDF file not found: {}", pdf_path.display());
    }

    let text = hallucinator_pdf::extract::extract_text_from_pdf(&pdf_path)?;
    let ref_section = hallucinator_pdf::section::find_references_section(&text)
        .ok_or_else(|| anyhow::anyhow!("No references section found"))?;
    let raw_refs = hallucinator_pdf::section::segment_references(&ref_section);

    let pdf_name = pdf_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| pdf_path.display().to_string());

    if use_color {
        writeln!(
            writer,
            "{} {} ({} raw references segmented)\n",
            "DRY RUN:".bold().cyan(),
            pdf_name.bold(),
            raw_refs.len()
        )?;
    } else {
        writeln!(
            writer,
            "DRY RUN: {} ({} raw references segmented)\n",
            pdf_name,
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
        let raw_display: String = ref_text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let raw_display = if raw_display.len() > 200 {
            format!("{}...", &raw_display[..200])
        } else {
            raw_display
        };

        if use_color {
            writeln!(
                writer,
                "{}",
                format!("[{}]", i + 1).bold().yellow()
            )?;
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
            if use_color {
                writeln!(
                    writer,
                    "  {}",
                    format!("SKIPPED (title too short: {} words)", word_count)
                        .red()
                )?;
            } else {
                writeln!(writer, "  SKIPPED (title too short: {} words)", word_count)?;
            }
        }

        writeln!(writer)?;
    }

    writeln!(writer, "Total: {} raw references", raw_refs.len())?;

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
            // Switch to progress bar style on first event with a known total
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
                dl_bar.finish_with_message(format!(
                    "Downloaded in {:.0?}",
                    dl_bar.elapsed()
                ));
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
                dl_bar.finish_with_message(format!(
                    "Downloaded in {:.0?}",
                    dl_bar.elapsed()
                ));
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
                dl_bar.finish_with_message(format!(
                    "Downloaded in {:.0?}",
                    dl_bar.elapsed()
                ));
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
