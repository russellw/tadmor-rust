use axum::Router;

use crate::http::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
}
