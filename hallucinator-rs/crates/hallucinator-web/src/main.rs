use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

mod handlers;
mod models;
mod state;
mod template;
mod upload;

use state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    // Load DBLP offline database if configured
    let dblp_offline_path = std::env::var("DBLP_OFFLINE_PATH").ok();
    let mut dblp_offline_db = None;
    let mut dblp_offline_path_display = String::new();

    if let Some(ref path_str) = dblp_offline_path {
        let path = std::path::PathBuf::from(path_str);
        if path.exists() {
            match hallucinator_dblp::DblpDatabase::open(&path) {
                Ok(db) => {
                    // Check staleness (30 days)
                    if let Ok(staleness) = db.check_staleness(30)
                        && staleness.is_stale
                    {
                        eprintln!(
                            "Warning: DBLP offline database is {} days old. Consider updating with: hallucinator-cli update-dblp <path>",
                            staleness.age_days.unwrap_or(0)
                        );
                    }
                    dblp_offline_path_display = path_str.clone();
                    dblp_offline_db = Some(Arc::new(Mutex::new(db)));
                    println!("DBLP offline database loaded: {}", path_str);
                }
                Err(e) => {
                    eprintln!(
                        "Warning: Failed to open DBLP database at {}: {}",
                        path_str, e
                    );
                }
            }
        } else {
            eprintln!("Warning: DBLP database file not found at {}", path_str);
        }
    }

    let state = Arc::new(AppState {
        dblp_offline_path: dblp_offline_path.map(std::path::PathBuf::from),
        dblp_offline_db,
        dblp_offline_path_display,
    });

    // Allow large file uploads (500MB)
    let body_limit = axum::extract::DefaultBodyLimit::max(500 * 1024 * 1024);

    let app = axum::Router::new()
        .route("/", axum::routing::get(handlers::index::index))
        .route(
            "/analyze/stream",
            axum::routing::post(handlers::stream::stream),
        )
        .route("/retry", axum::routing::post(handlers::retry::retry))
        .route("/static/logo.png", axum::routing::get(template::serve_logo))
        .layer(body_limit)
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 5001));
    println!("Listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
