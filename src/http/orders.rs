//! Order endpoints (spec/api.md §5.10): one set of routes per side.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

use super::AppState;
use super::extract::{Body, Id};
use crate::documents::{self, DocumentInput, PURCHASE_BILLS, SALES_INVOICES};
use crate::error::Error;
use crate::orders::{self, DocumentRequest, PURCHASE, SALES, Side, StockRequest};

/// The routes every order has, under /{path}.
fn common(s: &'static Side) -> Router<AppState> {
    let kind = s.kind;
    let base = format!("/{}", kind.path);
    Router::new()
        .route(
            &base,
            get(move |State(st): State<AppState>| async move { Ok::<_, Error>(Json(orders::list(&st.pool, s).await?)) })
                .post(move |State(st): State<AppState>, Body(body): Body<Value>| async move {
                    let id = documents::create(&st.pool, kind, DocumentInput::from_json(kind, body)?).await?;
                    Ok::<_, Error>((StatusCode::CREATED, Json(json!({"id": id}))))
                }),
        )
        .route(
            &format!("{base}/{{id}}"),
            get(move |State(st): State<AppState>, Id(id): Id| async move { Ok::<_, Error>(Json(orders::get(&st.pool, s, id).await?)) })
                .put(move |State(st): State<AppState>, Id(id): Id, Body(body): Body<Value>| async move {
                    documents::update(&st.pool, kind, id, DocumentInput::from_json(kind, body)?).await?;
                    Ok::<_, Error>(StatusCode::NO_CONTENT)
                })
                .delete(move |State(st): State<AppState>, Id(id): Id| async move {
                    documents::delete(&st.pool, kind, id).await?;
                    Ok::<_, Error>(StatusCode::NO_CONTENT)
                }),
        )
        .route(
            &format!("{base}/{{id}}/lines"),
            get(move |State(st): State<AppState>, Id(id): Id| async move { Ok::<_, Error>(Json(orders::lines(&st.pool, s, id).await?)) }),
        )
        .route(
            &format!("{base}/{{id}}/confirm"),
            post(move |State(st): State<AppState>, Id(id): Id| async move {
                orders::confirm(&st.pool, s, id).await?;
                Ok::<_, Error>(StatusCode::NO_CONTENT)
            }),
        )
        .route(
            &format!("{base}/{{id}}/close"),
            post(move |State(st): State<AppState>, Id(id): Id| async move {
                orders::close(&st.pool, s, id).await?;
                Ok::<_, Error>(StatusCode::NO_CONTENT)
            }),
        )
        .route(
            &format!("{base}/{{id}}/cancel"),
            post(move |State(st): State<AppState>, Id(id): Id| async move {
                orders::cancel(&st.pool, s, id).await?;
                Ok::<_, Error>(StatusCode::NO_CONTENT)
            }),
        )
}

pub fn routes() -> Router<AppState> {
    common(&SALES)
        .merge(common(&PURCHASE))
        .route(
            "/sales-orders/{id}/invoice",
            post(|State(st): State<AppState>, Id(id): Id, Body(r): Body<DocumentRequest>| async move {
                let invoice = orders::fulfil_document(&st.pool, &SALES, &SALES_INVOICES, id, r).await?;
                Ok::<_, Error>((StatusCode::CREATED, Json(json!({"invoice_id": invoice}))))
            }),
        )
        .route(
            "/purchase-orders/{id}/bill",
            post(|State(st): State<AppState>, Id(id): Id, Body(r): Body<DocumentRequest>| async move {
                let bill = orders::fulfil_document(&st.pool, &PURCHASE, &PURCHASE_BILLS, id, r).await?;
                Ok::<_, Error>((StatusCode::CREATED, Json(json!({"bill_id": bill}))))
            }),
        )
        .route(
            "/sales-orders/{id}/ship",
            post(|State(st): State<AppState>, Id(id): Id, Body(r): Body<StockRequest>| async move {
                Ok::<_, Error>((StatusCode::CREATED, Json(json!({"movement_ids": orders::ship(&st.pool, id, r).await?}))))
            }),
        )
        .route(
            "/purchase-orders/{id}/receive",
            post(|State(st): State<AppState>, Id(id): Id, Body(r): Body<StockRequest>| async move {
                Ok::<_, Error>((StatusCode::CREATED, Json(json!({"movement_ids": orders::receive(&st.pool, id, r).await?}))))
            }),
        )
}
