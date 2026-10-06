//! Shared setup for the integration tests, which run against
//! TEST_DATABASE_URL and drop and recreate its public schema.

#![allow(dead_code)] // each test binary uses its own subset

use std::sync::Mutex;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::header::{CONTENT_TYPE, COOKIE, SET_COOKIE};
use axum::http::{Request, StatusCode};
use serde_json::Value;
use sqlx::PgPool;
use tadmor::db;
use tadmor::http::{AppState, router};
use tadmor::users::{self, NewUser};
use tower::ServiceExt;

/// Whether this test process has reset the schema yet. Each test runs on
/// its own runtime and thread; the first to get here resets, the rest wait.
static RESET: Mutex<bool> = Mutex::new(false);

pub async fn pool() -> PgPool {
    let url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must name a throwaway database; its public schema is dropped");
    let pool = db::connect(&url).await.expect("connect to TEST_DATABASE_URL");
    #[allow(clippy::await_holding_lock)]
    {
        let mut reset = RESET.lock().unwrap_or_else(|e| e.into_inner());
        if !*reset {
            sqlx::raw_sql("DROP SCHEMA public CASCADE; CREATE SCHEMA public")
                .execute(&pool)
                .await
                .expect("reset schema");
            db::migrate(&pool).await.expect("migrate");
            *reset = true;
        }
    }
    pool
}

pub async fn app() -> Router {
    router(AppState::new(pool().await))
}

/// A unique email for a test's own user, so tests sharing the database never
/// collide.
pub fn email(tag: &str) -> String {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{tag}-{}-{n}@test.example", std::process::id())
}

/// Creates a user directly through the service, and returns its id.
pub async fn user(pool: &PgPool, email: &str, password: &str, is_admin: bool) -> i32 {
    let new = NewUser { email: email.into(), full_name: "Test User".into(), password: password.into(), is_admin };
    users::create(pool, new).await.expect("create user")
}

/// A client for the router in-process, holding a session cookie once it
/// logs in, as a browser would.
pub struct Client {
    pub app: Router,
    pub cookie: Option<String>,
}

pub struct Reply {
    pub status: StatusCode,
    pub set_cookie: Vec<String>,
    pub body: Value,
}

impl Client {
    pub fn new(app: Router) -> Client {
        Client { app, cookie: None }
    }

    pub async fn send(&mut self, method: &str, path: &str, body: Option<&str>) -> Reply {
        let mut req = Request::builder().method(method).uri(path);
        if let Some(cookie) = &self.cookie {
            req = req.header(COOKIE, cookie);
        }
        if body.is_some() {
            req = req.header(CONTENT_TYPE, "application/json");
        }
        let req = req.body(body.map(|b| Body::from(b.to_string())).unwrap_or_default()).unwrap();
        let response = self.app.clone().oneshot(req).await.unwrap();
        let status = response.status();
        let set_cookie: Vec<String> =
            response.headers().get_all(SET_COOKIE).iter().map(|v| v.to_str().unwrap().to_string()).collect();
        for c in &set_cookie {
            let pair = c.split(';').next().unwrap();
            self.cookie = if pair.ends_with('=') { None } else { Some(pair.to_string()) };
        }
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
        Reply { status, set_cookie, body }
    }

    pub async fn get(&mut self, path: &str) -> Reply {
        self.send("GET", path, None).await
    }

    pub async fn post(&mut self, path: &str, body: Value) -> Reply {
        self.send("POST", path, Some(&body.to_string())).await
    }

    pub async fn put(&mut self, path: &str, body: Value) -> Reply {
        self.send("PUT", path, Some(&body.to_string())).await
    }

    /// Logs in and panics unless it succeeds.
    pub async fn login(&mut self, email: &str, password: &str) -> Reply {
        let r = self.post("/api/auth/login", serde_json::json!({"email": email, "password": password})).await;
        assert_eq!(r.status, StatusCode::OK, "login as {email}: {}", r.body);
        r
    }
}

/// Sends a GET to the router in-process and returns the status and JSON body.
pub async fn get(app: Router, path: &str) -> (StatusCode, Value) {
    let r = Client::new(app).get(path).await;
    (r.status, r.body)
}

/// A client logged in as a fresh administrator of its own.
pub async fn admin() -> Client {
    let pool = pool().await;
    let email = email("admin");
    user(&pool, &email, "admin password", true).await;
    let mut c = Client::new(router(AppState::new(pool)));
    c.login(&email, "admin password").await;
    c
}

/// A unique short code for a test's own records (SKUs, account codes, names).
pub fn code(tag: &str) -> String {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{tag}{}x{n}", std::process::id())
}

/// POSTs and returns the new record's id, panicking unless it is a 201.
pub async fn create(c: &mut Client, path: &str, body: Value) -> i64 {
    let r = c.post(path, body.clone()).await;
    assert_eq!(r.status, StatusCode::CREATED, "POST {path} {body}: {}", r.body);
    r.body["id"].as_i64().unwrap()
}

/// A calendar year of the test's own, with an open fiscal year covering it.
/// Accounting periods never overlap, so tests that post never share a year.
pub async fn open_year(c: &mut Client) -> i32 {
    static NEXT: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(4000);
    let y = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    create(c, "/api/fiscal-years", serde_json::json!({"name": code("FY"), "start_date": format!("{y}-01-01"), "end_date": format!("{y}-12-31")})).await;
    y
}

/// A new postable, active account of the given type.
pub async fn account(c: &mut Client, account_type: &str) -> i64 {
    create(c, "/api/accounts", serde_json::json!({"code": code("A"), "name": "Test", "account_type": account_type, "is_postable": true})).await
}

/// A party's accounts: its control account, its income or expense account,
/// and a tax account.
pub struct Ledger {
    pub control: i64,
    pub detail: i64,
    pub tax: i64,
}

/// A new customer (or, with `"suppliers"`, supplier) on a new organization,
/// with its control account set.
pub async fn party(c: &mut Client, collection: &str) -> (i64, Ledger) {
    let sales = collection == "customers";
    let ledger = Ledger {
        control: account(c, if sales { "asset" } else { "liability" }).await,
        detail: account(c, if sales { "revenue" } else { "expense" }).await,
        tax: account(c, "liability").await,
    };
    let org = create(c, "/api/organizations", serde_json::json!({"name": code("Org")})).await;
    let control = if sales { "ar_account_id" } else { "ap_account_id" };
    let id = create(c, &format!("/api/{collection}"), serde_json::json!({"organization_id": org, control: ledger.control})).await;
    (id, ledger)
}

/// A journal entry's lines as sorted (account, debit, credit, base debit,
/// base credit) tuples, with decimals normalized, for comparing as a multiset.
pub async fn entry_lines(c: &mut Client, entry: i64) -> Vec<(i64, String, String, String, String)> {
    let e = c.get(&format!("/api/journal-entries/{entry}")).await;
    assert_eq!(e.status, StatusCode::OK, "journal entry {entry}");
    let norm = |v: &Value| {
        let s = v.as_str().unwrap();
        let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.') } else { s };
        s.to_string()
    };
    let mut lines: Vec<_> = e.body["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| (l["account_id"].as_i64().unwrap(), norm(&l["debit"]), norm(&l["credit"]), norm(&l["base_debit"]), norm(&l["base_credit"])))
        .collect();
    lines.sort();
    lines
}

/// An expected journal line: (account, debit, credit), with base amounts equal.
pub fn line(account: i64, debit: &str, credit: &str) -> (i64, String, String, String, String) {
    (account, debit.into(), credit.into(), debit.into(), credit.into())
}
