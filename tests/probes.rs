mod common;

use std::time::Duration;

use axum::http::StatusCode;
use serde_json::json;
use tadmor::db;
use tadmor::http::{AppState, router};

#[tokio::test]
async fn healthz_is_ok() {
    let (status, body) = common::get(common::app().await, "/healthz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"status": "ok"}));
}

#[tokio::test]
async fn readyz_is_ready_with_a_database() {
    let (status, body) = common::get(common::app().await, "/readyz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"status": "ready"}));
}

#[tokio::test]
async fn readyz_is_unavailable_without_one() {
    // Nothing listens on port 1, so every connection attempt fails.
    let options = db::options("postgres://tadmor@127.0.0.1:1/none").unwrap();
    let pool = db::pool_options().acquire_timeout(Duration::from_millis(500)).connect_lazy_with(options);
    let (status, body) = common::get(router(AppState::new(pool)), "/readyz").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body, json!({"status": "database unavailable"}));
}

#[tokio::test]
async fn migrations_seed_the_reference_data() {
    // spec/api.md §4: the countries a fresh instance starts with.
    let pool = common::pool().await;
    let countries = sqlx::query_scalar!("SELECT code FROM countries ORDER BY code").fetch_all(&pool).await.unwrap();
    assert_eq!(countries, ["AU", "CA", "DE", "FR", "GB", "IE", "JP", "US"]);
}
