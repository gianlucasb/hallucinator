use hallucinator_dblp::DblpPool;
use std::path::PathBuf;
use std::sync::Arc;

/// Shared application state accessible from all handlers.
pub struct AppState {
    pub dblp_offline_path: Option<PathBuf>,
    pub dblp_offline_db: Option<Arc<DblpPool>>,
    pub dblp_offline_path_display: String,
}
