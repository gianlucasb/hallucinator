use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::model::paper::{RefPhase, RefState};

/// Copy text to the system clipboard via OSC 52 escape sequence.
/// Works in Ghostty, iTerm2, kitty, WezTerm, and most modern terminals.
pub(super) fn osc52_copy(text: &str) {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    // Write directly to stdout, bypassing the terminal backend buffer
    let _ = std::io::stdout().write_all(format!("\x1b]52;c;{}\x07", encoded).as_bytes());
    let _ = std::io::stdout().flush();
}

/// Default path for offline databases: `~/.local/share/hallucinator/<filename>`.
pub(super) fn default_db_path(filename: &str) -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("hallucinator")
        .join(filename)
}

/// Format a DBLP build progress event into a short status string.
pub(super) fn format_dblp_progress(
    event: &hallucinator_dblp::BuildProgress,
    build_started: Option<Instant>,
    parse_started: Option<Instant>,
) -> String {
    match event {
        hallucinator_dblp::BuildProgress::Downloading {
            bytes_downloaded,
            total_bytes,
            ..
        } => {
            let speed = build_started
                .map(|s| {
                    let elapsed = s.elapsed().as_secs_f64();
                    if elapsed > 0.5 {
                        format!(
                            " {}/s",
                            format_bytes((*bytes_downloaded as f64 / elapsed) as u64)
                        )
                    } else {
                        String::new()
                    }
                })
                .unwrap_or_default();
            if let Some(total) = total_bytes {
                let pct = (*bytes_downloaded as f64 / *total as f64 * 100.0) as u32;
                let eta = format_eta(*bytes_downloaded, *total, build_started);
                format!(
                    "Downloading... {} / {} ({}%){}{}",
                    format_bytes(*bytes_downloaded),
                    format_bytes(*total),
                    pct,
                    speed,
                    eta
                )
            } else {
                format!(
                    "Downloading... {}{}",
                    format_bytes(*bytes_downloaded),
                    speed
                )
            }
        }
        hallucinator_dblp::BuildProgress::Parsing {
            records_inserted,
            bytes_read,
            bytes_total,
        } => {
            let pct = if *bytes_total > 0 {
                (*bytes_read as f64 / *bytes_total as f64 * 100.0) as u32
            } else {
                0
            };
            let rate = parse_started
                .map(|s| {
                    let elapsed = s.elapsed().as_secs_f64();
                    if elapsed > 0.5 {
                        format!(
                            " ({}/s)",
                            format_number((*records_inserted as f64 / elapsed) as u64)
                        )
                    } else {
                        String::new()
                    }
                })
                .unwrap_or_default();
            let eta = format_eta(*bytes_read, *bytes_total, parse_started);
            format!(
                "Parsing... {} publications ({}%){}{}",
                format_number(*records_inserted),
                pct,
                rate,
                eta
            )
        }
        hallucinator_dblp::BuildProgress::RebuildingIndex => "Rebuilding FTS index...".to_string(),
        hallucinator_dblp::BuildProgress::Compacting => {
            "Compacting database (VACUUM)...".to_string()
        }
        hallucinator_dblp::BuildProgress::Complete {
            publications,
            authors,
            skipped,
        } => {
            if *skipped {
                "Already up to date (304 Not Modified)".to_string()
            } else {
                format!(
                    "Complete: {} publications, {} authors",
                    format_number(*publications),
                    format_number(*authors)
                )
            }
        }
    }
}

/// Format an ACL build progress event into a short status string.
pub(super) fn format_acl_progress(
    event: &hallucinator_acl::BuildProgress,
    build_started: Option<Instant>,
    parse_started: Option<Instant>,
) -> String {
    match event {
        hallucinator_acl::BuildProgress::Downloading {
            bytes_downloaded,
            total_bytes,
        } => {
            let speed = build_started
                .map(|s| {
                    let elapsed = s.elapsed().as_secs_f64();
                    if elapsed > 0.5 {
                        format!(
                            " {}/s",
                            format_bytes((*bytes_downloaded as f64 / elapsed) as u64)
                        )
                    } else {
                        String::new()
                    }
                })
                .unwrap_or_default();
            if let Some(total) = total_bytes {
                let pct = (*bytes_downloaded as f64 / *total as f64 * 100.0) as u32;
                let eta = format_eta(*bytes_downloaded, *total, build_started);
                format!(
                    "Downloading... {} / {} ({}%){}{}",
                    format_bytes(*bytes_downloaded),
                    format_bytes(*total),
                    pct,
                    speed,
                    eta
                )
            } else {
                format!(
                    "Downloading... {}{}",
                    format_bytes(*bytes_downloaded),
                    speed
                )
            }
        }
        hallucinator_acl::BuildProgress::Extracting { files_extracted } => {
            format!("Extracting... {} files", format_number(*files_extracted))
        }
        hallucinator_acl::BuildProgress::Parsing {
            records_parsed,
            records_inserted,
            files_processed,
            files_total,
        } => {
            let rate = parse_started
                .map(|s| {
                    let elapsed = s.elapsed().as_secs_f64();
                    if elapsed > 0.5 {
                        format!(
                            " ({}/s)",
                            format_number((*records_inserted as f64 / elapsed) as u64)
                        )
                    } else {
                        String::new()
                    }
                })
                .unwrap_or_default();
            let eta = format_eta(*files_processed, *files_total, parse_started);
            format!(
                "Parsing... {} records, {} inserted ({}/{}){}{}",
                format_number(*records_parsed),
                format_number(*records_inserted),
                files_processed,
                files_total,
                rate,
                eta
            )
        }
        hallucinator_acl::BuildProgress::RebuildingIndex => "Rebuilding FTS index...".to_string(),
        hallucinator_acl::BuildProgress::Complete {
            publications,
            authors,
            skipped,
        } => {
            if *skipped {
                "Already up to date (same commit SHA)".to_string()
            } else {
                format!(
                    "Complete: {} publications, {} authors",
                    format_number(*publications),
                    format_number(*authors)
                )
            }
        }
    }
}

/// Format OpenAlex build progress for display in the config panel.
/// Returns `None` for transient file-level events (keep previous status).
pub(super) fn format_openalex_progress(
    event: &hallucinator_openalex::BuildProgress,
    build_started: Option<Instant>,
) -> Option<String> {
    Some(match event {
        hallucinator_openalex::BuildProgress::ListingPartitions { message } => message.clone(),
        hallucinator_openalex::BuildProgress::FileStarted { .. }
        | hallucinator_openalex::BuildProgress::FileComplete { .. }
        | hallucinator_openalex::BuildProgress::FileProgress { .. } => return None,
        hallucinator_openalex::BuildProgress::Downloading {
            files_done,
            files_total,
            bytes_downloaded,
            records_indexed,
        } => {
            let speed = build_started
                .map(|s| {
                    let elapsed = s.elapsed().as_secs_f64();
                    if elapsed > 0.5 {
                        format!(
                            " {}/s",
                            format_bytes((*bytes_downloaded as f64 / elapsed) as u64)
                        )
                    } else {
                        String::new()
                    }
                })
                .unwrap_or_default();
            let rate = build_started
                .map(|s| {
                    let elapsed = s.elapsed().as_secs_f64();
                    if elapsed > 0.5 {
                        format!(
                            " ({}/s)",
                            format_number((*records_indexed as f64 / elapsed) as u64)
                        )
                    } else {
                        String::new()
                    }
                })
                .unwrap_or_default();
            let eta = format_eta(*files_done, *files_total, build_started);
            format!(
                "Downloading... {}/{} files, {} indexed, {}{}{}",
                files_done,
                files_total,
                format_number(*records_indexed),
                format_bytes(*bytes_downloaded),
                speed,
                if rate.is_empty() {
                    eta
                } else {
                    format!("{}{}", rate, eta)
                }
            )
        }
        hallucinator_openalex::BuildProgress::Committing { records_indexed } => {
            format!(
                "Committing... {} records indexed",
                format_number(*records_indexed)
            )
        }
        hallucinator_openalex::BuildProgress::Merging => "Merging index segments...".to_string(),
        hallucinator_openalex::BuildProgress::FileSkipped { .. } => return None,
        hallucinator_openalex::BuildProgress::Complete {
            publications,
            skipped,
            ..
        } => {
            if *skipped {
                "Already up to date".to_string()
            } else {
                format!(
                    "Complete: {} publications indexed",
                    format_number(*publications)
                )
            }
        }
    })
}

/// Format an ETA string from progress and elapsed time.
pub(super) fn format_eta(done: u64, total: u64, started: Option<Instant>) -> String {
    if done == 0 || total == 0 {
        return String::new();
    }
    let Some(started) = started else {
        return String::new();
    };
    let elapsed = started.elapsed().as_secs_f64();
    if elapsed < 1.0 {
        return String::new();
    }
    let fraction = done as f64 / total as f64;
    if fraction <= 0.0 || fraction > 1.0 {
        return String::new();
    }
    let remaining_secs = (elapsed / fraction - elapsed).max(0.0) as u64;
    if remaining_secs < 60 {
        format!(", eta {}s", remaining_secs)
    } else {
        format!(", eta {}m{}s", remaining_secs / 60, remaining_secs % 60)
    }
}

/// Format a number with comma separators (e.g. 1,234,567).
pub(super) fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

/// Canonicalize a path and strip the Windows `\\?\` extended-length prefix.
pub(super) fn clean_canonicalize(path: &Path) -> String {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let s = canonical.display().to_string();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}

pub(super) fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

pub(super) fn verdict_sort_key(rs: &RefState) -> u8 {
    if matches!(rs.phase, RefPhase::Skipped(_)) {
        return 5; // sort skipped refs last
    }
    match &rs.result {
        Some(r) => {
            if r.retraction_info.as_ref().is_some_and(|ri| ri.is_retracted) {
                0
            } else {
                match &r.status {
                    hallucinator_core::Status::NotFound => 1,
                    hallucinator_core::Status::Mismatch(_) => 2,
                    hallucinator_core::Status::Verified => 3,
                }
            }
        }
        None => 4,
    }
}

/// Build a one-time warning message for a DB that is repeatedly failing.
pub(super) fn db_failure_warning(
    db_name: &str,
    rate_limited_count: usize,
    has_dblp_offline: bool,
) -> String {
    let kind = if rate_limited_count > 0 {
        "rate-limited (429)"
    } else {
        "failing"
    };

    match db_name {
        "OpenAlex" => {
            format!("OpenAlex {kind} repeatedly — check API key and credit balance at openalex.org")
        }
        "Semantic Scholar" => {
            format!("Semantic Scholar {kind} repeatedly — check API key at semanticscholar.org")
        }
        "CrossRef" => format!(
            "CrossRef {kind} repeatedly — set crossref_mailto in config for higher rate limits"
        ),
        "DBLP" if !has_dblp_offline => format!(
            "DBLP online {kind} repeatedly — build an offline database: hallucinator-tui update-dblp"
        ),
        _ => format!("{db_name} {kind} repeatedly"),
    }
}

/// Get the process resident set size (working set) in bytes.
/// Returns what Task Manager / top / Activity Monitor would show.
pub(super) fn get_rss_bytes() -> Option<usize> {
    #[cfg(target_os = "windows")]
    {
        #[repr(C)]
        #[allow(non_snake_case)]
        struct ProcessMemoryCounters {
            cb: u32,
            PageFaultCount: u32,
            PeakWorkingSetSize: usize,
            WorkingSetSize: usize,
            QuotaPeakPagedPoolUsage: usize,
            QuotaPagedPoolUsage: usize,
            QuotaPeakNonPagedPoolUsage: usize,
            QuotaNonPagedPoolUsage: usize,
            PagefileUsage: usize,
            PeakPagefileUsage: usize,
        }
        unsafe extern "system" {
            fn GetCurrentProcess() -> isize;
            fn K32GetProcessMemoryInfo(
                process: isize,
                ppsmemCounters: *mut ProcessMemoryCounters,
                cb: u32,
            ) -> i32;
        }
        unsafe {
            let mut pmc: ProcessMemoryCounters = std::mem::zeroed();
            pmc.cb = std::mem::size_of::<ProcessMemoryCounters>() as u32;
            if K32GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, pmc.cb) != 0 {
                return Some(pmc.WorkingSetSize);
            }
        }
        None
    }
    #[cfg(target_os = "linux")]
    {
        let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
        let rss_pages: usize = statm.split_whitespace().nth(1)?.parse().ok()?;
        Some(rss_pages * 4096)
    }
    #[cfg(target_os = "macos")]
    {
        #[repr(C)]
        struct MachTaskBasicInfo {
            virtual_size: u64,
            resident_size: u64,
            resident_size_max: u64,
            user_time: [u32; 2],
            system_time: [u32; 2],
            policy: i32,
            suspend_count: i32,
        }
        unsafe extern "C" {
            fn mach_task_self() -> u32;
            fn task_info(
                target_task: u32,
                flavor: u32,
                task_info_out: *mut MachTaskBasicInfo,
                task_info_count: *mut u32,
            ) -> i32;
        }
        const MACH_TASK_BASIC_INFO: u32 = 20;
        unsafe {
            let mut info: MachTaskBasicInfo = std::mem::zeroed();
            let mut count =
                (std::mem::size_of::<MachTaskBasicInfo>() / std::mem::size_of::<u32>()) as u32;
            if task_info(
                mach_task_self(),
                MACH_TASK_BASIC_INFO,
                &mut info,
                &mut count,
            ) == 0
            {
                return Some(info.resident_size as usize);
            }
        }
        None
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        None
    }
}
