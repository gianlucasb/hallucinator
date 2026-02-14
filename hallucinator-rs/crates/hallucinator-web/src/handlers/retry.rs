use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use std::sync::Arc;

use hallucinator_core::{Config, Status};

use crate::models::{RetryRequest, RetryResponse};
use crate::state::AppState;

pub async fn retry(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RetryRequest>,
) -> impl IntoResponse {
    if req.title.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Title is required" })),
        )
            .into_response();
    }

    if req.failed_dbs.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "No databases to retry" })),
        )
            .into_response();
    }

    let config = Config {
        openalex_key: req.openalex_key.clone(),
        s2_api_key: req.s2_api_key.clone(),
        dblp_offline_path: state.dblp_offline_path.clone(),
        dblp_offline_db: state.dblp_offline_db.clone(),
        check_openalex_authors: req.check_openalex_authors,
        ..Config::default()
    };

    let client = reqwest::Client::new();

    let result = hallucinator_core::query_all_databases(
        &req.title,
        &req.ref_authors,
        &config,
        &client,
        true, // longer timeout for retries
        Some(&req.failed_dbs),
        None,
    )
    .await;

    let status_str = match result.status {
        Status::Verified => "verified",
        Status::NotFound => "not_found",
        Status::AuthorMismatch => "author_mismatch",
    };

    let error_type = match result.status {
        Status::NotFound => Some("not_found".to_string()),
        Status::AuthorMismatch => Some("author_mismatch".to_string()),
        Status::Verified => None,
    };

    Json(RetryResponse {
        success: true,
        status: status_str.to_string(),
        source: result.source,
        found_authors: result.found_authors,
        paper_url: result.paper_url,
        error_type,
        failed_dbs: result.failed_dbs,
    })
    .into_response()
}
