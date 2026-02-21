pub use hallucinator_core::config_file::*;

use crate::model::config::ConfigState;

/// Convert a `ConfigFile` into partial fills on a `ConfigState`.
/// Only sets values that are `Some` in the file config (doesn't overwrite with defaults).
pub fn apply_to_config_state(file_cfg: &ConfigFile, state: &mut ConfigState) {
    if let Some(api) = &file_cfg.api_keys {
        if let Some(ref key) = api.openalex_key
            && !key.is_empty()
        {
            state.openalex_key = key.clone();
        }
        if let Some(ref key) = api.s2_api_key
            && !key.is_empty()
        {
            state.s2_api_key = key.clone();
        }
        if let Some(ref email) = api.crossref_mailto
            && !email.is_empty()
        {
            state.crossref_mailto = email.clone();
        }
    }
    if let Some(db) = &file_cfg.databases {
        if let Some(ref path) = db.dblp_offline_path
            && !path.is_empty()
        {
            state.dblp_offline_path = path.clone();
        }
        if let Some(ref path) = db.acl_offline_path
            && !path.is_empty()
        {
            state.acl_offline_path = path.clone();
        }
        if let Some(ref path) = db.openalex_offline_path
            && !path.is_empty()
        {
            state.openalex_offline_path = path.clone();
        }
        if let Some(ref path) = db.cache_path
            && !path.is_empty()
        {
            state.cache_path = path.clone();
        }
        if let Some(ref url) = db.searxng_url
            && !url.is_empty()
        {
            state.searxng_url = Some(url.clone());
        }
        if let Some(ref disabled) = db.disabled {
            for (name, enabled) in &mut state.disabled_dbs {
                if disabled.iter().any(|d| d.eq_ignore_ascii_case(name)) {
                    *enabled = false;
                }
            }
        }
    }
    if let Some(conc) = &file_cfg.concurrency {
        if let Some(v) = conc.num_workers {
            state.num_workers = v.max(1);
        }
        if let Some(v) = conc.db_timeout_secs {
            state.db_timeout_secs = v.max(1);
        }
        if let Some(v) = conc.db_timeout_short_secs {
            state.db_timeout_short_secs = v.max(1);
        }
        if let Some(v) = conc.max_rate_limit_retries {
            state.max_rate_limit_retries = v;
        }
        if let Some(v) = conc.max_archive_size_mb {
            state.max_archive_size_mb = v;
        }
    }
    if let Some(disp) = &file_cfg.display {
        if let Some(ref theme) = disp.theme
            && !theme.is_empty()
        {
            state.theme_name = theme.clone();
        }
        if let Some(fps) = disp.fps {
            state.fps = fps.clamp(1, 120);
        }
    }
}

/// Convert a `ConfigState` into a `ConfigFile` for saving.
pub fn from_config_state(state: &ConfigState) -> ConfigFile {
    let disabled: Vec<String> = state
        .disabled_dbs
        .iter()
        .filter(|(_, enabled)| !enabled)
        .map(|(name, _)| name.clone())
        .collect();

    ConfigFile {
        api_keys: Some(ApiKeysConfig {
            openalex_key: if state.openalex_key.is_empty() {
                None
            } else {
                Some(state.openalex_key.clone())
            },
            s2_api_key: if state.s2_api_key.is_empty() {
                None
            } else {
                Some(state.s2_api_key.clone())
            },
            crossref_mailto: if state.crossref_mailto.is_empty() {
                None
            } else {
                Some(state.crossref_mailto.clone())
            },
        }),
        databases: Some(DatabasesConfig {
            dblp_offline_path: if state.dblp_offline_path.is_empty() {
                None
            } else {
                Some(state.dblp_offline_path.clone())
            },
            acl_offline_path: if state.acl_offline_path.is_empty() {
                None
            } else {
                Some(state.acl_offline_path.clone())
            },
            openalex_offline_path: if state.openalex_offline_path.is_empty() {
                None
            } else {
                Some(state.openalex_offline_path.clone())
            },
            cache_path: if state.cache_path.is_empty() {
                None
            } else {
                Some(state.cache_path.clone())
            },
            searxng_url: state.searxng_url.clone(),
            disabled: if disabled.is_empty() {
                None
            } else {
                Some(disabled)
            },
        }),
        concurrency: Some(ConcurrencyConfig {
            num_workers: Some(state.num_workers),
            db_timeout_secs: Some(state.db_timeout_secs),
            db_timeout_short_secs: Some(state.db_timeout_short_secs),
            max_rate_limit_retries: Some(state.max_rate_limit_retries),
            max_archive_size_mb: Some(state.max_archive_size_mb),
        }),
        display: Some(DisplayConfig {
            theme: Some(state.theme_name.clone()),
            fps: Some(state.fps),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_cache_path_to_config_state() {
        let file_cfg = ConfigFile {
            databases: Some(DatabasesConfig {
                cache_path: Some("/tmp/cache.db".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut state = ConfigState::default();
        assert!(state.cache_path.is_empty());

        apply_to_config_state(&file_cfg, &mut state);
        assert_eq!(state.cache_path, "/tmp/cache.db");
    }

    #[test]
    fn apply_empty_cache_path_does_not_overwrite() {
        let file_cfg = ConfigFile {
            databases: Some(DatabasesConfig {
                cache_path: Some(String::new()), // empty string
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut state = ConfigState {
            cache_path: "existing.db".to_string(),
            ..Default::default()
        };

        apply_to_config_state(&file_cfg, &mut state);
        assert_eq!(state.cache_path, "existing.db"); // not overwritten
    }

    #[test]
    fn apply_none_cache_path_does_not_overwrite() {
        let file_cfg = ConfigFile::default(); // no databases section
        let mut state = ConfigState {
            cache_path: "existing.db".to_string(),
            ..Default::default()
        };

        apply_to_config_state(&file_cfg, &mut state);
        assert_eq!(state.cache_path, "existing.db"); // preserved
    }

    #[test]
    fn from_config_state_cache_path() {
        let state = ConfigState {
            cache_path: "/tmp/cache.db".to_string(),
            ..Default::default()
        };
        let file_cfg = from_config_state(&state);
        assert_eq!(
            file_cfg.databases.unwrap().cache_path.unwrap(),
            "/tmp/cache.db"
        );
    }

    #[test]
    fn from_config_state_empty_cache_path_is_none() {
        let state = ConfigState::default();
        let file_cfg = from_config_state(&state);
        assert!(file_cfg.databases.unwrap().cache_path.is_none());
    }

    #[test]
    fn full_round_trip_config_state_toml_config_state() {
        // ConfigState -> ConfigFile -> TOML -> ConfigFile -> ConfigState
        let state = ConfigState {
            cache_path: "/data/hallucinator_cache.db".to_string(),
            openalex_key: "test-key".to_string(),
            ..Default::default()
        };

        let file_cfg = from_config_state(&state);
        let toml_str = toml::to_string_pretty(&file_cfg).unwrap();
        let parsed: ConfigFile = toml::from_str(&toml_str).unwrap();

        let mut state2 = ConfigState::default();
        apply_to_config_state(&parsed, &mut state2);

        assert_eq!(state2.cache_path, "/data/hallucinator_cache.db");
        assert_eq!(state2.openalex_key, "test-key");
    }
}
