//! PDF and email endpoints for the printable documents (spec/api.md §5.11).

use axum::extract::State;
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use super::AppState;
use super::extract::{Id, OptionalBody};
use crate::error::Error;
use crate::printing::{self, Printable};

/// The body of an email request; `to` defaults to the counterparty's email.
#[derive(Default, Deserialize)]
#[serde(default)]
pub struct EmailBody {
    to: Vec<String>,
}

pub fn routes(p: &'static Printable) -> Router<AppState> {
    let base = format!("/{}/{{id}}", p.kind.path);
    Router::new()
        .route(
            &format!("{base}/pdf"),
            get(move |State(st): State<AppState>, Id(id): Id| async move {
                let printed = printing::render(&st.pool, p, id).await?;
                let disposition = format!("inline; filename=\"{}\"", printed.filename(p));
                let mut response = printed.pdf.into_response();
                let headers = response.headers_mut();
                headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/pdf"));
                headers.insert(CONTENT_DISPOSITION, HeaderValue::from_str(&disposition).map_err(Error::internal)?);
                Ok::<Response, Error>(response)
            }),
        )
        .route(
            &format!("{base}/email"),
            post(move |State(st): State<AppState>, Id(id): Id, OptionalBody(body): OptionalBody<EmailBody>| async move {
                let default = printing::recipient(&st.pool, p, id).await?;
                let to = if body.to.is_empty() {
                    vec![default.filter(|e| !e.is_empty()).ok_or_else(|| {
                        Error::unprocessable("no recipient given, and the counterparty organization has no email")
                    })?]
                } else {
                    body.to
                };
                let Some(mailer) = &st.mailer else {
                    return Err(Error::new(StatusCode::NOT_IMPLEMENTED, "email sending is not configured"));
                };
                let printed = printing::render(&st.pool, p, id).await?;
                let subject = format!("{} {}", p.label, printed.number);
                let note = format!("Please find attached {} {}.", p.label.to_lowercase(), printed.number);
                let filename = printed.filename(p);
                mailer.send_pdf(&to, &subject, &note, &filename, printed.pdf).await?;
                Ok(Json(json!({"status": "sent", "to": to})))
            }),
        )
}
