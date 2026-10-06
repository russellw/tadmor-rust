//! Request extractors that refuse in the API's own error shape, where
//! axum's built-in ones would answer in plain text or with other statuses.

use axum::body::Bytes;
use std::collections::HashMap;

use axum::extract::{FromRequest, FromRequestParts, Path, Query, Request};
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

/// A path id: a positive integer that fits in 64 bits, or a 400. One too
/// large for any row is still well formed, and simply finds nothing (404).
pub struct Id(pub i64);

impl<S: Send + Sync> FromRequestParts<S> for Id {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Error> {
        let Path(raw) = Path::<String>::from_request_parts(parts, state)
            .await
            .map_err(|_| Error::bad_request("invalid id"))?;
        match raw.parse::<i64>() {
            Ok(id) if id > 0 => Ok(Id(id)),
            _ => Err(Error::bad_request("invalid id")),
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

/// Whether `s` is a real calendar date written `YYYY-MM-DD`.
pub fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' || !digits(0..4) || !digits(5..7) || !digits(8..10) {
        return false;
    }
    let (y, m, d): (u32, u32, u32) = (s[0..4].parse().unwrap(), s[5..7].parse().unwrap(), s[8..10].parse().unwrap());
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&d)
}

/// Optional `from` and `to` query parameters: inclusive date bounds, absent
/// or empty meaning unbounded. A malformed one is a 400 (spec/api.md §5.14).
pub struct DateRange {
    pub from: Option<String>,
    pub to: Option<String>,
}

impl<S: Send + Sync> FromRequestParts<S> for DateRange {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Error> {
        let Query(mut params) = Query::<HashMap<String, String>>::from_request_parts(parts, state)
            .await
            .map_err(|_| Error::bad_request("malformed query string"))?;
        let mut bound = |key: &str| match params.remove(key).filter(|v| !v.is_empty()) {
            Some(v) if !is_date(&v) => Err(Error::bad_request(format!("{key} must be a YYYY-MM-DD date"))),
            v => Ok(v),
        };
        Ok(DateRange { from: bound("from")?, to: bound("to")? })
    }
}

/// An optional `as_of` date query parameter; a malformed one is a 400.
pub struct AsOf(pub Option<String>);

impl<S: Send + Sync> FromRequestParts<S> for AsOf {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Error> {
        let Query(mut params) = Query::<HashMap<String, String>>::from_request_parts(parts, state)
            .await
            .map_err(|_| Error::bad_request("malformed query string"))?;
        match params.remove("as_of").filter(|v| !v.is_empty()) {
            Some(v) if !is_date(&v) => Err(Error::bad_request("as_of must be a YYYY-MM-DD date")),
            v => Ok(AsOf(v)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_must_be_real() {
        for ok in ["2024-02-29", "2000-02-29", "2101-12-31", "0001-01-01"] {
            assert!(is_date(ok), "{ok}");
        }
        for bad in ["2023-02-29", "1900-02-29", "2101-13-01", "2101-00-10", "2101-01-32", "yesterday", "2101-1-01", "21010101", "", "2101-01-0a", "2101-01-0é"] {
            assert!(!is_date(bad), "{bad}");
        }
    }
}
