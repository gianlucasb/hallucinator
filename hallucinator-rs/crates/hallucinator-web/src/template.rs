use axum::http::header;
use axum::response::{Html, IntoResponse};

const INDEX_HTML: &str = include_str!("../../../../templates/index.html");
const LOGO_PNG: &[u8] = include_bytes!("../../../../static/logo.png");

/// Render the index page, injecting the DBLP offline path.
pub fn render_index(dblp_path: &str) -> Html<String> {
    let html = INDEX_HTML.replace("{{ dblp_offline_path }}", dblp_path);
    Html(html)
}

/// Serve the logo PNG with correct content type.
pub async fn serve_logo() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "image/png")], LOGO_PNG)
}
