//! Payment endpoints (spec/api.md §5.9), and applying payments and credit
//! notes: one set of routes per kind.

use axum::extract::State;
use axum::http::StatusCode;
use axum::middleware::from_fn;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

use super::AppState;
use super::extract::{Body, Id};
use super::session::require_admin;
use crate::error::Error;
use crate::payments::{self, PaymentInput, PaymentKind};
use crate::settlement::{self, Settler};

/// The routes of one kind of payment, under /{kind.path}.
pub fn routes(kind: &'static PaymentKind) -> Router<AppState> {
    let base = format!("/{}", kind.path);
    Router::new()
        .route(
            &base,
            get(move |State(s): State<AppState>| async move { Ok::<_, Error>(Json(payments::list(&s.pool, kind).await?)) })
                .post(move |State(s): State<AppState>, Body(body): Body<Value>| async move {
                    let id = payments::create(&s.pool, kind, PaymentInput::from_json(kind, body)?).await?;
                    Ok::<_, Error>((StatusCode::CREATED, Json(json!({"id": id}))))
                }),
        )
        .route(
            &format!("{base}/{{id}}"),
            get(move |State(s): State<AppState>, Id(id): Id| async move { Ok::<_, Error>(Json(payments::get(&s.pool, kind, id).await?)) })
                .put(move |State(s): State<AppState>, Id(id): Id, Body(body): Body<Value>| async move {
                    payments::update(&s.pool, kind, id, PaymentInput::from_json(kind, body)?).await?;
                    Ok::<_, Error>(StatusCode::NO_CONTENT)
                })
                .delete(move |State(s): State<AppState>, Id(id): Id| async move {
                    payments::delete(&s.pool, kind, id).await?;
                    Ok::<_, Error>(StatusCode::NO_CONTENT)
                }),
        )
        .route(
            &format!("{base}/{{id}}/post"),
            post(move |State(s): State<AppState>, Id(id): Id| async move {
                Ok::<_, Error>(Json(json!({"journal_entry_id": payments::post(&s.pool, kind, id).await?})))
            }),
        )
        .route(
            &format!("{base}/{{id}}/unpost"),
            post(move |State(s): State<AppState>, Id(id): Id| async move {
                Ok::<_, Error>(Json(json!({"reversal_entry_id": payments::unpost(&s.pool, kind, id).await?})))
            })
            .route_layer(from_fn(require_admin)),
        )
}

/// The apply and applications routes of a settling document.
pub fn settlement_routes(s: &'static Settler) -> Router<AppState> {
    let base = format!("/{}/{{id}}", s.path);
    Router::new()
        .route(
            &format!("{base}/apply"),
            post(move |State(st): State<AppState>, Id(id): Id| async move {
                Ok::<_, Error>(Json(json!({"applications": settlement::apply(&st.pool, s, id).await?})))
            }),
        )
        .route(
            &format!("{base}/applications"),
            get(move |State(st): State<AppState>, Id(id): Id| async move { Ok::<_, Error>(Json(settlement::applications(&st.pool, s, id).await?)) }),
        )
}
