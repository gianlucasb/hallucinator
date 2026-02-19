//! JSON sidecar metadata for Tantivy index directories.
//!
//! Since Tantivy has no built-in key-value store, we persist build metadata
//! in a `metadata.json` file alongside the index segments.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::OpenAlexError;

const METADATA_FILENAME: &str = "openalex_metadata.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexMetadata {
    /// Schema version for compatibility checks.
    pub schema_version: String,
    /// Unix timestamp of the last build.
    pub build_date: Option<String>,
    /// Number of publications in the index.
    pub publication_count: Option<u64>,
    /// The newest partition date synced (YYYY-MM-DD).
    pub last_sync_date: Option<String>,
}

impl Default for IndexMetadata {
    fn default() -> Self {
        Self {
            schema_version: "1".to_string(),
            build_date: None,
            publication_count: None,
            last_sync_date: None,
        }
    }
}

/// Read metadata from the index directory. Returns default if file missing.
pub fn read_metadata(dir: &Path) -> Result<IndexMetadata, OpenAlexError> {
    let path = dir.join(METADATA_FILENAME);
    if !path.exists() {
        return Ok(IndexMetadata::default());
    }
    let content = std::fs::read_to_string(&path)?;
    serde_json::from_str(&content).map_err(|e| OpenAlexError::Parse(e.to_string()))
}

/// Write metadata to the index directory.
pub fn write_metadata(dir: &Path, meta: &IndexMetadata) -> Result<(), OpenAlexError> {
    let path = dir.join(METADATA_FILENAME);
    let content =
        serde_json::to_string_pretty(meta).map_err(|e| OpenAlexError::Parse(e.to_string()))?;
    std::fs::write(&path, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let meta = IndexMetadata {
            schema_version: "1".to_string(),
            build_date: Some("1700000000".to_string()),
            publication_count: Some(42),
            last_sync_date: Some("2025-01-15".to_string()),
        };
        write_metadata(dir.path(), &meta).unwrap();
        let loaded = read_metadata(dir.path()).unwrap();
        assert_eq!(loaded.schema_version, "1");
        assert_eq!(loaded.publication_count, Some(42));
        assert_eq!(loaded.last_sync_date.as_deref(), Some("2025-01-15"));
    }

    #[test]
    fn missing_metadata_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let meta = read_metadata(dir.path()).unwrap();
        assert_eq!(meta.schema_version, "1");
        assert!(meta.build_date.is_none());
        assert!(meta.publication_count.is_none());
    }
}
