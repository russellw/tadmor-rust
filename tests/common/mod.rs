//! Shared setup for the integration tests, which run against
//! TEST_DATABASE_URL and drop and recreate its public schema.

use std::sync::Mutex;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::Value;
use sqlx::PgPool;
use tadmor::db;
use tadmor::http::{AppState, router};
use tower::ServiceExt;

/// Whether this test process has reset the schema yet. Each test runs on
/// its own runtime and thread; the first to get here resets, the rest wait.
static RESET: Mutex<bool> = Mutex::new(false);

pub async fn pool() -> PgPool {
    let url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must name a throwaway database; its public schema is dropped");
    let pool = db::connect(&url).await.expect("connect to TEST_DATABASE_URL");
    #[allow(clippy::await_holding_lock)]
    {
        let mut reset = RESET.lock().unwrap_or_else(|e| e.into_inner());
        if !*reset {
            sqlx::raw_sql("DROP SCHEMA public CASCADE; CREATE SCHEMA public")
                .execute(&pool)
                .await
                .expect("reset schema");
            db::migrate(&pool).await.expect("migrate");
            *reset = true;
        }
    }
    pool
}

pub async fn app() -> Router {
    router(AppState { pool: pool().await })
}

/// Sends a GET to the router in-process and returns the status and JSON body.
pub async fn get(app: Router, path: &str) -> (StatusCode, Value) {
    let response = app.oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}
