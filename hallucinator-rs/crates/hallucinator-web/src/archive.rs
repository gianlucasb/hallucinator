use flate2::read::GzDecoder;
use std::io::Read;
use std::path::{Path, PathBuf};
use tar::Archive;

const MAX_FILES: usize = 50;
const MAX_EXTRACTED_SIZE: u64 = 500 * 1024 * 1024; // 500MB

/// A PDF file extracted from an archive.
pub struct ExtractedPdf {
    pub path: PathBuf,
    pub filename: String,
}

/// Extract PDF files from a ZIP archive.
pub fn extract_from_zip(data: &[u8], dir: &Path) -> Result<Vec<ExtractedPdf>, String> {
    let cursor = std::io::Cursor::new(data);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| format!("Failed to open ZIP: {}", e))?;

    let mut pdfs = Vec::new();
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
            .map_or(true, |f| f.to_string_lossy().starts_with('.'))
        {
            continue;
        }

        // Only process PDFs
        if !name_str.to_lowercase().ends_with(".pdf") {
            continue;
        }

        // Check limits
        total_size += file.size();
        if total_size > MAX_EXTRACTED_SIZE {
            return Err(format!(
                "Archive exceeds maximum extracted size of {}MB",
                MAX_EXTRACTED_SIZE / 1024 / 1024
            ));
        }

        if pdfs.len() >= MAX_FILES {
            return Err(format!(
                "Archive contains more than {} PDF files",
                MAX_FILES
            ));
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

        // Verify PDF magic bytes
        if !buf.starts_with(b"%PDF-") {
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
        return Err("No PDF files found in archive".to_string());
    }

    Ok(pdfs)
}

/// Extract PDF files from a tar.gz archive.
pub fn extract_from_tar_gz(data: &[u8], dir: &Path) -> Result<Vec<ExtractedPdf>, String> {
    let gz = GzDecoder::new(data);
    let mut archive = Archive::new(gz);

    let entries = archive
        .entries()
        .map_err(|e| format!("Failed to read tar.gz: {}", e))?;

    let mut pdfs = Vec::new();
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
            .map_or(true, |f| f.to_string_lossy().starts_with('.'))
        {
            continue;
        }

        // Only process PDFs
        if !name_str.to_lowercase().ends_with(".pdf") {
            continue;
        }

        // Check limits
        total_size += entry.size();
        if total_size > MAX_EXTRACTED_SIZE {
            return Err(format!(
                "Archive exceeds maximum extracted size of {}MB",
                MAX_EXTRACTED_SIZE / 1024 / 1024
            ));
        }

        if pdfs.len() >= MAX_FILES {
            return Err(format!(
                "Archive contains more than {} PDF files",
                MAX_FILES
            ));
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

        // Verify PDF magic bytes
        if !buf.starts_with(b"%PDF-") {
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
        return Err("No PDF files found in archive".to_string());
    }

    Ok(pdfs)
}
