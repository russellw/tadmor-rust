//! The HTTP surface: the probes at the root and the JSON API under /api/
//! (spec/api.md). The UI is to come.

mod extract;
mod master;
mod session;
mod users;

use axum::extract::State;
use axum::http::StatusCode;
use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::{get, post, put};
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
        .route_layer(from_fn(session::require_admin));

    // Authentication wraps the whole API, unknown paths included, so they
    // answer 401 without a session and 404 with one (spec/api.md §1.1).
    let api = Router::new()
        .route("/auth/login", post(session::login))
        .route("/auth/logout", post(session::logout))
        .route("/auth/me", get(session::me))
        .route("/organizations", get(master::list_organizations).post(master::create_organization))
        .route("/organizations/{id}", get(master::get_organization).put(master::update_organization))
        .route("/customers", get(master::list_customers).post(master::create_customer))
        .route("/customers/{id}", get(master::get_customer).put(master::update_customer))
        .route("/suppliers", get(master::list_suppliers).post(master::create_supplier))
        .route("/suppliers/{id}", get(master::get_supplier).put(master::update_supplier))
        .route("/products", get(master::list_products).post(master::create_product))
        .route("/products/{id}", get(master::get_product).put(master::update_product))
        .route("/accounts", get(master::list_accounts).post(master::create_account))
        .route("/accounts/{id}", get(master::get_account).put(master::update_account))
        .route("/accounts/{id}/ledger", get(master::account_ledger))
        .route("/tax-codes", get(master::list_tax_codes).post(master::create_tax_code))
        .route("/tax-codes/{code}", get(master::get_tax_code).put(master::update_tax_code))
        .route("/payment-terms", get(master::list_payment_terms).post(master::create_payment_term))
        .route("/payment-terms/{code}", get(master::get_payment_term).put(master::update_payment_term))
        .route("/warehouses", get(master::list_warehouses).post(master::create_warehouse))
        .route("/warehouses/{id}", get(master::get_warehouse).put(master::update_warehouse))
        .route("/fiscal-years", get(master::list_fiscal_years).post(master::create_fiscal_year))
        .route("/fiscal-years/{id}", get(master::get_fiscal_year).put(master::update_fiscal_year))
        .route("/accounting-periods", get(master::list_accounting_periods).post(master::create_accounting_period))
        .route("/accounting-periods/{id}", get(master::get_accounting_period).put(master::update_accounting_period))
        .route(
            "/settings",
            get(master::get_settings).merge(put(master::update_settings).route_layer(from_fn(session::require_admin))),
        )
        .route("/exchange-rates", get(master::list_exchange_rates).post(master::create_exchange_rate))
        .route(
            "/exchange-rates/{currency}/{date}",
            put(master::update_exchange_rate).delete(master::delete_exchange_rate),
        )
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
