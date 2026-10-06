//! Stock movement endpoints (spec/api.md §5.12).

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::AppState;
use super::extract::{Body, Id, OptionalBody};
use crate::error::Result;
use crate::inventory::{self, MovementInput, StockMovement};

pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<StockMovement>>> {
    Ok(Json(inventory::list(&state.pool).await?))
}

pub async fn get(State(state): State<AppState>, Id(id): Id) -> Result<Json<StockMovement>> {
    Ok(Json(inventory::get(&state.pool, id).await?))
}

pub async fn create(State(state): State<AppState>, Body(input): Body<MovementInput>) -> Result<(StatusCode, Json<Value>)> {
    let id = inventory::create(&state.pool, input).await?;
    Ok((StatusCode::CREATED, Json(json!({"id": id}))))
}

pub async fn update(State(state): State<AppState>, Id(id): Id, Body(input): Body<MovementInput>) -> Result<StatusCode> {
    inventory::update(&state.pool, id, input).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete(State(state): State<AppState>, Id(id): Id) -> Result<StatusCode> {
    inventory::delete(&state.pool, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Required for receipts, ignored otherwise; the whole body may be absent.
#[derive(Default, Deserialize)]
#[serde(default)]
pub struct PostBody {
    credit_account_id: Option<i64>,
}

pub async fn post(State(state): State<AppState>, Id(id): Id, OptionalBody(body): OptionalBody<PostBody>) -> Result<Json<Value>> {
    Ok(Json(json!({"journal_entry_id": inventory::post(&state.pool, id, body.credit_account_id).await?})))
}

pub async fn unpost(State(state): State<AppState>, Id(id): Id) -> Result<Json<Value>> {
    Ok(Json(json!({"reversal_entry_id": inventory::unpost(&state.pool, id).await?})))
}
