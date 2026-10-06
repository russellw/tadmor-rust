//! Reports (spec/api.md §5.14) and year-end close and reopen (§5.7).

use axum::Json;
use axum::extract::State;
use serde::Deserialize;
use serde_json::{Value, json};

use super::AppState;
use super::extract::{AsOf, Body, DateRange, Id};
use crate::error::{Error, Result};
use crate::reporting::{self, ActivityRow, AgingRow, BalanceSheet, CashFlow, ValuationRow};
use crate::yearend::{self, Closed};

pub async fn profit_and_loss(State(state): State<AppState>, range: DateRange) -> Result<Json<Vec<ActivityRow>>> {
    Ok(Json(reporting::profit_and_loss(&state.pool, range.from.as_deref(), range.to.as_deref()).await?))
}

pub async fn balance_sheet(State(state): State<AppState>, AsOf(as_of): AsOf) -> Result<Json<BalanceSheet>> {
    Ok(Json(reporting::balance_sheet(&state.pool, as_of.as_deref()).await?))
}

pub async fn cash_flow(State(state): State<AppState>, range: DateRange) -> Result<Json<CashFlow>> {
    Ok(Json(reporting::cash_flow(&state.pool, range.from.as_deref(), range.to.as_deref()).await?))
}

pub async fn ar_aging(State(state): State<AppState>) -> Result<Json<Vec<AgingRow>>> {
    Ok(Json(reporting::ar_aging(&state.pool).await?))
}

pub async fn ap_aging(State(state): State<AppState>) -> Result<Json<Vec<AgingRow>>> {
    Ok(Json(reporting::ap_aging(&state.pool).await?))
}

pub async fn inventory_valuation(State(state): State<AppState>) -> Result<Json<Vec<ValuationRow>>> {
    Ok(Json(reporting::inventory_valuation(&state.pool).await?))
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub struct CloseBody {
    retained_earnings_account_id: i64,
}

pub async fn close_year(State(state): State<AppState>, Id(id): Id, Body(body): Body<CloseBody>) -> Result<Json<Closed>> {
    if body.retained_earnings_account_id <= 0 {
        return Err(Error::bad_request("retained_earnings_account_id is required"));
    }
    Ok(Json(yearend::close(&state.pool, id, body.retained_earnings_account_id).await?))
}

pub async fn reopen_year(State(state): State<AppState>, Id(id): Id) -> Result<Json<Value>> {
    Ok(Json(json!({"reversal_entry_id": yearend::reopen(&state.pool, id).await?})))
}
