//! Server bootstrap: router assembly, bind-address selection and the
//! static-asset middleware.

use axum::http::{header, HeaderValue};
use axum::middleware;
use axum::routing::{get, post};
use axum::Router;
use std::io;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::services::{ServeDir, ServeFile};

use super::discovery;
use super::handlers;
use super::state::AppState;

const DEFAULT_ADDRESS: &str = "0.0.0.0:8080";
/// Environment override used by tests and by developers who need a second
/// instance on a different port (e.g. `CNC_WEB_ADDRESS=127.0.0.1:8021`).
const ADDRESS_ENV: &str = "CNC_WEB_ADDRESS";
const WEB_ROOT: &str = "web";

/// Forces browsers to revalidate UI assets on every load.
///
/// `ServeDir` sets no `Cache-Control`, so browsers apply *heuristic*
/// freshness and may serve a stale module after an application update; a
/// stale `js/i18n.js` next to a fresh `js/ui-settings.js` once crashed the
/// whole UI with a missing-export `SyntaxError`. `no-cache` means "always
/// revalidate": the browser sends a conditional request and the server
/// answers with a cheap `304 Not Modified` when nothing changed, so assets
/// are never stale and never refetched in full unnecessarily.
async fn revalidate_static_assets(
    request: axum::extract::Request,
    next: middleware::Next,
) -> axum::response::Response {
    let mut response = next.run(request).await;
    if !response.headers().contains_key(header::CACHE_CONTROL) {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    }
    response
}

/// Binds the axum router and serves the UI + JSON API forever.
pub async fn run() -> io::Result<()> {
    let controller = Arc::new(AppState::new()?);
    let bind = std::env::var(ADDRESS_ENV).unwrap_or_else(|_| DEFAULT_ADDRESS.to_string());
    let listener = TcpListener::bind(&bind).await?;
    let address = listener.local_addr()?;

    let static_files =
        ServeDir::new(WEB_ROOT).fallback(ServeFile::new(format!("{WEB_ROOT}/index.html")));
    let app = Router::new()
        .route("/api/status", get(handlers::get_status))
        .route(
            "/api/discovered-machines",
            get(discovery::get_discovered_machines).post(discovery::rediscover_machines),
        )
        .route("/api/machines/select", post(discovery::select_machine))
        .route(
            "/api/log",
            get(handlers::get_log).post(handlers::post_log_message),
        )
        .route("/api/command", post(handlers::post_machine_command))
        .route(
            "/api/command/cancel",
            post(handlers::cancel_machine_command),
        )
        .route("/api/command-history", get(handlers::get_command_history))
        .route(
            "/api/program",
            get(handlers::get_program).post(handlers::post_program),
        )
        .route("/api/toolpath", get(handlers::get_toolpath))
        .route("/api/program/run", post(handlers::start_program))
        .route("/api/program/pause", post(handlers::pause_program))
        .route("/api/program/resume", post(handlers::resume_program))
        .route("/api/program/abort", post(handlers::abort_program))
        .route("/api/jog", post(handlers::jog))
        .route("/api/spindle", post(handlers::set_spindle))
        .route("/api/reset-alarm", post(handlers::post_reset_alarm))
        .route("/api/jog/xy", post(handlers::jog_xy))
        .route("/api/jog/continuous", post(handlers::jog_continuous))
        .route("/api/jog/continuous-xy", post(handlers::jog_continuous_xy))
        .route("/api/jog/cancel", post(handlers::jog_cancel))
        .route("/api/jog/speed", post(handlers::jog_speed))
        .route(
            "/api/position/select",
            post(handlers::select_coordinate_system),
        )
        .route("/api/position/zero", post(handlers::zero_all))
        .route("/api/position/zero-axis", post(handlers::zero_axis))
        .route("/api/position/home", post(handlers::home_axis))
        .route("/api/position/home-all", post(handlers::home_all))
        .route("/api/position/set", post(handlers::set_axis_work_value))
        .route("/api/position/go", post(handlers::go_to_axis_work_value))
        .route(
            "/api/macros",
            get(handlers::get_macros).put(handlers::put_macros),
        )
        .route("/api/files/sources", get(handlers::get_files_sources))
        .route("/api/files/list", get(handlers::get_files_list))
        .route("/api/files/content", get(handlers::get_files_content))
        .route("/api/files/download", get(handlers::get_files_download))
        .route("/api/files/mkdir", post(handlers::post_files_mkdir))
        .route("/api/files/delete", post(handlers::post_files_delete))
        .route("/api/files/rename", post(handlers::post_files_rename))
        .route("/api/files/move", post(handlers::post_files_move))
        .route("/api/files/copy", post(handlers::post_files_copy))
        .route("/api/files/upload", post(handlers::post_files_upload))
        .fallback_service(static_files)
        .layer(middleware::from_fn(revalidate_static_assets))
        .with_state(controller);

    eprintln!("Web server listening on http://{address}");
    eprintln!("Warning: web controls have no authentication; use only on a trusted network.");
    axum::serve(listener, app).await
}
