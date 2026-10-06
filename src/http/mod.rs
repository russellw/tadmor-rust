//! The HTTP surface: the probes at the root and the JSON API under /api/
//! (spec/api.md). The UI is to come.

mod banking;
mod documents;
mod extract;
mod inventory;
mod master;
mod orders;
mod payments;
mod printing;
mod reports;
pub(crate) mod session;
mod users;

use axum::extract::State;
use axum::http::StatusCode;
use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde_json::{Value, json};
use sqlx::PgPool;

use crate::error::Error;

pub use extract::is_date;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    /// None when email is off.
    pub mailer: Option<std::sync::Arc<crate::mailer::Mailer>>,
}

impl AppState {
    /// A state with email off.
    pub fn new(pool: PgPool) -> AppState {
        AppState { pool, mailer: None }
    }
}

pub fn router(state: AppState) -> Router {
    let admin = Router::new()
        .route("/users", get(users::list).post(users::create))
        .route("/users/{id}", get(users::get).put(users::update))
        .route("/users/{id}/password", post(users::set_password))
        .route("/fiscal-years/{id}/close", post(reports::close_year))
        .route("/fiscal-years/{id}/reopen", post(reports::reopen_year))
        .route("/stock-movements/{id}/unpost", post(inventory::unpost))
        .route("/bank-statements/{id}/reopen", post(banking::reopen))
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
        .route("/stock-movements", get(inventory::list).post(inventory::create))
        .route("/stock-movements/{id}", get(inventory::get).put(inventory::update).delete(inventory::delete))
        .route("/stock-movements/{id}/post", post(inventory::post))
        .route("/bank-statements", get(banking::list).post(banking::create))
        .route("/bank-statements/{id}", get(banking::get).put(banking::update).delete(banking::delete))
        .route("/bank-statements/{id}/lines", get(banking::lines).post(banking::add_line))
        .route("/bank-statements/{id}/import", post(banking::import))
        .route("/bank-statements/{id}/candidates", get(banking::candidates))
        .route("/bank-statements/{id}/auto-match", post(banking::auto_match))
        .route("/bank-statements/{id}/reconcile", post(banking::reconcile))
        .route("/bank-statement-lines/{id}", axum::routing::delete(banking::delete_line))
        .route("/bank-statement-lines/{id}/match", post(banking::match_line))
        .route("/bank-statement-lines/{id}/unmatch", post(banking::unmatch_line))
        .route("/journal-entries/{id}", get(documents::journal_entry))
        .route("/trial-balance", get(documents::trial_balance))
        .route("/profit-and-loss", get(reports::profit_and_loss))
        .route("/balance-sheet", get(reports::balance_sheet))
        .route("/cash-flow", get(reports::cash_flow))
        .route("/ar-aging", get(reports::ar_aging))
        .route("/ap-aging", get(reports::ap_aging))
        .route("/inventory-valuation", get(reports::inventory_valuation))
        .merge(admin)
        .merge(crate::documents::KINDS.iter().fold(Router::new(), |r, kind| r.merge(documents::routes(kind))))
        .merge(orders::routes())
        .merge(crate::printing::PRINTABLES.iter().fold(Router::new(), |r, p| r.merge(printing::routes(p))))
        .merge(crate::payments::PAYMENT_KINDS.iter().fold(Router::new(), |r, kind| r.merge(payments::routes(kind))))
        .merge(
            ["customer-payments", "supplier-payments", "sales-credit-notes", "purchase-credit-notes"]
                .into_iter()
                .filter_map(crate::settlement::settler)
                .fold(Router::new(), |r, s| r.merge(payments::settlement_routes(s))),
        )
        .fallback(no_such_endpoint)
        .method_not_allowed_fallback(no_such_endpoint)
        .layer(from_fn_with_state(state.clone(), session::require_session));

    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .nest("/api", api)
        .merge(crate::ui::router(state.clone()))
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
