use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// On-disk TOML configuration structure.
/// All fields are optional so partial configs work (merge with defaults).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigFile {
    pub api_keys: Option<ApiKeysConfig>,
    pub databases: Option<DatabasesConfig>,
    pub concurrency: Option<ConcurrencyConfig>,
    pub display: Option<DisplayConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiKeysConfig {
    pub openalex_key: Option<String>,
    pub s2_api_key: Option<String>,
    pub crossref_mailto: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DatabasesConfig {
    pub dblp_offline_path: Option<String>,
    pub acl_offline_path: Option<String>,
    pub openalex_offline_path: Option<String>,
    pub cache_path: Option<String>,
    pub searxng_url: Option<String>,
    pub disabled: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConcurrencyConfig {
    pub num_workers: Option<usize>,
    pub db_timeout_secs: Option<u64>,
    pub db_timeout_short_secs: Option<u64>,
    pub max_rate_limit_retries: Option<u32>,
    pub max_archive_size_mb: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DisplayConfig {
    pub theme: Option<String>,
    pub fps: Option<u32>,
}

/// Platform config directory path: `<config_dir>/hallucinator/config.toml`.
pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("hallucinator").join("config.toml"))
}

/// Load config by cascading CWD `.hallucinator.toml` over platform config.
/// CWD values override platform values.
pub fn load_config() -> ConfigFile {
    let platform = config_path().and_then(|p| load_from_path(&p));
    let cwd = load_from_path(&PathBuf::from(".hallucinator.toml"));

    match (platform, cwd) {
        (None, None) => ConfigFile::default(),
        (Some(p), None) => p,
        (None, Some(c)) => c,
        (Some(p), Some(c)) => merge(p, c),
    }
}

/// Load a config from a specific path. Returns `None` if the file doesn't
/// exist or can't be parsed.
pub fn load_from_path(path: &PathBuf) -> Option<ConfigFile> {
    let content = std::fs::read_to_string(path).ok()?;
    toml::from_str(&content).ok()
}

/// Merge two configs: `overlay` values take precedence over `base`.
pub fn merge(base: ConfigFile, overlay: ConfigFile) -> ConfigFile {
    ConfigFile {
        api_keys: Some(ApiKeysConfig {
            openalex_key: overlay
                .api_keys
                .as_ref()
                .and_then(|a| a.openalex_key.clone())
                .or_else(|| base.api_keys.as_ref().and_then(|a| a.openalex_key.clone())),
            s2_api_key: overlay
                .api_keys
                .as_ref()
                .and_then(|a| a.s2_api_key.clone())
                .or_else(|| base.api_keys.as_ref().and_then(|a| a.s2_api_key.clone())),
            crossref_mailto: overlay
                .api_keys
                .as_ref()
                .and_then(|a| a.crossref_mailto.clone())
                .or_else(|| {
                    base.api_keys
                        .as_ref()
                        .and_then(|a| a.crossref_mailto.clone())
                }),
        }),
        databases: Some(DatabasesConfig {
            dblp_offline_path: overlay
                .databases
                .as_ref()
                .and_then(|d| d.dblp_offline_path.clone())
                .or_else(|| {
                    base.databases
                        .as_ref()
                        .and_then(|d| d.dblp_offline_path.clone())
                }),
            acl_offline_path: overlay
                .databases
                .as_ref()
                .and_then(|d| d.acl_offline_path.clone())
                .or_else(|| {
                    base.databases
                        .as_ref()
                        .and_then(|d| d.acl_offline_path.clone())
                }),
            openalex_offline_path: overlay
                .databases
                .as_ref()
                .and_then(|d| d.openalex_offline_path.clone())
                .or_else(|| {
                    base.databases
                        .as_ref()
                        .and_then(|d| d.openalex_offline_path.clone())
                }),
            cache_path: overlay
                .databases
                .as_ref()
                .and_then(|d| d.cache_path.clone())
                .or_else(|| base.databases.as_ref().and_then(|d| d.cache_path.clone())),
            searxng_url: overlay
                .databases
                .as_ref()
                .and_then(|d| d.searxng_url.clone())
                .or_else(|| base.databases.as_ref().and_then(|d| d.searxng_url.clone())),
            disabled: overlay
                .databases
                .as_ref()
                .and_then(|d| d.disabled.clone())
                .or_else(|| base.databases.as_ref().and_then(|d| d.disabled.clone())),
        }),
        concurrency: Some(ConcurrencyConfig {
            num_workers: overlay
                .concurrency
                .as_ref()
                .and_then(|c| c.num_workers)
                .or_else(|| base.concurrency.as_ref().and_then(|c| c.num_workers)),
            db_timeout_secs: overlay
                .concurrency
                .as_ref()
                .and_then(|c| c.db_timeout_secs)
                .or_else(|| base.concurrency.as_ref().and_then(|c| c.db_timeout_secs)),
            db_timeout_short_secs: overlay
                .concurrency
                .as_ref()
                .and_then(|c| c.db_timeout_short_secs)
                .or_else(|| {
                    base.concurrency
                        .as_ref()
                        .and_then(|c| c.db_timeout_short_secs)
                }),
            max_rate_limit_retries: overlay
                .concurrency
                .as_ref()
                .and_then(|c| c.max_rate_limit_retries)
                .or_else(|| {
                    base.concurrency
                        .as_ref()
                        .and_then(|c| c.max_rate_limit_retries)
                }),
            max_archive_size_mb: overlay
                .concurrency
                .as_ref()
                .and_then(|c| c.max_archive_size_mb)
                .or_else(|| {
                    base.concurrency
                        .as_ref()
                        .and_then(|c| c.max_archive_size_mb)
                }),
        }),
        display: Some(DisplayConfig {
            theme: overlay
                .display
                .as_ref()
                .and_then(|d| d.theme.clone())
                .or_else(|| base.display.as_ref().and_then(|d| d.theme.clone())),
            fps: overlay
                .display
                .as_ref()
                .and_then(|d| d.fps)
                .or_else(|| base.display.as_ref().and_then(|d| d.fps)),
        }),
    }
}

/// Save the current config to the platform config directory.
pub fn save_config(config: &ConfigFile) -> Result<PathBuf, String> {
    let path = config_path().ok_or_else(|| "Could not determine config directory".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create config directory: {}", e))?;
    }
    let content =
        toml::to_string_pretty(config).map_err(|e| format!("Failed to serialize config: {}", e))?;
    std::fs::write(&path, content).map_err(|e| format!("Failed to write config: {}", e))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_path_round_trip_toml() {
        let config = ConfigFile {
            databases: Some(DatabasesConfig {
                cache_path: Some("/tmp/test_cache.db".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: ConfigFile = toml::from_str(&toml_str).unwrap();
        assert_eq!(
            parsed.databases.unwrap().cache_path.unwrap(),
            "/tmp/test_cache.db"
        );
    }

    #[test]
    fn cache_path_absent_deserializes_as_none() {
        let toml_str = "[databases]\ndblp_offline_path = \"/some/path\"\n";
        let parsed: ConfigFile = toml::from_str(toml_str).unwrap();
        assert!(parsed.databases.unwrap().cache_path.is_none());
    }

    #[test]
    fn merge_cache_path_overlay_wins() {
        let base = ConfigFile {
            databases: Some(DatabasesConfig {
                cache_path: Some("/base/cache.db".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let overlay = ConfigFile {
            databases: Some(DatabasesConfig {
                cache_path: Some("/overlay/cache.db".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let merged = merge(base, overlay);
        assert_eq!(
            merged.databases.unwrap().cache_path.unwrap(),
            "/overlay/cache.db"
        );
    }

    #[test]
    fn merge_cache_path_base_preserved_when_overlay_absent() {
        let base = ConfigFile {
            databases: Some(DatabasesConfig {
                cache_path: Some("/base/cache.db".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let overlay = ConfigFile::default();
        let merged = merge(base, overlay);
        assert_eq!(
            merged.databases.unwrap().cache_path.unwrap(),
            "/base/cache.db"
        );
    }
}
