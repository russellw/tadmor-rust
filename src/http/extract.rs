//! Request extractors that refuse in the API's own error shape, where
//! axum's built-in ones would answer in plain text or with other statuses.

use axum::body::Bytes;
use axum::extract::{FromRequest, FromRequestParts, Path, Request};
use axum::http::request::Parts;
use serde::de::DeserializeOwned;

use crate::auth::User;
use crate::error::Error;

/// A JSON request body. Anything that does not decode, an empty body
/// included, is a 400 (spec/api.md §1.4). The Content-Type is not checked,
/// and unknown fields are ignored (§1.1).
pub struct Body<T>(pub T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for Body<T> {
    type Rejection = Error;

    async fn from_request(req: Request, state: &S) -> Result<Self, Error> {
        let bytes = Bytes::from_request(req, state).await.map_err(|e| Error::bad_request(e.body_text()))?;
        serde_json::from_slice(&bytes).map(Body).map_err(|e| Error::bad_request(format!("invalid JSON body: {e}")))
    }
}

/// A path id: a positive integer, or a 400. One too large for any row is
/// still well formed, and simply finds nothing (404).
pub struct Id(pub i64);

impl<S: Send + Sync> FromRequestParts<S> for Id {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Error> {
        let Path(raw) = Path::<String>::from_request_parts(parts, state)
            .await
            .map_err(|_| Error::bad_request("invalid id"))?;
        if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Error::bad_request("invalid id"));
        }
        match raw.parse::<u64>() {
            Ok(0) => Err(Error::bad_request("invalid id")),
            Ok(n) => Ok(Id(i64::try_from(n).unwrap_or(i64::MAX))),
            Err(_) => Ok(Id(i64::MAX)), // digits beyond u64: positive, and matches nothing
        }
    }
}

/// The user that `require_session` attached to the request.
pub struct CurrentUser(pub User);

impl<S: Send + Sync> FromRequestParts<S> for CurrentUser {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Error> {
        parts.extensions.get::<User>().cloned().map(CurrentUser).ok_or_else(|| Error::unauthorized("authentication required"))
    }
}
