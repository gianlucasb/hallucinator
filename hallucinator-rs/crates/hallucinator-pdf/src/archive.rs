use flate2::read::GzDecoder;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use tar::Archive;

/// A PDF file extracted from an archive.
pub struct ExtractedPdf {
    pub path: PathBuf,
    pub filename: String,
}

/// Result of archive extraction — includes any warnings (e.g. size limit reached).
pub struct ExtractionResult {
    pub pdfs: Vec<ExtractedPdf>,
    pub warnings: Vec<String>,
}

/// Item sent through the channel during streaming archive extraction.
pub enum ArchiveItem {
    /// A single extracted PDF.
    Pdf(ExtractedPdf),
    /// A warning (e.g. size limit reached).
    Warning(String),
    /// Extraction finished; `total` is the number of PDFs sent.
    Done { total: usize },
}

/// Returns true if the given path looks like a supported archive.
pub fn is_archive_path(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    name.ends_with(".zip") || name.ends_with(".tar.gz") || name.ends_with(".tgz")
}

/// Returns true if the filename is a supported extractable type (PDF, BBL, or BIB).
fn is_extractable(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".pdf") || lower.ends_with(".bbl") || lower.ends_with(".bib")
}

/// Returns true if the file content looks valid for its extension.
/// PDFs must start with `%PDF-`; BBL and BIB files are plain text and skip the check.
fn passes_magic_check(name: &str, data: &[u8]) -> bool {
    if name.to_lowercase().ends_with(".pdf") {
        data.starts_with(b"%PDF-")
    } else {
        true // .bbl/.bib files are plain text, no magic bytes to check
    }
}

/// Read an archive file from disk, detect its type, and extract PDFs into `dir`.
///
/// Supports ZIP and tar.gz archives. Type is detected by extension and magic bytes.
/// `max_size` limits total extracted bytes (0 = unlimited). When the limit is reached,
/// extraction stops and a warning is included in the result.
pub fn extract_archive(
    archive_path: &Path,
    dir: &Path,
    max_size: u64,
) -> Result<ExtractionResult, String> {
    let data = std::fs::read(archive_path)
        .map_err(|e| format!("Failed to read archive {}: {}", archive_path.display(), e))?;

    let name = archive_path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    // Detect by extension first, then fall back to magic bytes
    if name.ends_with(".zip") || data.starts_with(b"PK") {
        extract_from_zip(&data, dir, max_size)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") || data.starts_with(&[0x1f, 0x8b])
    {
        extract_from_tar_gz(&data, dir, max_size)
    } else {
        Err(format!(
            "Unsupported archive format: {}",
            archive_path.display()
        ))
    }
}

/// Extract PDF files from a ZIP archive.
/// `max_size` limits total extracted bytes (0 = unlimited).
pub fn extract_from_zip(
    data: &[u8],
    dir: &Path,
    max_size: u64,
) -> Result<ExtractionResult, String> {
    let cursor = std::io::Cursor::new(data);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| format!("Failed to open ZIP: {}", e))?;

    let mut pdfs = Vec::new();
    let mut warnings = Vec::new();
    let mut total_size: u64 = 0;

    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| format!("Failed to read ZIP entry: {}", e))?;

        let name = match file.enclosed_name() {
            Some(p) => p.to_path_buf(),
            None => continue, // Skip path traversal attempts
        };

        let name_str = name.to_string_lossy().to_string();

        // Skip directories, hidden files, macOS resource forks
        if file.is_dir() {
            continue;
        }
        if name_str.contains("__MACOSX") {
            continue;
        }
        if name
            .file_name()
            .is_none_or(|f| f.to_string_lossy().starts_with('.'))
        {
            continue;
        }

        // Only process PDFs, BBL, and BIB files
        if !is_extractable(&name_str) {
            continue;
        }

        // Check size limit
        if max_size > 0 {
            total_size += file.size();
            if total_size > max_size {
                warnings.push(format!(
                    "Size limit ({}MB) reached after {} files, skipping remaining",
                    max_size / 1024 / 1024,
                    pdfs.len()
                ));
                break;
            }
        }

        // Extract to temp dir with flat name
        let basename = name
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let out_name = format!("{}_{}", i, basename);
        let out_path = dir.join(&out_name);

        let mut buf = Vec::new();
        file.read_to_end(&mut buf)
            .map_err(|e| format!("Failed to extract {}: {}", name_str, e))?;

        // Verify magic bytes (PDFs only)
        if !passes_magic_check(&name_str, &buf) {
            continue;
        }

        std::fs::write(&out_path, &buf)
            .map_err(|e| format!("Failed to write {}: {}", out_name, e))?;

        pdfs.push(ExtractedPdf {
            path: out_path,
            filename: basename,
        });
    }

    if pdfs.is_empty() {
        return Err("No PDF, BBL, or BIB files found in archive".to_string());
    }

    Ok(ExtractionResult { pdfs, warnings })
}

/// Extract PDF files from a tar.gz archive (batch).
/// `max_size` limits total extracted bytes (0 = unlimited).
pub fn extract_from_tar_gz(
    data: &[u8],
    dir: &Path,
    max_size: u64,
) -> Result<ExtractionResult, String> {
    let gz = GzDecoder::new(data);
    let mut archive = Archive::new(gz);

    let entries = archive
        .entries()
        .map_err(|e| format!("Failed to read tar.gz: {}", e))?;

    let mut pdfs = Vec::new();
    let mut warnings = Vec::new();
    let mut total_size: u64 = 0;

    for (i, entry) in entries.enumerate() {
        let mut entry = entry.map_err(|e| format!("Failed to read tar entry: {}", e))?;

        let path = entry
            .path()
            .map_err(|e| format!("Failed to read entry path: {}", e))?
            .to_path_buf();
        let name_str = path.to_string_lossy().to_string();

        // Skip directories, hidden files, macOS resource forks
        if entry.header().entry_type().is_dir() {
            continue;
        }
        if name_str.contains("__MACOSX") {
            continue;
        }
        // Check for path traversal
        if name_str.contains("..") || name_str.starts_with('/') {
            continue;
        }
        if path
            .file_name()
            .is_none_or(|f| f.to_string_lossy().starts_with('.'))
        {
            continue;
        }

        // Only process PDFs, BBL, and BIB files
        if !is_extractable(&name_str) {
            continue;
        }

        // Check size limit
        if max_size > 0 {
            total_size += entry.size();
            if total_size > max_size {
                warnings.push(format!(
                    "Size limit ({}MB) reached after {} files, skipping remaining",
                    max_size / 1024 / 1024,
                    pdfs.len()
                ));
                break;
            }
        }

        // Extract to temp dir
        let basename = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let out_name = format!("{}_{}", i, basename);
        let out_path = dir.join(&out_name);

        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|e| format!("Failed to extract {}: {}", name_str, e))?;

        // Verify magic bytes (PDFs only)
        if !passes_magic_check(&name_str, &buf) {
            continue;
        }

        std::fs::write(&out_path, &buf)
            .map_err(|e| format!("Failed to write {}: {}", out_name, e))?;

        pdfs.push(ExtractedPdf {
            path: out_path,
            filename: basename,
        });
    }

    if pdfs.is_empty() {
        return Err("No PDF, BBL, or BIB files found in archive".to_string());
    }

    Ok(ExtractionResult { pdfs, warnings })
}

/// Stream-extract PDFs, BBL, and BIB files from an archive, sending each one through `tx` as it's extracted.
///
/// This is the streaming counterpart of [`extract_archive`]. Instead of collecting all
/// PDFs into a Vec, each extracted PDF is sent immediately via the channel so the caller
/// can process them incrementally. On completion, sends `ArchiveItem::Done`.
pub fn extract_archive_streaming(
    archive_path: &Path,
    dir: &Path,
    max_size: u64,
    tx: &mpsc::Sender<ArchiveItem>,
) -> Result<(), String> {
    let data = std::fs::read(archive_path)
        .map_err(|e| format!("Failed to read archive {}: {}", archive_path.display(), e))?;

    let name = archive_path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    if name.ends_with(".zip") || data.starts_with(b"PK") {
        extract_from_zip_streaming(&data, dir, max_size, tx)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") || data.starts_with(&[0x1f, 0x8b])
    {
        extract_from_tar_gz_streaming(&data, dir, max_size, tx)
    } else {
        Err(format!(
            "Unsupported archive format: {}",
            archive_path.display()
        ))
    }
}

/// Streaming ZIP extraction — sends each file through the channel as it's extracted.
fn extract_from_zip_streaming(
    data: &[u8],
    dir: &Path,
    max_size: u64,
    tx: &mpsc::Sender<ArchiveItem>,
) -> Result<(), String> {
    let cursor = std::io::Cursor::new(data);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| format!("Failed to open ZIP: {}", e))?;

    let mut total: usize = 0;
    let mut total_size: u64 = 0;

    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| format!("Failed to read ZIP entry: {}", e))?;

        let name = match file.enclosed_name() {
            Some(p) => p.to_path_buf(),
            None => continue,
        };

        let name_str = name.to_string_lossy().to_string();

        if file.is_dir() {
            continue;
        }
        if name_str.contains("__MACOSX") {
            continue;
        }
        if name
            .file_name()
            .is_none_or(|f| f.to_string_lossy().starts_with('.'))
        {
            continue;
        }
        if !is_extractable(&name_str) {
            continue;
        }

        if max_size > 0 {
            total_size += file.size();
            if total_size > max_size {
                let _ = tx.send(ArchiveItem::Warning(format!(
                    "Size limit ({}MB) reached after {} files, skipping remaining",
                    max_size / 1024 / 1024,
                    total
                )));
                break;
            }
        }

        let basename = name
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let out_name = format!("{}_{}", i, basename);
        let out_path = dir.join(&out_name);

        let mut buf = Vec::new();
        file.read_to_end(&mut buf)
            .map_err(|e| format!("Failed to extract {}: {}", name_str, e))?;

        if !passes_magic_check(&name_str, &buf) {
            continue;
        }

        std::fs::write(&out_path, &buf)
            .map_err(|e| format!("Failed to write {}: {}", out_name, e))?;

        total += 1;
        if tx
            .send(ArchiveItem::Pdf(ExtractedPdf {
                path: out_path,
                filename: basename,
            }))
            .is_err()
        {
            return Ok(());
        }
    }

    if total == 0 {
        return Err("No PDF, BBL, or BIB files found in archive".to_string());
    }

    let _ = tx.send(ArchiveItem::Done { total });
    Ok(())
}

/// Streaming tar.gz extraction — sends each file through the channel as it's extracted.
fn extract_from_tar_gz_streaming(
    data: &[u8],
    dir: &Path,
    max_size: u64,
    tx: &mpsc::Sender<ArchiveItem>,
) -> Result<(), String> {
    let gz = GzDecoder::new(data);
    let mut archive = Archive::new(gz);

    let entries = archive
        .entries()
        .map_err(|e| format!("Failed to read tar.gz: {}", e))?;

    let mut total: usize = 0;
    let mut total_size: u64 = 0;

    for (i, entry) in entries.enumerate() {
        let mut entry = entry.map_err(|e| format!("Failed to read tar entry: {}", e))?;

        let path = entry
            .path()
            .map_err(|e| format!("Failed to read entry path: {}", e))?
            .to_path_buf();
        let name_str = path.to_string_lossy().to_string();

        if entry.header().entry_type().is_dir() {
            continue;
        }
        if name_str.contains("__MACOSX") {
            continue;
        }
        if name_str.contains("..") || name_str.starts_with('/') {
            continue;
        }
        if path
            .file_name()
            .is_none_or(|f| f.to_string_lossy().starts_with('.'))
        {
            continue;
        }
        if !is_extractable(&name_str) {
            continue;
        }

        if max_size > 0 {
            total_size += entry.size();
            if total_size > max_size {
                let _ = tx.send(ArchiveItem::Warning(format!(
                    "Size limit ({}MB) reached after {} files, skipping remaining",
                    max_size / 1024 / 1024,
                    total
                )));
                break;
            }
        }

        let basename = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let out_name = format!("{}_{}", i, basename);
        let out_path = dir.join(&out_name);

        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|e| format!("Failed to extract {}: {}", name_str, e))?;

        if !passes_magic_check(&name_str, &buf) {
            continue;
        }

        std::fs::write(&out_path, &buf)
            .map_err(|e| format!("Failed to write {}: {}", out_name, e))?;

        total += 1;
        if tx
            .send(ArchiveItem::Pdf(ExtractedPdf {
                path: out_path,
                filename: basename,
            }))
            .is_err()
        {
            return Ok(());
        }
    }

    if total == 0 {
        return Err("No PDF, BBL, or BIB files found in archive".to_string());
    }

    let _ = tx.send(ArchiveItem::Done { total });
    Ok(())
}
