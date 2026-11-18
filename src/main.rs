use std::net::SocketAddr;
use axum::{routing::get, Router};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let app = Router::new().route("/healthz", get(|| async { "ok" }));
    let addr = SocketAddr::from(([0, 0, 0, 0], 8980));
    tracing::info!("listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
