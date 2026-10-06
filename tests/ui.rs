//! The user interface (spec/domain.md §13), driven as a browser would:
//! signing in, following links, posting forms with their tokens.

mod common;

use axum::http::StatusCode;
use common::{Browser, code};

#[tokio::test]
async fn signing_in_and_out() {
    let mut b = Browser { app: common::app().await, cookie: None, token: String::new() };
    // G1: without a session every page leads to the login screen, and back after.
    let r = b.get("/sales-invoices").await;
    assert_eq!((r.status, r.location.as_deref()), (StatusCode::SEE_OTHER, Some("/login?next=/sales-invoices")));
    let pool = common::pool().await;
    let email = common::email("ui");
    common::user(&pool, &email, "the password", false).await;
    let r = b.submit("/login", &[("email", &email), ("password", "wrong password"), ("next", "/sales-invoices")]).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(r.html.contains("invalid email or password"), "the error is shown");
    let r = b.submit("/login", &[("email", &email), ("password", "the password"), ("next", "//evil.example")]).await;
    assert_eq!(r.location.as_deref(), Some("/"), "only local paths");
    // G2: the user's name, and signing out.
    let home = b.get("/").await;
    assert!(home.html.contains("Test User") && home.html.contains("Sign out"));
    let r = b.submit("/logout", &[]).await;
    assert_eq!(r.location.as_deref(), Some("/login"));
    assert_eq!(b.get("/").await.status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn forms_need_their_token_and_pages_have_a_policy() {
    let mut b = Browser::admin().await;
    let r = b.submit("/organizations", &[("name", "Forged"), ("_token", "not-it")]).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let req = axum::http::Request::get("/").header("cookie", b.cookie.clone().unwrap()).body(axum::body::Body::empty()).unwrap();
    let response = tower::ServiceExt::oneshot(b.app.clone(), req).await.unwrap();
    let csp = response.headers()["content-security-policy"].to_str().unwrap();
    assert!(csp.contains("default-src 'self'") && !csp.contains("unsafe-inline"));
    // G8: an unknown address.
    assert_eq!(b.get("/no/such/page").await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn master_data_lists_and_forms() {
    let mut b = Browser::admin().await;
    // G3: the navigation reaches every list.
    let home = b.get("/").await;
    for path in ["/organizations", "/customers", "/suppliers", "/products", "/accounts", "/tax-codes", "/payment-terms", "/warehouses", "/settings", "/users"] {
        assert!(home.html.contains(&format!("href=\"{path}\"")), "{path} is linked");
        assert_eq!(b.get(path).await.status, StatusCode::OK, "{path}");
    }

    // M1: create, refuse with the server's message (G5), and edit.
    b.get("/organizations/new").await;
    let r = b.submit("/organizations", &[("name", "")]).await;
    assert!(r.html.contains("name is required"), "the refusal is shown");
    let name = code("Org");
    let r = b.submit("/organizations", &[("name", &name), ("country_code", "IE"), ("default_currency", "EUR")]).await;
    let list = b.follow(r).await;
    assert!(list.html.contains(&name));
    let at = list.html.find(&name).unwrap();
    let href = &list.html[list.html[..at].rfind("href=\"").unwrap() + 6..];
    let edit_path = href[..href.find('"').unwrap()].to_string();
    let form = b.get(&edit_path).await;
    assert!(form.html.contains("value=\"IE\" selected"));
    let r = b.submit(&edit_path, &[("name", &format!("{name} Renamed"))]).await;
    assert!(b.follow(r).await.html.contains(&format!("{name} Renamed")));

    // M2: the organization is read-only once a customer exists.
    let org_id = edit_path.rsplit('/').next().unwrap().to_string();
    b.get("/customers/new").await;
    let r = b.submit("/customers", &[("organization_id", &org_id), ("credit_limit", "-1")]).await;
    assert!(r.html.contains("class=\"error\""), "a negative credit limit is refused visibly");
    let r = b.submit("/customers", &[("organization_id", &org_id), ("credit_limit", "1000")]).await;
    let customers = b.follow(r).await;
    assert!(customers.html.contains("1,000.00") && customers.html.contains("active"));

    // M4: a parent picker never offers the account itself.
    let form = b.get("/accounts/1").await;
    let parent = &form.html[form.html.find("name=\"parent_id\"").unwrap()..];
    let parent = &parent[..parent.find("</select>").unwrap()];
    assert!(!parent.contains("value=\"1\""), "an account is not its own parent");
}

#[tokio::test]
async fn administrator_only_screens() {
    let pool = common::pool().await;
    let email = common::email("plain");
    common::user(&pool, &email, "plain password", false).await;
    let mut b = Browser::sign_in(&email, "plain password").await;
    // G4: no Users link, and the screen refuses; settings are read-only.
    let home = b.get("/").await;
    assert!(!home.html.contains("href=\"/users\""));
    assert_eq!(b.get("/users").await.status, StatusCode::FORBIDDEN);
    let settings = b.get("/settings").await;
    assert!(settings.html.contains("Only an administrator") && !settings.html.contains(">Save<"));
    assert_eq!(b.submit("/settings", &[("base_currency", "EUR")]).await.status, StatusCode::FORBIDDEN);

    // M7: an administrator cannot demote themselves, and sees why.
    let my_email = common::email("me-admin");
    let me = common::user(&pool, &my_email, "admin password", true).await;
    let mut admin = Browser::sign_in(&my_email, "admin password").await;
    let path = format!("/users/{me}");
    assert!(admin.get("/users").await.html.contains(&format!("href=\"{path}\"")));
    admin.get(&path).await;
    let r = admin.submit(&path, &[("email", &my_email), ("full_name", "Test User"), ("is_active", "true")]).await;
    assert!(r.html.contains("you cannot remove your own administrator access"));
}
