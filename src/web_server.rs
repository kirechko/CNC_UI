use axum::Router;
use std::io;
use tokio::net::TcpListener;
use tower_http::services::{ServeDir, ServeFile};

const DEFAULT_ADDRESS: &str = "0.0.0.0:8080";
const WEB_ROOT: &str = "web";

pub async fn run() -> io::Result<()> {
    let listener = TcpListener::bind(DEFAULT_ADDRESS).await?;
    let address = listener.local_addr()?;

    let static_files =
        ServeDir::new(WEB_ROOT).fallback(ServeFile::new(format!("{WEB_ROOT}/index.html")));
    let app = Router::new().fallback_service(static_files);

    eprintln!("Web server listening on http://{address}");
    axum::serve(listener, app).await
}
