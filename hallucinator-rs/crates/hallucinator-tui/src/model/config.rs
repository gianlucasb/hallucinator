/// Configuration sections for the config screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSection {
    ApiKeys,
    Databases,
    Concurrency,
    Display,
}

impl ConfigSection {
    pub fn all() -> &'static [ConfigSection] {
        &[
            ConfigSection::ApiKeys,
            ConfigSection::Databases,
            ConfigSection::Concurrency,
            ConfigSection::Display,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ApiKeys => "API Keys",
            Self::Databases => "Databases",
            Self::Concurrency => "Concurrency & Timeouts",
            Self::Display => "Display",
        }
    }
}

/// State for the config screen.
#[derive(Debug, Clone)]
pub struct ConfigState {
    pub section: ConfigSection,
    pub section_cursor: usize,
    pub item_cursor: usize,
    pub editing: bool,
    pub edit_buffer: String,
    pub prev_screen: Option<super::super::app::Screen>,

    // Editable fields
    pub openalex_key: String,
    pub s2_api_key: String,
    pub crossref_mailto: String,
    pub disabled_dbs: Vec<(String, bool)>, // (name, enabled)
    pub dblp_offline_path: String,
    pub acl_offline_path: String,
    pub max_concurrent_papers: usize,
    pub max_concurrent_refs: usize,
    pub db_timeout_secs: u64,
    pub db_timeout_short_secs: u64,
    pub max_archive_size_mb: u32, // 0 = unlimited
    pub theme_name: String,
    pub fps: u32,
}

impl Default for ConfigState {
    fn default() -> Self {
        let all_dbs = vec![
            ("CrossRef".to_string(), true),
            ("arXiv".to_string(), true),
            ("DBLP".to_string(), true),
            ("Semantic Scholar".to_string(), true),
            ("SSRN".to_string(), true),
            ("ACL Anthology".to_string(), true),
            ("Europe PMC".to_string(), true),
            ("PubMed".to_string(), true),
            ("OpenAlex".to_string(), true),
        ];

        Self {
            section: ConfigSection::ApiKeys,
            section_cursor: 0,
            item_cursor: 0,
            editing: false,
            edit_buffer: String::new(),
            prev_screen: None,
            openalex_key: String::new(),
            s2_api_key: String::new(),
            crossref_mailto: String::new(),
            disabled_dbs: all_dbs,
            dblp_offline_path: String::new(),
            acl_offline_path: String::new(),
            max_concurrent_papers: 1,
            max_concurrent_refs: 4,
            db_timeout_secs: 10,
            db_timeout_short_secs: 5,
            max_archive_size_mb: 0, // unlimited
            theme_name: "hacker".to_string(),
            fps: 30,
        }
    }
}

impl ConfigState {
    /// Mask a key for display: show first 4 chars then asterisks.
    pub fn mask_key(key: &str) -> String {
        if key.is_empty() {
            "(not set)".to_string()
        } else if key.len() <= 4 {
            "*".repeat(key.len())
        } else {
            format!("{}{}", &key[..4], "*".repeat(key.len() - 4))
        }
    }
}
