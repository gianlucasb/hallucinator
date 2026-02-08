use axum::extract::State;
use axum::response::Html;
use std::sync::Arc;

use crate::state::AppState;
use crate::template;

pub async fn index(State(state): State<Arc<AppState>>) -> Html<String> {
    template::render_index(&state.dblp_offline_path_display)
}
