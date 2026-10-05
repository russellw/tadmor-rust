use axum::{Router, routing::get};

#[tokio::main]
async fn main() {
    let addr = std::env::var("HTTP_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
    let app = Router::new().route("/healthz", get(|| async { "ok" }));
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    eprintln!("listening on {addr}");
    axum::serve(listener, app).await.expect("serve");
}
