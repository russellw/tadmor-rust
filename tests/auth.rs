//! Sessions and user administration (spec/api.md §3 and §5.1).

mod common;

use axum::http::StatusCode;
use common::Client;
use serde_json::json;

#[tokio::test]
async fn the_api_needs_a_session() {
    let mut c = Client::new(common::app().await);
    for path in ["/api/auth/me", "/api/users", "/api/no-such-endpoint", "/api/accounts"] {
        let r = c.get(path).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{path}");
        assert!(r.body["error"].is_string(), "{path}: {}", r.body);
    }
    // Logout is idempotent and needs no session.
    assert_eq!(c.send("POST", "/api/auth/logout", None).await.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn login_me_logout() {
    let pool = common::pool().await;
    let email = common::email("login");
    let id = common::user(&pool, &email, "correct horse", false).await;
    let mut c = Client::new(common::app().await);

    for (e, p) in [(email.as_str(), ""), ("", "correct horse")] {
        assert_eq!(c.post("/api/auth/login", json!({"email": e, "password": p})).await.status, StatusCode::BAD_REQUEST);
    }
    assert_eq!(c.send("POST", "/api/auth/login", Some("{not json")).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(c.send("POST", "/api/auth/login", Some("")).await.status, StatusCode::BAD_REQUEST);
    for (e, p) in [(email.clone(), "wrong password"), (format!("nobody-{email}"), "correct horse")] {
        assert_eq!(c.post("/api/auth/login", json!({"email": e, "password": p})).await.status, StatusCode::UNAUTHORIZED);
    }

    // Emails are case-insensitive and trimmed.
    let r = c.login(&format!("  {} ", email.to_uppercase()), "correct horse").await;
    assert_eq!(r.body, json!({"id": id, "email": email, "full_name": "Test User", "is_admin": false}));
    let cookie = &r.set_cookie[0];
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Lax") && !cookie.contains("Secure"), "{cookie}");

    let me = c.get("/api/auth/me").await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.body["id"], id);
    // Authenticated, an unknown path or method under /api/ is a JSON 404.
    assert_eq!(c.get("/api/no-such-endpoint").await.status, StatusCode::NOT_FOUND);
    assert_eq!(c.send("DELETE", "/api/auth/me", None).await.status, StatusCode::NOT_FOUND);

    assert_eq!(c.send("POST", "/api/auth/logout", None).await.status, StatusCode::NO_CONTENT);
    assert!(c.cookie.is_none(), "logout clears the cookie");
    assert_eq!(c.get("/api/auth/me").await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_cookie_is_secure_behind_https() {
    let pool = common::pool().await;
    let email = common::email("https");
    common::user(&pool, &email, "correct horse", false).await;
    let app = common::app().await;
    let req = axum::http::Request::post("/api/auth/login")
        .header("x-forwarded-proto", "https")
        .body(axum::body::Body::from(json!({"email": email, "password": "correct horse"}).to_string()))
        .unwrap();
    let response = tower::ServiceExt::oneshot(app, req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers()["set-cookie"].to_str().unwrap().contains("; Secure"));
}

#[tokio::test]
async fn user_administration() {
    let pool = common::pool().await;
    let admin_email = common::email("admin");
    let admin_id = common::user(&pool, &admin_email, "admin password", true).await;
    let mut c = Client::new(common::app().await);
    c.login(&admin_email, "admin password").await;

    let email = common::email("ann");
    for (body, status) in [
        (json!({"email": "no-at-sign", "full_name": "X", "password": "longenough"}), StatusCode::UNPROCESSABLE_ENTITY),
        (json!({"email": email, "full_name": "", "password": "longenough"}), StatusCode::BAD_REQUEST),
        (json!({"email": email, "full_name": "X", "password": "short"}), StatusCode::UNPROCESSABLE_ENTITY),
        (json!({"email": email, "full_name": "X"}), StatusCode::BAD_REQUEST),
        (json!({"email": "", "full_name": "X", "password": "longenough"}), StatusCode::BAD_REQUEST),
    ] {
        assert_eq!(c.post("/api/users", body.clone()).await.status, status, "{body}");
    }

    let r = c.post("/api/users", json!({"email": email, "full_name": "Ann Example", "password": "longenough"})).await;
    assert_eq!(r.status, StatusCode::CREATED);
    let id = r.body["id"].as_i64().unwrap();
    let dup = json!({"email": email.to_uppercase(), "full_name": "Dup", "password": "longenough"});
    assert_eq!(c.post("/api/users", dup).await.status, StatusCode::CONFLICT);

    let u = c.get(&format!("/api/users/{id}")).await;
    assert_eq!(u.body, json!({"id": id, "email": email, "full_name": "Ann Example", "is_active": true, "is_admin": false}));
    for (path, status) in [
        ("/api/users/999999", StatusCode::NOT_FOUND),
        ("/api/users/99999999999", StatusCode::NOT_FOUND),
        ("/api/users/99999999999999999999999", StatusCode::BAD_REQUEST),
        ("/api/users/1.5", StatusCode::BAD_REQUEST),
        ("/api/users/0", StatusCode::BAD_REQUEST),
        ("/api/users/-1", StatusCode::BAD_REQUEST),
        ("/api/users/abc", StatusCode::BAD_REQUEST),
    ] {
        assert_eq!(c.get(path).await.status, status, "{path}");
    }

    let list = c.get("/api/users").await.body;
    let emails: Vec<&str> = list.as_array().unwrap().iter().map(|u| u["email"].as_str().unwrap()).collect();
    assert!(emails.contains(&email.as_str()) && emails.contains(&admin_email.as_str()));

    let rename = json!({"email": email, "full_name": "Ann Renamed", "is_active": true});
    assert_eq!(c.put(&format!("/api/users/{id}"), rename).await.status, StatusCode::NO_CONTENT);
    assert_eq!(c.get(&format!("/api/users/{id}")).await.body["full_name"], "Ann Renamed");
    let nameless = json!({"email": email, "full_name": "", "is_active": true});
    assert_eq!(c.put(&format!("/api/users/{id}"), nameless).await.status, StatusCode::BAD_REQUEST);
    let missing = json!({"email": "x@y.z", "full_name": "X", "is_active": true});
    assert_eq!(c.put("/api/users/999999", missing).await.status, StatusCode::NOT_FOUND);

    assert_eq!(c.post(&format!("/api/users/{id}/password"), json!({"password": "short"})).await.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(c.post(&format!("/api/users/{id}/password"), json!({})).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(c.post("/api/users/999999/password", json!({"password": "longenough"})).await.status, StatusCode::NOT_FOUND);

    // Administrators may not lock themselves out.
    for (active, admin) in [(false, true), (true, false)] {
        let me = json!({"email": admin_email, "full_name": "Test User", "is_active": active, "is_admin": admin});
        assert_eq!(c.put(&format!("/api/users/{admin_id}"), me).await.status, StatusCode::UNPROCESSABLE_ENTITY);
    }
}

#[tokio::test]
async fn sessions_end_and_roles_apply_at_once() {
    let pool = common::pool().await;
    let admin_email = common::email("admin");
    common::user(&pool, &admin_email, "admin password", true).await;
    let mut admin = Client::new(common::app().await);
    admin.login(&admin_email, "admin password").await;

    let email = common::email("roles");
    let id = common::user(&pool, &email, "first-password", false).await;
    let mut c = Client::new(common::app().await);
    c.login(&email, "first-password").await;
    let record = |active: bool, admin: bool| json!({"email": email, "full_name": "Role Tester", "is_active": active, "is_admin": admin});
    let user_path = format!("/api/users/{id}");

    // Ordinary users get 403 from the admin endpoints, before any body check.
    assert_eq!(c.get("/api/users").await.status, StatusCode::FORBIDDEN);
    assert_eq!(c.send("POST", "/api/users", Some("{not json")).await.status, StatusCode::FORBIDDEN);

    // Promotion and demotion take effect on the existing session.
    assert_eq!(admin.put(&user_path, record(true, true)).await.status, StatusCode::NO_CONTENT);
    assert_eq!(c.get("/api/users").await.status, StatusCode::OK);
    assert_eq!(admin.put(&user_path, record(true, false)).await.status, StatusCode::NO_CONTENT);
    assert_eq!(c.get("/api/users").await.status, StatusCode::FORBIDDEN);

    // A password reset revokes every session of that user.
    let reset = admin.post(&format!("{user_path}/password"), json!({"password": "second-password"})).await;
    assert_eq!(reset.status, StatusCode::NO_CONTENT);
    assert_eq!(c.get("/api/auth/me").await.status, StatusCode::UNAUTHORIZED);
    let old = c.post("/api/auth/login", json!({"email": email, "password": "first-password"})).await;
    assert_eq!(old.status, StatusCode::UNAUTHORIZED);
    c.login(&email, "second-password").await;

    // Deactivation ends sessions immediately and blocks login.
    assert_eq!(admin.put(&user_path, record(false, false)).await.status, StatusCode::NO_CONTENT);
    assert_eq!(c.get("/api/auth/me").await.status, StatusCode::UNAUTHORIZED);
    let blocked = c.post("/api/auth/login", json!({"email": email, "password": "second-password"})).await;
    assert_eq!(blocked.status, StatusCode::UNAUTHORIZED);
}
