//! Bank reconciliation endpoints (spec/api.md §5.13).

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::AppState;
use super::extract::{Body, Id};
use crate::banking::{self, Candidate, LineInput, Statement, StatementInput, StatementLine};
use crate::error::{Error, Result};

pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<Statement>>> {
    Ok(Json(banking::list(&state.pool).await?))
}

pub async fn get(State(state): State<AppState>, Id(id): Id) -> Result<Json<Statement>> {
    Ok(Json(banking::get(&state.pool, id).await?))
}

pub async fn create(State(state): State<AppState>, Body(input): Body<StatementInput>) -> Result<(StatusCode, Json<Value>)> {
    Ok((StatusCode::CREATED, Json(json!({"id": banking::create(&state.pool, input).await?}))))
}

pub async fn update(State(state): State<AppState>, Id(id): Id, Body(input): Body<StatementInput>) -> Result<StatusCode> {
    banking::update(&state.pool, id, input).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete(State(state): State<AppState>, Id(id): Id) -> Result<StatusCode> {
    banking::delete(&state.pool, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn lines(State(state): State<AppState>, Id(id): Id) -> Result<Json<Vec<StatementLine>>> {
    Ok(Json(banking::lines(&state.pool, id).await?))
}

pub async fn add_line(State(state): State<AppState>, Id(id): Id, Body(input): Body<LineInput>) -> Result<(StatusCode, Json<Value>)> {
    Ok((StatusCode::CREATED, Json(json!({"id": banking::add_line(&state.pool, id, input).await?}))))
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub struct ImportBody {
    csv: String,
}

pub async fn import(State(state): State<AppState>, Id(id): Id, Body(body): Body<ImportBody>) -> Result<Json<Value>> {
    Ok(Json(json!({"imported": banking::import(&state.pool, id, &body.csv).await?})))
}

pub async fn candidates(State(state): State<AppState>, Id(id): Id) -> Result<Json<Vec<Candidate>>> {
    Ok(Json(banking::candidates(&state.pool, id).await?))
}

pub async fn auto_match(State(state): State<AppState>, Id(id): Id) -> Result<Json<Value>> {
    Ok(Json(json!({"matched": banking::auto_match(&state.pool, id).await?})))
}

pub async fn reconcile(State(state): State<AppState>, Id(id): Id) -> Result<StatusCode> {
    banking::reconcile(&state.pool, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn reopen(State(state): State<AppState>, Id(id): Id) -> Result<StatusCode> {
    banking::reopen(&state.pool, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub struct MatchBody {
    journal_line_id: i64,
}

pub async fn match_line(State(state): State<AppState>, Id(id): Id, Body(body): Body<MatchBody>) -> Result<StatusCode> {
    if body.journal_line_id <= 0 {
        return Err(Error::bad_request("journal_line_id is required"));
    }
    banking::match_line(&state.pool, id, body.journal_line_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn unmatch_line(State(state): State<AppState>, Id(id): Id) -> Result<StatusCode> {
    banking::unmatch_line(&state.pool, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_line(State(state): State<AppState>, Id(id): Id) -> Result<StatusCode> {
    banking::delete_line(&state.pool, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
