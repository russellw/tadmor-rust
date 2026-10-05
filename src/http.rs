//! The HTTP surface: probes now, the JSON API and the UI to come.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .with_state(state)
}

/// spec/api.md §2: up while the process is.
async fn healthz() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

/// spec/api.md §2: ready when the database is reachable.
async fn readyz(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    match sqlx::query_scalar!(r#"SELECT 1 AS "one!""#).fetch_one(&state.pool).await {
        Ok(_) => (StatusCode::OK, Json(json!({"status": "ready"}))),
        Err(err) => {
            eprintln!("readyz: {err}");
            (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"status": "database unavailable"})))
        }
    }
}
