//! Master data, the fiscal calendar, settings, and exchange rates
//! (spec/api.md §5.2 to §5.8).

mod common;

use axum::http::StatusCode;
use common::{code, create};
use serde_json::json;

const BAD_REQUEST: StatusCode = StatusCode::BAD_REQUEST;
const CONFLICT: StatusCode = StatusCode::CONFLICT;
const NO_CONTENT: StatusCode = StatusCode::NO_CONTENT;
const NOT_FOUND: StatusCode = StatusCode::NOT_FOUND;
const UNPROCESSABLE: StatusCode = StatusCode::UNPROCESSABLE_ENTITY;

#[tokio::test]
async fn the_seed_data_is_listed() {
    let mut c = common::admin().await;
    let accounts = c.get("/api/accounts").await.body;
    let cash = accounts.as_array().unwrap().iter().find(|a| a["code"] == "1000").unwrap();
    assert_eq!(cash["is_cash"], true);
    assert_eq!(cash["cash_flow_activity"], "operating");
    let terms = c.get("/api/payment-terms").await.body;
    // Other tests add terms of their own; the seeded ones keep their order.
    let seeded = ["DUE", "NET15", "NET30", "NET60"];
    let codes: Vec<&str> =
        terms.as_array().unwrap().iter().map(|t| t["code"].as_str().unwrap()).filter(|c| seeded.contains(c)).collect();
    assert_eq!(codes, seeded);
    let std = c.get("/api/tax-codes/STD").await.body;
    assert_eq!(std["rate"], "0.0000");
    let settings = c.get("/api/settings").await.body;
    assert_eq!(settings["base_currency"], "USD");
}

#[tokio::test]
async fn organizations_are_fully_replaced_and_one_is_self() {
    let mut c = common::admin().await;
    assert_eq!(c.post("/api/organizations", json!({"name": ""})).await.status, BAD_REQUEST);
    assert_eq!(c.post("/api/organizations", json!({"name": "X", "country_code": "ZZ"})).await.status, UNPROCESSABLE);
    let name = code("Org");
    let id = create(&mut c, "/api/organizations", json!({"name": name, "country_code": "IE", "default_currency": "EUR", "email": "a@b.c"})).await;
    let path = format!("/api/organizations/{id}");
    assert_eq!(c.put(&path, json!({"name": format!("{name} Renamed")})).await.status, NO_CONTENT);
    let o = c.get(&path).await.body;
    assert_eq!(o["country_code"], json!(null));
    assert_eq!(o["email"], json!(null));
    assert_eq!(c.put("/api/organizations/999999", json!({"name": "Nobody"})).await.status, NOT_FOUND);
    // The single self organization is a database invariant shared with
    // other tests, so only check that a second one is refused.
    let first = c.post("/api/organizations", json!({"name": code("Self"), "is_self": true})).await.status;
    assert!(first == StatusCode::CREATED || first == CONFLICT);
    assert_eq!(c.post("/api/organizations", json!({"name": code("Self"), "is_self": true})).await.status, CONFLICT);
}

#[tokio::test]
async fn customers_and_suppliers_are_roles_on_an_organization() {
    let mut c = common::admin().await;
    let org = create(&mut c, "/api/organizations", json!({"name": code("Org")})).await;
    assert_eq!(c.post("/api/customers", json!({})).await.status, BAD_REQUEST);
    assert_eq!(c.post("/api/customers", json!({"organization_id": 999999})).await.status, UNPROCESSABLE);
    assert_eq!(c.post("/api/customers", json!({"organization_id": org, "credit_limit": "-1"})).await.status, UNPROCESSABLE);
    assert_eq!(c.post("/api/customers", json!({"organization_id": org, "credit_limit": "lots"})).await.status, UNPROCESSABLE);
    assert_eq!(c.post("/api/customers", json!({"organization_id": org, "ar_account_id": 99999999999i64})).await.status, UNPROCESSABLE);

    // Decimals round to their scale, half away from zero, before any check.
    let cust = create(&mut c, "/api/customers", json!({"organization_id": org, "credit_limit": "-0.00004", "is_active": false})).await;
    let got = c.get(&format!("/api/customers/{cust}")).await.body;
    assert_eq!(got["credit_limit"], "0.0000");
    assert_eq!(got["is_active"], true, "create ignores is_active");
    assert_eq!(c.post("/api/customers", json!({"organization_id": org})).await.status, CONFLICT);
    assert_eq!(c.put(&format!("/api/customers/{cust}"), json!({"organization_id": org, "credit_limit": "12.34565"})).await.status, NO_CONTENT);
    let got = c.get(&format!("/api/customers/{cust}")).await.body;
    assert_eq!(got["credit_limit"], "12.3457");
    assert_eq!(got["is_active"], false, "an omitted boolean is false");

    let sup = create(&mut c, "/api/suppliers", json!({"organization_id": org, "currency_code": "USD"})).await;
    assert_eq!(c.get(&format!("/api/suppliers/{sup}")).await.body["organization_id"], org);
    assert_eq!(c.post("/api/suppliers", json!({"organization_id": 0})).await.status, BAD_REQUEST);
}

#[tokio::test]
async fn products_and_accounts() {
    let mut c = common::admin().await;
    let sku = code("SKU");
    let id = create(&mut c, "/api/products", json!({"sku": sku, "name": "Widget"})).await;
    assert_eq!(c.get(&format!("/api/products/{id}")).await.body["unit_price"], "0.0000");
    assert_eq!(c.post("/api/products", json!({"sku": sku, "name": "Dup"})).await.status, CONFLICT);
    assert_eq!(c.post("/api/products", json!({"sku": code("SKU"), "name": "X", "unit_price": "twelve"})).await.status, UNPROCESSABLE);
    assert_eq!(c.post("/api/products", json!({"sku": code("SKU"), "name": "X", "unit_price": "1000000000000000"})).await.status, UNPROCESSABLE);
    assert_eq!(c.post("/api/products", json!({"sku": code("SKU"), "name": "X", "unit_price": 12})).await.status, BAD_REQUEST);

    let header = create(&mut c, "/api/accounts", json!({"code": code("A"), "name": "Header", "account_type": "asset"})).await;
    let a = c.get(&format!("/api/accounts/{header}")).await.body;
    assert_eq!((a["is_postable"].clone(), a["cash_flow_activity"].clone()), (json!(false), json!("operating")));
    for bad in [
        json!({"code": code("A"), "name": "X", "account_type": "bogus"}),
        json!({"code": code("A"), "name": "X", "account_type": "liability", "is_cash": true}),
        json!({"code": code("A"), "name": "X", "account_type": "asset", "cash_flow_activity": "sideways"}),
        json!({"code": code("A"), "name": "X", "account_type": "asset", "parent_id": 999999}),
    ] {
        assert_eq!(c.post("/api/accounts", bad.clone()).await.status, UNPROCESSABLE, "{bad}");
    }
    let self_parent = json!({"code": a["code"], "name": "Loop", "account_type": "asset", "parent_id": header});
    assert_eq!(c.put(&format!("/api/accounts/{header}"), self_parent).await.status, UNPROCESSABLE);

    assert_eq!(c.get(&format!("/api/accounts/{header}/ledger")).await.body, json!([]));
    assert_eq!(c.get(&format!("/api/accounts/{header}/ledger?from=2101-01-01&to=")).await.status, StatusCode::OK);
    assert_eq!(c.get(&format!("/api/accounts/{header}/ledger?from=yesterday")).await.status, BAD_REQUEST);
    assert_eq!(c.get(&format!("/api/accounts/{header}/ledger?to=2101-02-30")).await.status, BAD_REQUEST);
    assert_eq!(c.get("/api/accounts/999999/ledger").await.status, NOT_FOUND);
    assert_eq!(c.get("/api/accounts/999999/ledger?from=bad").await.status, BAD_REQUEST, "400 comes before 404");
}

#[tokio::test]
async fn tax_codes_payment_terms_and_warehouses() {
    let mut c = common::admin().await;
    let tax = code("TX");
    let r = c.post("/api/tax-codes", json!({"code": tax, "name": "VAT", "rate": "20"})).await;
    assert_eq!((r.status, r.body.clone()), (StatusCode::CREATED, json!({"code": tax})));
    assert_eq!(c.post("/api/tax-codes", json!({"code": code("TX"), "name": "X", "rate": "-1"})).await.status, UNPROCESSABLE);
    let path = format!("/api/tax-codes/{tax}");
    assert_eq!(c.put(&path, json!({"code": "IGNORED", "name": "Reduced", "rate": "13.5", "is_active": true})).await.status, NO_CONTENT);
    assert_eq!(c.get(&path).await.body["rate"], "13.5000");
    assert_eq!(c.get("/api/tax-codes/IGNORED").await.status, NOT_FOUND);

    let term = code("PT");
    assert_eq!(c.post("/api/payment-terms", json!({"code": term, "name": "Bad", "due_days": -1})).await.status, UNPROCESSABLE);
    assert_eq!(c.post("/api/payment-terms", json!({"code": term, "name": "Net 45", "due_days": 45})).await.status, StatusCode::CREATED);
    assert_eq!(c.put(&format!("/api/payment-terms/{term}"), json!({"name": "Net 45", "due_days": -5})).await.status, UNPROCESSABLE);

    let wh = create(&mut c, "/api/warehouses", json!({"code": code("WH"), "name": "Main"})).await;
    let w = c.get(&format!("/api/warehouses/{wh}")).await.body;
    assert_eq!(c.put(&format!("/api/warehouses/{wh}"), json!({"code": w["code"], "name": "Closed"})).await.status, NO_CONTENT);
    assert_eq!(c.get(&format!("/api/warehouses/{wh}")).await.body["is_active"], false);
}

#[tokio::test]
async fn fiscal_years_and_periods() {
    let mut c = common::admin().await;
    // A year of its own, far from other tests, since periods never overlap.
    let y = 3000 + std::process::id() % 5000;
    let name = code("FY");
    let dates = |a: &str, b: &str| (format!("{y}-{a}"), format!("{y}-{b}"));
    let (start, end) = dates("01-01", "12-31");
    assert_eq!(c.post("/api/fiscal-years", json!({"name": name, "start_date": "", "end_date": end})).await.status, BAD_REQUEST);
    assert_eq!(c.post("/api/fiscal-years", json!({"name": name, "start_date": end, "end_date": start})).await.status, UNPROCESSABLE);
    assert_eq!(c.post("/api/fiscal-years", json!({"name": name, "start_date": format!("{y}-13-45"), "end_date": end})).await.status, UNPROCESSABLE);
    let fy = create(&mut c, "/api/fiscal-years", json!({"name": name, "start_date": start, "end_date": end})).await;
    let renamed = json!({"name": format!("{name}r"), "start_date": start, "end_date": end, "status": "closed"});
    assert_eq!(c.put(&format!("/api/fiscal-years/{fy}"), renamed).await.status, NO_CONTENT);
    assert_eq!(c.get(&format!("/api/fiscal-years/{fy}")).await.body["status"], "open", "status is not settable by PUT");

    let period = |n: &str, a: &str, b: &str| {
        let (s, e) = dates(a, b);
        json!({"fiscal_year_id": fy, "name": n, "start_date": s, "end_date": e})
    };
    let q1 = create(&mut c, "/api/accounting-periods", period("Q1", "01-01", "03-31")).await;
    assert_eq!(c.post("/api/accounting-periods", period("Q1", "04-01", "06-30")).await.status, CONFLICT);
    assert_eq!(c.post("/api/accounting-periods", period("Overlap", "03-31", "04-30")).await.status, UNPROCESSABLE);
    let mut closed = period("Q1", "01-01", "03-31");
    closed["status"] = json!("closed");
    assert_eq!(c.put(&format!("/api/accounting-periods/{q1}"), closed).await.status, NO_CONTENT);
    assert_eq!(c.get(&format!("/api/accounting-periods/{q1}")).await.body["status"], "closed");
    assert_eq!(c.put(&format!("/api/accounting-periods/{q1}"), period("Q1", "01-01", "03-31")).await.status, NO_CONTENT);
    assert_eq!(c.get(&format!("/api/accounting-periods/{q1}")).await.body["status"], "open");
}

#[tokio::test]
async fn settings_and_exchange_rates() {
    let mut c = common::admin().await;
    let fx = c.get("/api/settings").await.body["fx_gain_loss_account_id"].clone();
    let summary = create(&mut c, "/api/accounts", json!({"code": code("A"), "name": "Header", "account_type": "expense"})).await;
    assert_eq!(c.put("/api/settings", json!({"base_currency": "", "fx_gain_loss_account_id": fx})).await.status, BAD_REQUEST);
    assert_eq!(c.put("/api/settings", json!({"base_currency": "US", "fx_gain_loss_account_id": fx})).await.status, UNPROCESSABLE);
    assert_eq!(c.put("/api/settings", json!({"base_currency": "USD", "fx_gain_loss_account_id": summary})).await.status, UNPROCESSABLE);
    assert_eq!(c.put("/api/settings", json!({"base_currency": "usd", "fx_gain_loss_account_id": fx})).await.status, NO_CONTENT);

    // A date of this test process's own, so parallel runs never collide.
    let day = format!("1800-01-{:02}", 1 + std::process::id() % 28);
    let r = c.post("/api/exchange-rates", json!({"currency_code": "cad", "rate_date": day, "rate": "1.30"})).await;
    assert_eq!((r.status, r.body.clone()), (StatusCode::CREATED, json!({"currency_code": "CAD", "rate_date": day})));
    assert_eq!(c.post("/api/exchange-rates", json!({"currency_code": "CAD", "rate_date": day, "rate": "1.4"})).await.status, CONFLICT);
    assert_eq!(c.post("/api/exchange-rates", json!({"currency_code": "CAD", "rate_date": "1800-02-01", "rate": "0"})).await.status, UNPROCESSABLE);
    assert_eq!(c.post("/api/exchange-rates", json!({"currency_code": "CAD", "rate_date": "1800-02-01", "rate": ""})).await.status, BAD_REQUEST);
    let path = format!("/api/exchange-rates/cad/{day}");
    assert_eq!(c.put(&path, json!({"rate": "1.325"})).await.status, NO_CONTENT);
    let rates = c.get("/api/exchange-rates").await.body;
    let ours = rates.as_array().unwrap().iter().find(|r| r["rate_date"] == day.as_str()).unwrap().clone();
    assert_eq!(ours["rate"], "1.325", "trailing zeros are trimmed");
    assert_eq!(c.send("DELETE", &path, None).await.status, NO_CONTENT);
    assert_eq!(c.send("DELETE", &path, None).await.status, NOT_FOUND);
    assert_eq!(c.put("/api/exchange-rates/CAD/not-a-date", json!({"rate": "1"})).await.status, NOT_FOUND);
}

#[tokio::test]
async fn ordinary_users_cannot_change_settings() {
    let pool = common::pool().await;
    let email = common::email("plain");
    common::user(&pool, &email, "plain password", false).await;
    let mut c = common::Client::new(common::app().await);
    c.login(&email, "plain password").await;
    assert_eq!(c.get("/api/settings").await.status, StatusCode::OK);
    assert_eq!(c.put("/api/settings", json!({"base_currency": "USD"})).await.status, StatusCode::FORBIDDEN);
}
