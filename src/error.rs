//! The error every service and handler returns: an HTTP status from
//! spec/api.md §1.4 and a message, answered as `{"error": "..."}`.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug)]
pub struct Error {
    pub status: StatusCode,
    pub message: String,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Error {
        Error { status, message: message.into() }
    }

    pub fn bad_request(message: impl Into<String>) -> Error {
        Error::new(StatusCode::BAD_REQUEST, message)
    }

    pub fn unauthorized(message: impl Into<String>) -> Error {
        Error::new(StatusCode::UNAUTHORIZED, message)
    }

    pub fn forbidden(message: impl Into<String>) -> Error {
        Error::new(StatusCode::FORBIDDEN, message)
    }

    pub fn not_found() -> Error {
        Error::new(StatusCode::NOT_FOUND, "not found")
    }

    pub fn unprocessable(message: impl Into<String>) -> Error {
        Error::new(StatusCode::UNPROCESSABLE_ENTITY, message)
    }

    /// A server fault. The detail goes to the log, not the client.
    pub fn internal(detail: impl std::fmt::Display) -> Error {
        eprintln!("internal error: {detail}");
        Error::new(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
    }
}

/// Refusals by the schema become client errors, by SQLSTATE: a duplicate
/// key is a conflict, and a broken foreign key, check, not-null or
/// exclusion constraint, a raised exception, or a data exception (class
/// 22, such as an invalid date or decimal) is unprocessable. Anything else
/// is a server fault.
impl From<sqlx::Error> for Error {
    fn from(err: sqlx::Error) -> Error {
        if let sqlx::Error::Database(db) = &err {
            let code = db.code().unwrap_or_default();
            match code.as_ref() {
                "23505" => return Error::new(StatusCode::CONFLICT, db.message()),
                "23503" | "23514" | "23502" | "23P01" | "P0001" => return Error::unprocessable(db.message()),
                c if c.starts_with("22") => return Error::unprocessable(db.message()),
                _ => {}
            }
        }
        Error::internal(err)
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (self.status, Json(json!({"error": self.message}))).into_response()
    }
}
