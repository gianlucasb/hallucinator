//! Kaggle dataset download for the arXiv metadata snapshot.
//!
//! Uses Kaggle's public API (Basic auth with username + key). Streams
//! the zip to disk instead of buffering in RAM so the ~4 GB payload
//! doesn't blow up memory usage. Credentials come from either the
//! standard `~/.kaggle/kaggle.json` file (same location `kaggle` CLI
//! uses) or from `KAGGLE_USERNAME` + `KAGGLE_KEY` env vars — env vars
//! win when both are set.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::ArxivError;

/// Kaggle slug for the Cornell-published arXiv metadata snapshot.
pub const KAGGLE_DATASET: &str = "Cornell-University/arxiv";

/// Name of the single JSONL file inside the downloaded zip. Hard-coded
/// because the Kaggle dataset has contained exactly one file for
/// years; if that ever changes we'd rather fail loud than guess.
pub const KAGGLE_DUMP_ENTRY: &str = "arxiv-metadata-oai-snapshot.json";

/// Progress events emitted while downloading.
#[derive(Debug, Clone)]
pub enum DownloadProgress {
    /// Download request accepted. `total_bytes` is the `Content-Length`
    /// the server reports (may be `None` on chunked transfers).
    Started { total_bytes: Option<u64> },
    /// Periodic byte-count update (roughly every 2 MB).
    Progress {
        bytes_downloaded: u64,
        total_bytes: Option<u64>,
    },
    /// Download finished successfully. `bytes` is the final payload
    /// size as written to disk.
    Complete { bytes: u64, elapsed: Duration },
}

#[derive(Debug, Deserialize)]
struct KaggleCredentials {
    username: String,
    key: String,
}

/// Locate Kaggle credentials. Tries `KAGGLE_USERNAME` + `KAGGLE_KEY`
/// first, then `~/.kaggle/kaggle.json`. Error messages point at the
/// Kaggle settings page so first-time users know where to go.
pub fn load_credentials() -> Result<(String, String), ArxivError> {
    if let (Ok(u), Ok(k)) = (
        std::env::var("KAGGLE_USERNAME"),
        std::env::var("KAGGLE_KEY"),
    ) && !u.is_empty()
        && !k.is_empty()
    {
        return Ok((u, k));
    }
    let home = dirs::home_dir()
        .ok_or_else(|| ArxivError::Harvest("HOME directory not found".into()))?;
    let path = home.join(".kaggle").join("kaggle.json");
    let file = File::open(&path).map_err(|e| {
        ArxivError::Harvest(format!(
            "Kaggle credentials missing at {}: {e}\n\
             Set KAGGLE_USERNAME + KAGGLE_KEY env vars, or place kaggle.json there.\n\
             Get a token at https://www.kaggle.com/settings → \"Create New Token\".",
            path.display()
        ))
    })?;
    let creds: KaggleCredentials = serde_json::from_reader(file).map_err(|e| {
        ArxivError::Harvest(format!("parsing {}: {e}", path.display()))
    })?;
    if creds.username.is_empty() || creds.key.is_empty() {
        return Err(ArxivError::Harvest(format!(
            "{} contains an empty username or key",
            path.display()
        )));
    }
    Ok((creds.username, creds.key))
}

/// Download the Kaggle dataset zip to `dest_path`, streaming to disk.
/// Returns the number of bytes written. `reqwest` follows the
/// Kaggle → S3 pre-signed-URL redirect automatically.
pub async fn download_kaggle_zip<P>(
    dest_path: &Path,
    mut progress: P,
) -> Result<u64, ArxivError>
where
    P: FnMut(DownloadProgress),
{
    let (user, key) = load_credentials()?;
    let url = format!("https://www.kaggle.com/api/v1/datasets/download/{KAGGLE_DATASET}");
    // 1 h cap: 4 GB on a slow home link (~5 Mbit/s) takes roughly that
    // long. Longer than the default reqwest timeout, shorter than
    // "forever" so a wedged connection doesn't hang the CLI.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3600))
        .user_agent(concat!(
            "hallucinator-arxiv-offline/",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .map_err(|e| ArxivError::Harvest(format!("http client: {e}")))?;
    let start = Instant::now();
    let resp = client
        .get(&url)
        .basic_auth(&user, Some(&key))
        .send()
        .await
        .map_err(|e| ArxivError::Harvest(format!("kaggle request: {e}")))?;

    if resp.status() == reqwest::StatusCode::UNAUTHORIZED
        || resp.status() == reqwest::StatusCode::FORBIDDEN
    {
        return Err(ArxivError::Harvest(format!(
            "Kaggle returned HTTP {} — credentials rejected, or you haven't accepted \
             the dataset license. Open https://www.kaggle.com/datasets/{KAGGLE_DATASET} \
             in a browser once to accept, then retry.",
            resp.status()
        )));
    }
    if !resp.status().is_success() {
        return Err(ArxivError::Harvest(format!(
            "Kaggle returned HTTP {}",
            resp.status()
        )));
    }

    let total_bytes = resp.content_length();
    progress(DownloadProgress::Started { total_bytes });

    if let Some(parent) = dest_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = File::create(dest_path)?;
    let mut downloaded: u64 = 0;
    let mut since_last_tick: u64 = 0;
    const TICK_BYTES: u64 = 2 * 1024 * 1024;

    // resp.bytes_stream() would be nicer but requires the futures
    // crate; .chunk() is zero-dep and reads one reqwest chunk at a
    // time, which is exactly what we want.
    let mut resp = resp;
    loop {
        let chunk = resp
            .chunk()
            .await
            .map_err(|e| ArxivError::Harvest(format!("chunk read: {e}")))?;
        let Some(chunk) = chunk else { break };
        file.write_all(&chunk)?;
        downloaded += chunk.len() as u64;
        since_last_tick += chunk.len() as u64;
        if since_last_tick >= TICK_BYTES {
            progress(DownloadProgress::Progress {
                bytes_downloaded: downloaded,
                total_bytes,
            });
            since_last_tick = 0;
        }
    }
    file.flush()?;

    progress(DownloadProgress::Complete {
        bytes: downloaded,
        elapsed: start.elapsed(),
    });
    Ok(downloaded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_env_vars_win() {
        // SAFETY: single-threaded test; no other threads reading env.
        unsafe {
            std::env::set_var("KAGGLE_USERNAME", "env-user");
            std::env::set_var("KAGGLE_KEY", "env-key");
        }
        let (u, k) = load_credentials().unwrap();
        assert_eq!(u, "env-user");
        assert_eq!(k, "env-key");
        unsafe {
            std::env::remove_var("KAGGLE_USERNAME");
            std::env::remove_var("KAGGLE_KEY");
        }
    }

    #[test]
    fn missing_credentials_has_helpful_message() {
        unsafe {
            std::env::remove_var("KAGGLE_USERNAME");
            std::env::remove_var("KAGGLE_KEY");
        }
        // We can't easily hide the real ~/.kaggle/kaggle.json, but the
        // error message contract is still exercised when it's absent.
        // Skip if the dev machine happens to have one.
        let path = dirs::home_dir()
            .map(|h| h.join(".kaggle").join("kaggle.json"));
        if path.as_ref().is_some_and(|p| p.exists()) {
            return;
        }
        let err = load_credentials().unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("kaggle.json"), "got: {msg}");
        assert!(msg.contains("Create New Token"), "got: {msg}");
    }
}
