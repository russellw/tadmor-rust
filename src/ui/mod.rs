//! The server-rendered user interface (spec/domain.md §13): Askama
//! templates over the same service modules the JSON API uses.
//!
//! Every page but the login screen needs a session, the same cookie the
//! API uses; without one the browser is sent to sign in and then back.
//! Every form posts a token derived from the session, and the pages carry a
//! Content Security Policy that allows no inline script or style.

mod accounting;
mod documents;
mod forms;
mod home;
mod lookups;
mod master;
mod orders;
mod reports;
mod view;

use axum::extract::{FromRequestParts, Request, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, LOCATION, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Form, Router};
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub use forms::{FormData, Values};
pub use view::{Action, Cell, Field, FieldKind, Form as FormView, Section, Table};

use crate::auth::{self, SESSION_DAYS, User};
use crate::error::Error;
use crate::http::AppState;
use crate::http::session::{is_https, session_cookie, session_token};
use view::{Chrome, LoginTemplate, NavGroup, PageTemplate};

/// The paths under the UI that need no session.
fn is_public(path: &str) -> bool {
    path == "/login" || path.starts_with("/static/")
}

/// The user and form token behind a UI request.
#[derive(Clone)]
pub struct Session {
    pub user: User,
    pub token: String,
}

impl<S: Send + Sync> FromRequestParts<S> for Session {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Response> {
        parts.extensions.get::<Session>().cloned().ok_or_else(|| Redirect::to("/login").into_response())
    }
}

/// The token that forms post: derived from the session, so another site
/// cannot know it, and stable for the session's life.
fn form_token(session: &str) -> String {
    Sha256::digest(format!("tadmor form token:{session}").as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Sends a request without a live session to the login screen, remembering
/// where it was going, and attaches the session to the rest.
async fn require_login(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    if is_public(&path) {
        return next.run(req).await;
    }
    let user = match session_token(req.headers()) {
        Some(token) => match auth::session_user(&state.pool, &token).await {
            Ok(Some(user)) => Some((user, token)),
            Ok(None) => None,
            Err(err) => return err.into_response(),
        },
        None => None,
    };
    let Some((user, token)) = user else {
        let target = req.uri().path_and_query().map(|p| p.as_str()).unwrap_or("/");
        let next = if req.method() == axum::http::Method::GET { target } else { "/" };
        return Redirect::to(&format!("/login?next={}", forms::url_encode(next))).into_response();
    };
    req.extensions_mut().insert(Session { user, token: form_token(&token) });
    next.run(req).await
}

/// No inline script or style, nothing from elsewhere, and not framed.
async fn security_headers(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'self'; img-src 'self' data:; form-action 'self'; frame-ancestors 'none'; base-uri 'self'"),
    );
    headers.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    headers.insert("referrer-policy", HeaderValue::from_static("same-origin"));
    response
}

pub fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/login", get(login_page).post(login))
        .route("/logout", axum::routing::post(logout))
        .route("/static/app.css", get(|| async { asset("text/css", include_str!("../../static/app.css")) }))
        .route("/static/app.js", get(|| async { asset("text/javascript", include_str!("../../static/app.js")) }))
        .merge(home::routes())
        .merge(master::routes())
        .merge(documents::routes())
        .merge(orders::routes())
        .merge(reports::routes())
        .merge(accounting::routes())
        .fallback(not_found)
        .layer(from_fn_with_state(state, require_login))
        .layer(axum::middleware::from_fn(security_headers))
}

fn asset(content_type: &'static str, body: &'static str) -> Response {
    ([(CONTENT_TYPE, content_type), (CACHE_CONTROL, "no-cache")], body).into_response()
}

/// G8: any other path, for a signed-in user, is a not-found page.
pub async fn not_found(session: Session) -> Response {
    let mut response = page(&session, "Not found", vec![Section::Text("There is no page at this address.".into())]);
    *response.status_mut() = StatusCode::NOT_FOUND;
    response
}

fn nav(is_admin: bool) -> Vec<NavGroup> {
    let mut master = vec![
        ("Organizations", "/organizations"),
        ("Customers", "/customers"),
        ("Suppliers", "/suppliers"),
        ("Products", "/products"),
        ("Chart of accounts", "/accounts"),
        ("Tax codes", "/tax-codes"),
        ("Payment terms", "/payment-terms"),
        ("Warehouses", "/warehouses"),
        ("Settings", "/settings"),
    ];
    if is_admin {
        master.push(("Users", "/users"));
    }
    vec![
        NavGroup { title: "Overview", links: vec![("Home", "/")] },
        NavGroup {
            title: "Sales",
            links: vec![
                ("Sales orders", "/sales-orders"),
                ("Invoices", "/sales-invoices"),
                ("Credit notes", "/sales-credit-notes"),
                ("Customer payments", "/customer-payments"),
            ],
        },
        NavGroup {
            title: "Purchasing",
            links: vec![
                ("Purchase orders", "/purchase-orders"),
                ("Bills", "/purchase-bills"),
                ("Supplier credits", "/purchase-credit-notes"),
                ("Supplier payments", "/supplier-payments"),
            ],
        },
        NavGroup { title: "Inventory", links: vec![("Stock movements", "/stock-movements"), ("Valuation", "/reports/inventory-valuation")] },
        NavGroup {
            title: "Reports",
            links: vec![
                ("Profit and loss", "/reports/profit-and-loss"),
                ("Balance sheet", "/reports/balance-sheet"),
                ("Cash flow", "/reports/cash-flow"),
                ("Trial balance", "/reports/trial-balance"),
                ("A/R aging", "/reports/ar-aging"),
                ("A/P aging", "/reports/ap-aging"),
            ],
        },
        NavGroup {
            title: "Accounting",
            links: vec![("Periods", "/periods"), ("Exchange rates", "/exchange-rates"), ("Bank statements", "/bank-statements")],
        },
        NavGroup { title: "Master data", links: master },
    ]
}

fn chrome(session: &Session) -> Chrome {
    Chrome {
        user_name: session.user.full_name.clone(),
        is_admin: session.user.is_admin,
        token: session.token.clone(),
        nav: nav(session.user.is_admin),
    }
}

fn render(template: impl askama::Template) -> Response {
    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(err) => Error::internal(err).into_response(),
    }
}

/// A page of sections inside the usual chrome.
pub fn page(session: &Session, title: &str, sections: Vec<Section>) -> Response {
    let chrome = chrome(session);
    render(PageTemplate { chrome: &chrome, title, sections: &sections })
}

/// After a successful post, go on to `to`.
pub fn see_other(to: &str) -> Response {
    (StatusCode::SEE_OTHER, [(LOCATION, to.to_string())]).into_response()
}

/// G4: the page for an administrator-only screen asked for by someone else.
pub fn forbidden(session: &Session) -> Response {
    let mut response = page(session, "Not allowed", vec![Section::Error("Only an administrator can do this.".into())]);
    *response.status_mut() = StatusCode::FORBIDDEN;
    response
}

/// The not-found page for a record that does not exist.
pub fn missing(session: &Session) -> Response {
    let mut response = page(session, "Not found", vec![Section::Text("That record does not exist.".into())]);
    *response.status_mut() = StatusCode::NOT_FOUND;
    response
}

/// Renders a service error that has no form to return to: not found as a
/// not-found page, anything else as a message.
pub fn failure(session: &Session, err: Error) -> Response {
    if err.status == StatusCode::NOT_FOUND {
        return missing(session);
    }
    let mut response = page(session, "Something went wrong", vec![Section::Error(err.message)]);
    *response.status_mut() = err.status;
    response
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NextQuery {
    next: String,
}

/// Where to go after signing in: a local path only.
fn safe_next(next: &str) -> &str {
    if next.starts_with('/') && !next.starts_with("//") && !next.starts_with("/\\") { next } else { "/" }
}

async fn login_page(axum::extract::Query(q): axum::extract::Query<NextQuery>) -> Response {
    render(LoginTemplate { next: safe_next(&q.next), email: "", error: None })
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LoginForm {
    email: String,
    password: String,
    next: String,
}

/// G1: signs in, or shows the login screen again with the error.
async fn login(State(state): State<AppState>, headers: axum::http::HeaderMap, Form(f): Form<LoginForm>) -> Response {
    let next = safe_next(&f.next).to_string();
    match auth::login(&state.pool, &f.email, &f.password).await {
        Ok((_, token)) => {
            let cookie = session_cookie(&token, SESSION_DAYS * 24 * 60 * 60, is_https(&headers));
            let mut response = see_other(&next);
            response.headers_mut().insert(SET_COOKIE, cookie);
            response
        }
        Err(err) => {
            let mut response = render(LoginTemplate { next: &next, email: &f.email, error: Some(&err.message) });
            *response.status_mut() = err.status;
            response
        }
    }
}

/// G2: signs out, and back to the login screen.
async fn logout(State(state): State<AppState>, headers: axum::http::HeaderMap, session: Session, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    if let Some(token) = session_token(&headers) {
        let _ = auth::logout(&state.pool, &token).await;
    }
    let mut response = see_other("/login");
    response.headers_mut().insert(SET_COOKIE, session_cookie("", 0, is_https(&headers)));
    response
}
