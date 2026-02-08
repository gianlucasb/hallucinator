use hallucinator_dblp::DblpDatabase;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Shared application state accessible from all handlers.
pub struct AppState {
    pub dblp_offline_path: Option<PathBuf>,
    pub dblp_offline_db: Option<Arc<Mutex<DblpDatabase>>>,
    pub dblp_offline_path_display: String,
}
