//! Endpoints for the four documents with lines (spec/api.md §5.9), one set
//! of routes per kind, plus journal entries and the trial balance.

use axum::extract::State;
use axum::http::StatusCode;
use axum::middleware::from_fn;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

use super::AppState;
use super::extract::{Body, Id};
use super::session::require_admin;
use crate::documents::{self, DocumentInput, Kind};
use crate::error::Result;
use crate::reporting::{self, JournalEntry, TrialBalanceRow};

/// The routes of one kind of document, under /{kind.path}.
pub fn routes(kind: &'static Kind) -> Router<AppState> {
    let base = format!("/{}", kind.path);
    Router::new()
        .route(
            &base,
            get(move |State(s): State<AppState>| async move { Ok::<_, crate::error::Error>(Json(documents::list(&s.pool, kind).await?)) })
                .post(move |State(s): State<AppState>, Body(body): Body<Value>| async move {
                    let id = documents::create(&s.pool, kind, DocumentInput::from_json(kind, body)?).await?;
                    Ok::<_, crate::error::Error>((StatusCode::CREATED, Json(json!({"id": id}))))
                }),
        )
        .route(
            &format!("{base}/{{id}}"),
            get(move |State(s): State<AppState>, Id(id): Id| async move { Ok::<_, crate::error::Error>(Json(documents::get(&s.pool, kind, id).await?)) })
                .put(move |State(s): State<AppState>, Id(id): Id, Body(body): Body<Value>| async move {
                    documents::update(&s.pool, kind, id, DocumentInput::from_json(kind, body)?).await?;
                    Ok::<_, crate::error::Error>(StatusCode::NO_CONTENT)
                })
                .delete(move |State(s): State<AppState>, Id(id): Id| async move {
                    documents::delete(&s.pool, kind, id).await?;
                    Ok::<_, crate::error::Error>(StatusCode::NO_CONTENT)
                }),
        )
        .route(
            &format!("{base}/{{id}}/lines"),
            get(move |State(s): State<AppState>, Id(id): Id| async move { Ok::<_, crate::error::Error>(Json(documents::lines(&s.pool, kind, id).await?)) }),
        )
        .route(
            &format!("{base}/{{id}}/post"),
            post(move |State(s): State<AppState>, Id(id): Id| async move {
                let entry = documents::post(&s.pool, kind, id).await?;
                Ok::<_, crate::error::Error>(Json(json!({"journal_entry_id": entry})))
            }),
        )
        .route(
            &format!("{base}/{{id}}/unpost"),
            post(move |State(s): State<AppState>, Id(id): Id| async move {
                let reversal = documents::unpost(&s.pool, kind, id).await?;
                Ok::<_, crate::error::Error>(Json(json!({"reversal_entry_id": reversal})))
            })
            .route_layer(from_fn(require_admin)),
        )
}

pub async fn journal_entry(State(state): State<AppState>, Id(id): Id) -> Result<Json<JournalEntry>> {
    Ok(Json(reporting::journal_entry(&state.pool, id).await?))
}

pub async fn trial_balance(State(state): State<AppState>) -> Result<Json<Vec<TrialBalanceRow>>> {
    Ok(Json(reporting::trial_balance(&state.pool).await?))
}
