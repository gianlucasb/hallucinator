//! Kaggle dataset download for the arXiv metadata snapshot.
//!
//! Streams the zip to disk instead of buffering it in RAM so the ~4 GB
//! payload doesn't blow up memory usage.
//!
//! Kaggle has two generations of credentials in circulation and we
//! accept both:
//!
//! - **API tokens** — one opaque `KGAT_…` string, which is what the
//!   settings page hands out now. Sent as `Authorization: Bearer …`.
//!   Read from the `KAGGLE_API_TOKEN` env var or
//!   `~/.kaggle/access_token`.
//! - **Legacy API keys** — the older username + key pair from
//!   `kaggle.json`. Still honoured by the API, and still obtainable
//!   under "Legacy API Credentials". Sent as HTTP Basic auth. Read
//!   from `KAGGLE_USERNAME` + `KAGGLE_KEY` or `~/.kaggle/kaggle.json`.
//!
//! Source order matches the official Kaggle SDK: every token source
//! before any legacy key source, env vars before files within each
//! tier.

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

/// Plain-text token files the Kaggle settings page tells users to
/// create. The `.txt` sibling is checked because Windows editors like
/// to append the extension — the official SDK checks it for the same
/// reason.
const ACCESS_TOKEN_FILENAMES: [&str; 2] = ["access_token", "access_token.txt"];

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

/// A Kaggle credential together with the wire format it has to be
/// presented in.
#[derive(Clone, PartialEq, Eq)]
pub enum KaggleAuth {
    /// New-style API token (`KGAT_…`), sent as a bearer token.
    Token(String),
    /// Legacy username + API key pair, sent as HTTP Basic auth.
    ApiKey { username: String, key: String },
}

impl KaggleAuth {
    /// Attach the credential to an outgoing request.
    pub fn apply(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self {
            Self::Token(token) => request.bearer_auth(token),
            Self::ApiKey { username, key } => request.basic_auth(username, Some(key)),
        }
    }

    /// Human-readable credential kind, for progress and error output.
    /// Never includes the secret itself.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Token(_) => "API token",
            Self::ApiKey { .. } => "legacy username + key",
        }
    }
}

/// Redacted on purpose — this type flows through error paths that get
/// printed and logged.
impl std::fmt::Debug for KaggleAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Token(_) => f.write_str("Token(<redacted>)"),
            Self::ApiKey { username, .. } => f
                .debug_struct("ApiKey")
                .field("username", username)
                .field("key", &"<redacted>")
                .finish(),
        }
    }
}

/// Legacy `kaggle.json` shape. Both fields default so a file missing
/// one of them lands on the "empty username or key" message instead of
/// a serde error nobody can act on.
#[derive(Debug, Deserialize)]
struct LegacyCredentialsFile {
    #[serde(default)]
    username: String,
    #[serde(default)]
    key: String,
}

/// Read a token out of a file. Missing, unreadable and empty-after-
/// trimming are all treated alike: no token here, try the next source.
fn read_token_file(path: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;
    let token = contents.trim();
    if token.is_empty() {
        return None;
    }
    Some(token.to_string())
}

/// `KAGGLE_API_TOKEN` normally holds the token itself. The official SDK
/// also accepts a path to a file containing one, so tooling that can
/// only hand over file paths keeps working; mirror that.
fn token_from_env() -> Option<String> {
    let value = std::env::var("KAGGLE_API_TOKEN").ok()?;
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let as_path = Path::new(value);
    if as_path.is_file() {
        return read_token_file(as_path);
    }
    Some(value.to_string())
}

fn legacy_key_from_env() -> Option<KaggleAuth> {
    let username = std::env::var("KAGGLE_USERNAME").ok()?;
    let key = std::env::var("KAGGLE_KEY").ok()?;
    if username.is_empty() || key.is_empty() {
        return None;
    }
    Some(KaggleAuth::ApiKey { username, key })
}

/// Parse a legacy `kaggle.json`. Unlike the token files, a file that
/// exists but doesn't parse is a hard error rather than a fallthrough —
/// the user clearly meant to authenticate with it.
fn legacy_key_from_file(path: &Path) -> Result<KaggleAuth, ArxivError> {
    let file = File::open(path)
        .map_err(|e| ArxivError::Harvest(format!("opening {}: {e}", path.display())))?;
    let creds: LegacyCredentialsFile = serde_json::from_reader(file)
        .map_err(|e| ArxivError::Harvest(format!("parsing {}: {e}", path.display())))?;
    if creds.username.is_empty() || creds.key.is_empty() {
        return Err(ArxivError::Harvest(format!(
            "{} contains an empty username or key",
            path.display()
        )));
    }
    Ok(KaggleAuth::ApiKey {
        username: creds.username,
        key: creds.key,
    })
}

/// Locate a Kaggle credential. See the module docs for the source
/// order; the error walks first-time users through creating a token.
pub fn load_credentials() -> Result<KaggleAuth, ArxivError> {
    let kaggle_dir = dirs::home_dir().map(|home| home.join(".kaggle"));
    load_credentials_from(kaggle_dir.as_deref())
}

/// Resolution against an explicit config dir. Split out so tests can
/// point at a temp dir rather than the developer's real `~/.kaggle`.
fn load_credentials_from(kaggle_dir: Option<&Path>) -> Result<KaggleAuth, ArxivError> {
    if let Some(token) = token_from_env() {
        return Ok(KaggleAuth::Token(token));
    }
    if let Some(dir) = kaggle_dir {
        for filename in ACCESS_TOKEN_FILENAMES {
            if let Some(token) = read_token_file(&dir.join(filename)) {
                return Ok(KaggleAuth::Token(token));
            }
        }
    }
    if let Some(auth) = legacy_key_from_env() {
        return Ok(auth);
    }
    if let Some(dir) = kaggle_dir {
        let path = dir.join("kaggle.json");
        if path.exists() {
            return legacy_key_from_file(&path);
        }
    }

    Err(ArxivError::Harvest(missing_credentials_message(kaggle_dir)))
}

/// First-run guidance. Leads with the token flow because that is what
/// the settings page offers now, and mentions `kaggle.json` after it so
/// users who already have one still recognise their setup.
fn missing_credentials_message(kaggle_dir: Option<&Path>) -> String {
    let token_file = match kaggle_dir {
        Some(dir) => dir.join("access_token").display().to_string(),
        None => "~/.kaggle/access_token".to_string(),
    };
    format!(
        "Kaggle credentials not found.\n\
         Create a token in the API section of https://www.kaggle.com/settings, then either:\n\
         - export KAGGLE_API_TOKEN=<token>\n\
         - or save the token to {token_file}\n\
         Legacy credentials still work too: set KAGGLE_USERNAME + KAGGLE_KEY, or place a \
         kaggle.json next to the path above."
    )
}

/// Download the Kaggle dataset zip to `dest_path`, streaming to disk.
/// Returns the number of bytes written. `reqwest` follows the
/// Kaggle → cloud-storage pre-signed-URL redirect automatically.
pub async fn download_kaggle_zip<P>(dest_path: &Path, mut progress: P) -> Result<u64, ArxivError>
where
    P: FnMut(DownloadProgress),
{
    let auth = load_credentials()?;
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
    let resp = auth
        .apply(client.get(&url))
        .send()
        .await
        .map_err(|e| ArxivError::Harvest(format!("kaggle request: {e}")))?;

    if resp.status() == reqwest::StatusCode::UNAUTHORIZED
        || resp.status() == reqwest::StatusCode::FORBIDDEN
    {
        return Err(ArxivError::Harvest(format!(
            "Kaggle returned HTTP {} for your {} — the credential was rejected (tokens \
             expire and can be revoked), or you haven't accepted the dataset license. \
             Open https://www.kaggle.com/datasets/{KAGGLE_DATASET} in a browser once to \
             accept, then retry.",
            resp.status(),
            auth.kind()
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
    use std::sync::{Mutex, MutexGuard};

    /// Every credential env var the loader consults, so a test can
    /// start from a known-empty environment.
    const CREDENTIAL_VARS: [&str; 3] = ["KAGGLE_API_TOKEN", "KAGGLE_USERNAME", "KAGGLE_KEY"];

    /// Env vars are process-global, so these tests have to run one at a
    /// time.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Clears every credential env var for the duration of a test, so a
    /// test only sees the sources it sets up itself and a developer's
    /// own `KAGGLE_*` exports can't change the outcome.
    struct EnvGuard {
        _lock: MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn new() -> Self {
            // A test that panics mid-assert poisons the lock; the env is
            // still cleaned up by Drop, so keep going.
            let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            clear_credential_vars();
            Self { _lock: lock }
        }

        fn set(&self, key: &str, value: &str) {
            // SAFETY: ENV_LOCK serialises every test that touches env.
            unsafe { std::env::set_var(key, value) }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            clear_credential_vars();
        }
    }

    fn clear_credential_vars() {
        // SAFETY: callers hold ENV_LOCK.
        unsafe {
            for var in CREDENTIAL_VARS {
                std::env::remove_var(var);
            }
        }
    }

    /// Creates `<dir>/<name>` with `contents`, standing in for a file in
    /// the user's `~/.kaggle`.
    fn write_config_file(dir: &Path, name: &str, contents: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), contents).unwrap();
    }

    fn authorization_header(auth: &KaggleAuth) -> String {
        let request = auth
            .apply(reqwest::Client::new().get("https://www.kaggle.com/"))
            .build()
            .unwrap();
        request
            .headers()
            .get(reqwest::header::AUTHORIZATION)
            .expect("credential set no Authorization header")
            .to_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn token_goes_out_as_a_bearer_header() {
        let auth = KaggleAuth::Token("KGAT_testtoken".to_string());
        assert_eq!(authorization_header(&auth), "Bearer KGAT_testtoken");
    }

    #[test]
    fn legacy_key_goes_out_as_basic_auth() {
        let auth = KaggleAuth::ApiKey {
            username: "user".to_string(),
            key: "key".to_string(),
        };
        // base64("user:key")
        assert_eq!(authorization_header(&auth), "Basic dXNlcjprZXk=");
    }

    #[test]
    fn debug_does_not_leak_the_secret() {
        let token = format!("{:?}", KaggleAuth::Token("KGAT_secret".to_string()));
        assert!(!token.contains("KGAT_secret"), "got: {token}");
        let legacy = format!(
            "{:?}",
            KaggleAuth::ApiKey {
                username: "user".to_string(),
                key: "supersecret".to_string(),
            }
        );
        assert!(!legacy.contains("supersecret"), "got: {legacy}");
        assert!(legacy.contains("user"), "got: {legacy}");
    }

    #[test]
    fn token_env_var_wins_over_every_other_source() {
        let dir = tempfile::tempdir().unwrap();
        let env = EnvGuard::new();
        env.set("KAGGLE_API_TOKEN", "KGAT_fromenv");
        env.set("KAGGLE_USERNAME", "env-user");
        env.set("KAGGLE_KEY", "env-key");
        write_config_file(dir.path(), "access_token", "KGAT_fromfile");
        write_config_file(dir.path(), "kaggle.json", r#"{"username":"f","key":"k"}"#);
        assert_eq!(
            load_credentials_from(Some(dir.path())).unwrap(),
            KaggleAuth::Token("KGAT_fromenv".to_string())
        );
    }

    /// Kaggle's own tooling accepts a path in `KAGGLE_API_TOKEN`, so a
    /// secret mounted as a file works without a wrapper script.
    #[test]
    fn token_env_var_may_hold_a_file_path() {
        let dir = tempfile::tempdir().unwrap();
        let env = EnvGuard::new();
        let token_path = dir.path().join("mounted-secret");
        std::fs::write(&token_path, "KGAT_fromfilepath\n").unwrap();
        env.set("KAGGLE_API_TOKEN", token_path.to_str().unwrap());
        assert_eq!(
            load_credentials_from(None).unwrap(),
            KaggleAuth::Token("KGAT_fromfilepath".to_string())
        );
    }

    #[test]
    fn access_token_file_is_read_and_trimmed() {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::new();
        write_config_file(dir.path(), "access_token", "  KGAT_fromfile\n");
        assert_eq!(
            load_credentials_from(Some(dir.path())).unwrap(),
            KaggleAuth::Token("KGAT_fromfile".to_string())
        );
    }

    #[test]
    fn access_token_txt_is_a_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::new();
        write_config_file(dir.path(), "access_token.txt", "KGAT_windows\n");
        assert_eq!(
            load_credentials_from(Some(dir.path())).unwrap(),
            KaggleAuth::Token("KGAT_windows".to_string())
        );
    }

    /// An access_token file left empty by a failed copy-paste shouldn't
    /// shadow working legacy credentials.
    #[test]
    fn empty_access_token_file_falls_through_to_kaggle_json() {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::new();
        write_config_file(dir.path(), "access_token", "\n  \n");
        write_config_file(
            dir.path(),
            "kaggle.json",
            r#"{"username":"json-user","key":"json-key"}"#,
        );
        assert_eq!(
            load_credentials_from(Some(dir.path())).unwrap(),
            KaggleAuth::ApiKey {
                username: "json-user".to_string(),
                key: "json-key".to_string(),
            }
        );
    }

    #[test]
    fn legacy_env_vars_still_work() {
        let dir = tempfile::tempdir().unwrap();
        let env = EnvGuard::new();
        env.set("KAGGLE_USERNAME", "env-user");
        env.set("KAGGLE_KEY", "env-key");
        assert_eq!(
            load_credentials_from(Some(dir.path())).unwrap(),
            KaggleAuth::ApiKey {
                username: "env-user".to_string(),
                key: "env-key".to_string(),
            }
        );
    }

    #[test]
    fn legacy_env_vars_lose_to_a_token_file() {
        let dir = tempfile::tempdir().unwrap();
        let env = EnvGuard::new();
        env.set("KAGGLE_USERNAME", "env-user");
        env.set("KAGGLE_KEY", "env-key");
        write_config_file(dir.path(), "access_token", "KGAT_fromfile");
        assert_eq!(
            load_credentials_from(Some(dir.path())).unwrap(),
            KaggleAuth::Token("KGAT_fromfile".to_string())
        );
    }

    #[test]
    fn kaggle_json_still_works() {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::new();
        write_config_file(
            dir.path(),
            "kaggle.json",
            r#"{"username":"json-user","key":"json-key"}"#,
        );
        assert_eq!(
            load_credentials_from(Some(dir.path())).unwrap(),
            KaggleAuth::ApiKey {
                username: "json-user".to_string(),
                key: "json-key".to_string(),
            }
        );
    }

    #[test]
    fn malformed_kaggle_json_is_a_hard_error() {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::new();
        write_config_file(dir.path(), "kaggle.json", "{not json");
        let msg = load_credentials_from(Some(dir.path()))
            .unwrap_err()
            .to_string();
        assert!(msg.contains("kaggle.json"), "got: {msg}");
    }

    #[test]
    fn kaggle_json_without_a_key_is_reported_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::new();
        write_config_file(dir.path(), "kaggle.json", r#"{"username":"u"}"#);
        let msg = load_credentials_from(Some(dir.path()))
            .unwrap_err()
            .to_string();
        assert!(msg.contains("empty username or key"), "got: {msg}");
    }

    #[test]
    fn missing_credentials_points_at_the_token_flow() {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::new();
        let msg = load_credentials_from(Some(dir.path()))
            .unwrap_err()
            .to_string();
        assert!(msg.contains("KAGGLE_API_TOKEN"), "got: {msg}");
        assert!(msg.contains("access_token"), "got: {msg}");
        assert!(msg.contains("kaggle.com/settings"), "got: {msg}");
        // The legacy path stays discoverable for anyone who already has one.
        assert!(msg.contains("kaggle.json"), "got: {msg}");
    }
}
