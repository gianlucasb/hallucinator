use std::path::PathBuf;
use std::time::Instant;

use super::{App, InputMode, Screen};
use crate::model::config::ConfigSection;
use crate::tui_event::BackendCommand;

impl App {
    /// Handle mouse click → row selection.
    pub(super) fn handle_click(&mut self, _x: u16, y: u16) {
        if let Some(table_area) = self.last_table_area
            && y >= table_area.y
            && y < table_area.y + table_area.height
        {
            // Account for border (1) + header row (1) = offset 2 from table_area.y
            let row_offset = 2u16;
            if y >= table_area.y + row_offset {
                let clicked_row = (y - table_area.y - row_offset) as usize;
                match &self.screen {
                    Screen::Queue => {
                        if clicked_row < self.queue_sorted.len() {
                            self.queue_cursor = clicked_row;
                        }
                    }
                    Screen::Paper(idx) => {
                        let indices = self.paper_ref_indices(*idx);
                        if clicked_row < indices.len() {
                            self.paper_cursor = clicked_row;
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// Get the number of items in the current config section.
    pub(super) fn config_section_item_count(&self) -> usize {
        match self.config_state.section {
            ConfigSection::ApiKeys => 3,
            ConfigSection::Databases => 7 + self.config_state.disabled_dbs.len(), // DBLP + ACL + OpenAlex + cache_path + clear_cache + clear_not_found + searxng_url + toggles
            ConfigSection::Concurrency => 5,
            ConfigSection::Display => 2, // theme + fps
        }
    }

    /// Handle Enter on Config screen (start editing a field).
    pub(super) fn handle_config_enter(&mut self) {
        match self.config_state.section {
            ConfigSection::ApiKeys => {
                let value = match self.config_state.item_cursor {
                    0 => self.config_state.openalex_key.clone(),
                    1 => self.config_state.s2_api_key.clone(),
                    2 => self.config_state.crossref_mailto.clone(),
                    _ => return,
                };
                self.config_state.editing = true;
                self.config_state.edit_buffer = value;
                self.input_mode = InputMode::TextInput;
            }
            ConfigSection::Concurrency => {
                let value = match self.config_state.item_cursor {
                    0 => self.config_state.num_workers.to_string(),
                    1 => self.config_state.max_rate_limit_retries.to_string(),
                    2 => self.config_state.db_timeout_secs.to_string(),
                    3 => self.config_state.db_timeout_short_secs.to_string(),
                    4 => self.config_state.max_archive_size_mb.to_string(),
                    _ => return,
                };
                self.config_state.editing = true;
                self.config_state.edit_buffer = value;
                self.input_mode = InputMode::TextInput;
            }
            ConfigSection::Display => match self.config_state.item_cursor {
                0 => {
                    // Cycle theme: hacker → modern → t800 → hacker
                    self.cycle_theme();
                }
                1 => {
                    // Edit FPS
                    self.config_state.editing = true;
                    self.config_state.edit_buffer = self.config_state.fps.to_string();
                    self.input_mode = InputMode::TextInput;
                }
                _ => {}
            },
            ConfigSection::Databases => {
                if self.config_state.item_cursor == 0 {
                    // Item 0: edit DBLP offline path
                    self.config_state.editing = true;
                    self.config_state.edit_buffer = self.config_state.dblp_offline_path.clone();
                    self.input_mode = InputMode::TextInput;
                } else if self.config_state.item_cursor == 1 {
                    // Item 1: edit ACL offline path
                    self.config_state.editing = true;
                    self.config_state.edit_buffer = self.config_state.acl_offline_path.clone();
                    self.input_mode = InputMode::TextInput;
                } else if self.config_state.item_cursor == 2 {
                    // Item 2: edit OpenAlex offline path
                    self.config_state.editing = true;
                    self.config_state.edit_buffer = self.config_state.openalex_offline_path.clone();
                    self.input_mode = InputMode::TextInput;
                } else if self.config_state.item_cursor == 3 {
                    // Item 3: edit cache path
                    self.config_state.editing = true;
                    self.config_state.edit_buffer = self.config_state.cache_path.clone();
                    self.input_mode = InputMode::TextInput;
                } else if self.config_state.item_cursor == 4 {
                    // Item 4: clear cache button
                    self.clear_query_cache();
                } else if self.config_state.item_cursor == 5 {
                    // Item 5: clear not-found button
                    self.clear_not_found_cache();
                } else if self.config_state.item_cursor == 6 {
                    // Item 6: edit SearxNG URL
                    self.config_state.editing = true;
                    self.config_state.edit_buffer =
                        self.config_state.searxng_url.clone().unwrap_or_default();
                    self.input_mode = InputMode::TextInput;
                } else {
                    // Items 7+: toggle DB (same as space)
                    self.handle_config_space();
                }
            }
        }
    }

    /// Handle Space on Config screen (toggle database or cycle theme).
    pub(super) fn handle_config_space(&mut self) {
        match self.config_state.section {
            ConfigSection::Databases => {
                // Items 7+ are DB toggles (0-2: offline paths, 3: cache path, 4: clear cache, 5: clear not-found, 6: searxng url)
                if self.config_state.item_cursor >= 7 {
                    let toggle_idx = self.config_state.item_cursor - 7;
                    if let Some((_, enabled)) = self.config_state.disabled_dbs.get_mut(toggle_idx) {
                        *enabled = !*enabled;
                        self.config_state.dirty = true;
                    }
                }
            }
            ConfigSection::Display => {
                if self.config_state.item_cursor == 0 {
                    self.cycle_theme();
                }
            }
            _ => {}
        }
    }

    /// Confirm a config text edit.
    pub(super) fn confirm_config_edit(&mut self) {
        let buf = self.config_state.edit_buffer.clone();
        match self.config_state.section {
            ConfigSection::ApiKeys => match self.config_state.item_cursor {
                0 => self.config_state.openalex_key = buf,
                1 => self.config_state.s2_api_key = buf,
                2 => self.config_state.crossref_mailto = buf,
                _ => {}
            },
            ConfigSection::Concurrency => match self.config_state.item_cursor {
                0 => {
                    if let Ok(v) = buf.parse::<usize>() {
                        self.config_state.num_workers = v.max(1);
                    }
                }
                1 => {
                    if let Ok(v) = buf.parse::<u32>() {
                        self.config_state.max_rate_limit_retries = v;
                    }
                }
                2 => {
                    if let Ok(v) = buf.parse::<u64>() {
                        self.config_state.db_timeout_secs = v.max(1);
                    }
                }
                3 => {
                    if let Ok(v) = buf.parse::<u64>() {
                        self.config_state.db_timeout_short_secs = v.max(1);
                    }
                }
                4 => {
                    if let Ok(v) = buf.parse::<u32>() {
                        self.config_state.max_archive_size_mb = v;
                    }
                }
                _ => {}
            },
            ConfigSection::Databases => match self.config_state.item_cursor {
                0 => {
                    self.config_state.dblp_offline_path = if buf.is_empty() {
                        buf
                    } else {
                        super::clean_canonicalize(&PathBuf::from(&buf))
                    };
                }
                1 => {
                    self.config_state.acl_offline_path = if buf.is_empty() {
                        buf
                    } else {
                        super::clean_canonicalize(&PathBuf::from(&buf))
                    };
                }
                2 => {
                    self.config_state.openalex_offline_path = if buf.is_empty() {
                        buf
                    } else {
                        super::clean_canonicalize(&PathBuf::from(&buf))
                    };
                }
                3 => {
                    self.config_state.cache_path = if buf.is_empty() {
                        buf
                    } else {
                        super::clean_canonicalize(&PathBuf::from(&buf))
                    };
                }
                6 => {
                    self.config_state.searxng_url = if buf.is_empty() { None } else { Some(buf) };
                }
                _ => {}
            },
            ConfigSection::Display => {
                if self.config_state.item_cursor == 1
                    && let Ok(v) = buf.parse::<u32>()
                {
                    self.config_state.fps = v.clamp(1, 120);
                }
            }
        }
        self.config_state.dirty = true;
        self.config_state.editing = false;
        self.config_state.edit_buffer.clear();
        self.input_mode = InputMode::Normal;
    }

    /// Clear the query cache (both in-memory and on-disk).
    pub(super) fn clear_query_cache(&mut self) {
        // Prefer the live cache handle — clears both L1 DashMap and L2 SQLite,
        // and VACUUM runs on the same connection so there's no locking conflict.
        if let Some(ref cache) = self.current_query_cache {
            cache.clear();
            self.config_state.cache_clear_status = Some("Cache cleared".to_string());
            self.activity.log("Query cache cleared".to_string());
            return;
        }

        // No live cache — open a temporary handle to clear the file on disk.
        let cache_path = if self.config_state.cache_path.is_empty() {
            None
        } else {
            Some(std::path::PathBuf::from(&self.config_state.cache_path))
        };

        if let Some(ref path) = cache_path {
            if path.exists() {
                match hallucinator_core::QueryCache::open(
                    path,
                    std::time::Duration::from_secs(1),
                    std::time::Duration::from_secs(1),
                ) {
                    Ok(cache) => {
                        cache.clear();
                        self.config_state.cache_clear_status = Some("Cache cleared".to_string());
                        self.activity.log("Query cache cleared".to_string());
                    }
                    Err(e) => {
                        self.config_state.cache_clear_status = Some(format!("Failed: {}", e));
                        self.activity
                            .log_warn(format!("Failed to clear cache: {}", e));
                    }
                }
            } else {
                self.config_state.cache_clear_status = Some("No cache file found".to_string());
            }
        } else {
            self.config_state.cache_clear_status = Some("No cache path configured".to_string());
        }
    }

    /// Clear only not-found entries from the query cache.
    pub(super) fn clear_not_found_cache(&mut self) {
        if let Some(ref cache) = self.current_query_cache {
            let removed = cache.clear_not_found();
            self.config_state.cache_clear_status = Some(format!(
                "Cleared {} not-found entries (ref x db pairs)",
                removed
            ));
            self.activity.log(format!(
                "Cleared {} not-found cache entries (each ref checked against multiple DBs)",
                removed
            ));
            return;
        }

        let cache_path = if self.config_state.cache_path.is_empty() {
            None
        } else {
            Some(std::path::PathBuf::from(&self.config_state.cache_path))
        };

        if let Some(ref path) = cache_path {
            if path.exists() {
                match hallucinator_core::QueryCache::open(
                    path,
                    std::time::Duration::from_secs(1),
                    std::time::Duration::from_secs(1),
                ) {
                    Ok(cache) => {
                        let removed = cache.clear_not_found();
                        self.config_state.cache_clear_status =
                            Some(format!("Cleared {} not-found entries", removed));
                        self.activity
                            .log(format!("Cleared {} not-found cache entries", removed));
                    }
                    Err(e) => {
                        self.config_state.cache_clear_status = Some(format!("Failed: {}", e));
                        self.activity
                            .log_warn(format!("Failed to clear not-found cache: {}", e));
                    }
                }
            } else {
                self.config_state.cache_clear_status = Some("No cache file found".to_string());
            }
        } else {
            self.config_state.cache_clear_status = Some("No cache path configured".to_string());
        }
    }

    /// Save config to disk and clear the dirty flag.
    pub(super) fn save_config(&mut self) {
        let file_cfg = crate::config_file::from_config_state(&self.config_state);
        match crate::config_file::save_config(&file_cfg) {
            Ok(path) => {
                self.config_state.dirty = false;
                self.activity
                    .log(format!("Config saved to {}", path.display()));
            }
            Err(e) => {
                self.activity.log(format!("Config save failed: {}", e));
            }
        }
    }

    pub(super) fn handle_build_database(&mut self) {
        if self.screen != Screen::Config || self.config_state.section != ConfigSection::Databases {
            return;
        }

        let item = self.config_state.item_cursor;
        if item == 0 {
            // DBLP
            if self.config_state.dblp_building {
                return; // already building
            }
            let db_path = if self.config_state.dblp_offline_path.is_empty() {
                super::default_db_path("dblp.db")
            } else {
                PathBuf::from(&self.config_state.dblp_offline_path)
            };
            self.config_state.dblp_building = true;
            self.config_state.dblp_build_status = Some("Starting...".to_string());
            self.config_state.dblp_build_started = Some(Instant::now());
            self.config_state.dblp_parse_started = None;
            self.activity.log(format!(
                "Building DBLP database at {}...",
                db_path.display()
            ));
            if let Some(tx) = &self.backend_cmd_tx {
                let _ = tx.send(BackendCommand::BuildDblp { db_path });
            }
        } else if item == 1 {
            // ACL
            if self.config_state.acl_building {
                return;
            }
            let db_path = if self.config_state.acl_offline_path.is_empty() {
                super::default_db_path("acl.db")
            } else {
                PathBuf::from(&self.config_state.acl_offline_path)
            };
            self.config_state.acl_building = true;
            self.config_state.acl_build_status = Some("Starting...".to_string());
            self.config_state.acl_build_started = Some(Instant::now());
            self.config_state.acl_parse_started = None;
            self.activity
                .log(format!("Building ACL database at {}...", db_path.display()));
            if let Some(tx) = &self.backend_cmd_tx {
                let _ = tx.send(BackendCommand::BuildAcl { db_path });
            }
        } else if item == 2 {
            // OpenAlex
            if self.config_state.openalex_building {
                return;
            }
            let db_path = if self.config_state.openalex_offline_path.is_empty() {
                super::default_db_path("openalex.idx")
            } else {
                PathBuf::from(&self.config_state.openalex_offline_path)
            };
            self.config_state.openalex_building = true;
            self.config_state.openalex_build_status = Some("Starting...".to_string());
            self.config_state.openalex_build_started = Some(Instant::now());
            self.activity.log(format!(
                "Building OpenAlex index at {}...",
                db_path.display()
            ));
            if let Some(tx) = &self.backend_cmd_tx {
                let _ = tx.send(BackendCommand::BuildOpenalex { db_path });
            }
        }
    }
}
