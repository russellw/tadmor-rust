//! The HTTP surface: the probes at the root and the JSON API under /api/
//! (spec/api.md). The UI is to come.

mod extract;
mod session;
mod users;

use axum::extract::State;
use axum::http::StatusCode;
use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use sqlx::PgPool;

use crate::error::Error;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
}

pub fn router(state: AppState) -> Router {
    let admin = Router::new()
        .route("/users", get(users::list).post(users::create))
        .route("/users/{id}", get(users::get).put(users::update))
        .route("/users/{id}/password", post(users::set_password))
        .route_layer(axum::middleware::from_fn(session::require_admin));

    // Authentication wraps the whole API, unknown paths included, so they
    // answer 401 without a session and 404 with one (spec/api.md §1.1).
    let api = Router::new()
        .route("/auth/login", post(session::login))
        .route("/auth/logout", post(session::logout))
        .route("/auth/me", get(session::me))
        .merge(admin)
        .fallback(no_such_endpoint)
        .method_not_allowed_fallback(no_such_endpoint)
        .layer(from_fn_with_state(state.clone(), session::require_session));

    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .nest("/api", api)
        .with_state(state)
}

async fn no_such_endpoint() -> Error {
    Error::new(StatusCode::NOT_FOUND, "no such endpoint")
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
