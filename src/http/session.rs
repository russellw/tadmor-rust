//! Cookie sessions (spec/api.md §3): the middleware that guards the API,
//! administrator gating, and the login, logout and me endpoints.

use axum::Json;
use axum::extract::{Request, State};
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use super::AppState;
use super::extract::{Body, CurrentUser};
use crate::auth::{self, SESSION_DAYS, User};
use crate::error::Error;

const COOKIE_NAME: &str = "tadmor_session";

/// The paths under /api/ that need no session.
const PUBLIC: [&str; 2] = ["/auth/login", "/auth/logout"];

fn session_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE_NAME)
        .map(|(_, value)| value.to_string())
        .filter(|value| !value.is_empty())
}

/// Whether the client reached us over HTTPS, as the reverse proxy reports it.
fn is_https(headers: &HeaderMap) -> bool {
    headers.get("x-forwarded-proto").is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"https"))
}

fn session_cookie(value: &str, max_age: i64, secure: bool) -> HeaderValue {
    let secure = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!("{COOKIE_NAME}={value}; Path=/; Max-Age={max_age}; HttpOnly; SameSite=Lax{secure}"))
        .expect("cookie is ASCII")
}

/// Rejects requests under /api/ without a live session, and attaches the
/// session's user, re-read from the database, to the rest.
pub async fn require_session(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    if PUBLIC.contains(&req.uri().path()) {
        return next.run(req).await;
    }
    let Some(token) = session_token(req.headers()) else {
        return Error::unauthorized("authentication required").into_response();
    };
    match auth::session_user(&state.pool, &token).await {
        Ok(Some(user)) => {
            req.extensions_mut().insert(user);
            next.run(req).await
        }
        Ok(None) => {
            let mut response = Error::unauthorized("session expired").into_response();
            response.headers_mut().insert(SET_COOKIE, session_cookie("", 0, is_https(req.headers())));
            response
        }
        Err(err) => err.into_response(),
    }
}

/// Lets only administrators through. Runs inside `require_session`.
pub async fn require_admin(req: Request, next: Next) -> Response {
    match req.extensions().get::<User>() {
        Some(user) if user.is_admin => next.run(req).await,
        _ => Error::forbidden("administrator access required").into_response(),
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct LoginBody {
    email: String,
    password: String,
}

pub async fn login(State(state): State<AppState>, headers: HeaderMap, Body(body): Body<LoginBody>) -> Result<Response, Error> {
    let (user, token) = auth::login(&state.pool, &body.email, &body.password).await?;
    eprintln!("login: {}", user.email);
    let cookie = session_cookie(&token, SESSION_DAYS * 24 * 60 * 60, is_https(&headers));
    Ok(([(SET_COOKIE, cookie)], Json(user)).into_response())
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Error> {
    if let Some(token) = session_token(&headers) {
        auth::logout(&state.pool, &token).await?;
    }
    Ok((StatusCode::NO_CONTENT, [(SET_COOKIE, session_cookie("", 0, is_https(&headers)))]).into_response())
}

pub async fn me(CurrentUser(user): CurrentUser) -> Json<User> {
    Json(user)
}
