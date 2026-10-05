//! Master data, calendar, settings and exchange-rate endpoints (spec/api.md
//! §5.2 to §5.8): thin wrappers over the service modules.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::AppState;
use super::extract::{Body, DateRange, Id};
use crate::currency::{self, ExchangeRate, ExchangeRateInput, ExchangeRateKey, Settings};
use crate::error::Result;
use crate::reporting::{self, LedgerRow};
use crate::{calendar, master};

/// Handlers for a resource with a synthetic id: list, get, create (201
/// `{"id"}`), and update (204), each delegating to a service function.
macro_rules! id_resource {
    ($list:ident, $get:ident, $create:ident, $update:ident, $record:ty, $input:ty, $svc:ident) => {
        pub async fn $list(State(state): State<AppState>) -> Result<Json<Vec<$record>>> {
            Ok(Json($svc::$list(&state.pool).await?))
        }

        pub async fn $get(State(state): State<AppState>, Id(id): Id) -> Result<Json<$record>> {
            Ok(Json($svc::$get(&state.pool, id).await?))
        }

        pub async fn $create(State(state): State<AppState>, Body(input): Body<$input>) -> Result<(StatusCode, Json<Value>)> {
            let id = $svc::$create(&state.pool, input).await?;
            Ok((StatusCode::CREATED, Json(json!({"id": id}))))
        }

        pub async fn $update(State(state): State<AppState>, Id(id): Id, Body(input): Body<$input>) -> Result<StatusCode> {
            $svc::$update(&state.pool, id, input).await?;
            Ok(StatusCode::NO_CONTENT)
        }
    };
}

id_resource!(list_organizations, get_organization, create_organization, update_organization, master::Organization, master::OrganizationInput, master);
id_resource!(list_customers, get_customer, create_customer, update_customer, master::Customer, master::CustomerInput, master);
id_resource!(list_suppliers, get_supplier, create_supplier, update_supplier, master::Supplier, master::SupplierInput, master);
id_resource!(list_products, get_product, create_product, update_product, master::Product, master::ProductInput, master);
id_resource!(list_accounts, get_account, create_account, update_account, master::Account, master::AccountInput, master);
id_resource!(list_warehouses, get_warehouse, create_warehouse, update_warehouse, master::Warehouse, master::WarehouseInput, master);
id_resource!(list_fiscal_years, get_fiscal_year, create_fiscal_year, update_fiscal_year, calendar::FiscalYear, calendar::FiscalYearInput, calendar);
id_resource!(
    list_accounting_periods,
    get_accounting_period,
    create_accounting_period,
    update_accounting_period,
    calendar::AccountingPeriod,
    calendar::AccountingPeriodInput,
    calendar
);

pub async fn account_ledger(State(state): State<AppState>, Id(id): Id, range: DateRange) -> Result<Json<Vec<LedgerRow>>> {
    Ok(Json(reporting::account_ledger(&state.pool, id, range.from.as_deref(), range.to.as_deref()).await?))
}

// Tax codes and payment terms are keyed by code, and the path's code wins
// over the body's on update.

pub async fn list_tax_codes(State(state): State<AppState>) -> Result<Json<Vec<master::TaxCode>>> {
    Ok(Json(master::list_tax_codes(&state.pool).await?))
}

pub async fn get_tax_code(State(state): State<AppState>, Path(code): Path<String>) -> Result<Json<master::TaxCode>> {
    Ok(Json(master::get_tax_code(&state.pool, &code).await?))
}

pub async fn create_tax_code(State(state): State<AppState>, Body(input): Body<master::TaxCodeInput>) -> Result<(StatusCode, Json<Value>)> {
    let code = master::create_tax_code(&state.pool, input).await?;
    Ok((StatusCode::CREATED, Json(json!({"code": code}))))
}

pub async fn update_tax_code(
    State(state): State<AppState>,
    Path(code): Path<String>,
    Body(input): Body<master::TaxCodeInput>,
) -> Result<StatusCode> {
    master::update_tax_code(&state.pool, &code, input).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_payment_terms(State(state): State<AppState>) -> Result<Json<Vec<master::PaymentTerm>>> {
    Ok(Json(master::list_payment_terms(&state.pool).await?))
}

pub async fn get_payment_term(State(state): State<AppState>, Path(code): Path<String>) -> Result<Json<master::PaymentTerm>> {
    Ok(Json(master::get_payment_term(&state.pool, &code).await?))
}

pub async fn create_payment_term(
    State(state): State<AppState>,
    Body(input): Body<master::PaymentTermInput>,
) -> Result<(StatusCode, Json<Value>)> {
    let code = master::create_payment_term(&state.pool, input).await?;
    Ok((StatusCode::CREATED, Json(json!({"code": code}))))
}

pub async fn update_payment_term(
    State(state): State<AppState>,
    Path(code): Path<String>,
    Body(input): Body<master::PaymentTermInput>,
) -> Result<StatusCode> {
    master::update_payment_term(&state.pool, &code, input).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_settings(State(state): State<AppState>) -> Result<Json<Settings>> {
    Ok(Json(currency::get_settings(&state.pool).await?))
}

pub async fn update_settings(State(state): State<AppState>, Body(input): Body<Settings>) -> Result<StatusCode> {
    currency::update_settings(&state.pool, input).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_exchange_rates(State(state): State<AppState>) -> Result<Json<Vec<ExchangeRate>>> {
    Ok(Json(currency::list_exchange_rates(&state.pool).await?))
}

pub async fn create_exchange_rate(
    State(state): State<AppState>,
    Body(input): Body<ExchangeRateInput>,
) -> Result<(StatusCode, Json<ExchangeRateKey>)> {
    Ok((StatusCode::CREATED, Json(currency::create_exchange_rate(&state.pool, input).await?)))
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub struct RateBody {
    rate: String,
}

pub async fn update_exchange_rate(
    State(state): State<AppState>,
    Path((currency, date)): Path<(String, String)>,
    Body(body): Body<RateBody>,
) -> Result<StatusCode> {
    currency::update_exchange_rate(&state.pool, &currency, &date, &body.rate).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_exchange_rate(State(state): State<AppState>, Path((currency, date)): Path<(String, String)>) -> Result<StatusCode> {
    currency::delete_exchange_rate(&state.pool, &currency, &date).await?;
    Ok(StatusCode::NO_CONTENT)
}
