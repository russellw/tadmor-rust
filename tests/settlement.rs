//! Payments, applying payments and credit notes, and realized exchange
//! differences (spec/api.md §5.9, domain §5 and §7.3).

mod common;

use axum::http::StatusCode;
use common::{Client, code, create, entry_lines, line};
use serde_json::{Value, json};

const CONFLICT: StatusCode = StatusCode::CONFLICT;
const NO_CONTENT: StatusCode = StatusCode::NO_CONTENT;
const OK: StatusCode = StatusCode::OK;
const UNPROCESSABLE: StatusCode = StatusCode::UNPROCESSABLE_ENTITY;

async fn post(c: &mut Client, collection: &str, id: i64) -> i64 {
    let r = c.send("POST", &format!("/api/{collection}/{id}/post"), None).await;
    assert_eq!(r.status, OK, "posting {collection} {id}: {}", r.body);
    r.body["journal_entry_id"].as_i64().unwrap()
}

async fn apply(c: &mut Client, collection: &str, id: i64) -> Vec<(i64, String)> {
    let r = c.send("POST", &format!("/api/{collection}/{id}/apply"), None).await;
    assert_eq!(r.status, OK, "applying {collection} {id}: {}", r.body);
    r.body["applications"].as_array().unwrap().iter().map(|a| (a["document_id"].as_i64().unwrap(), a["amount_applied"].as_str().unwrap().to_string())).collect()
}

/// A posted invoice (or bill) for `amount` in `currency`.
async fn posted(c: &mut Client, collection: &str, party: i64, date: String, currency: &str, amount: &str, account: i64) -> i64 {
    let sales = collection == "sales-invoices";
    let body = if sales {
        json!({"invoice_number": code("INV"), "customer_id": party, "invoice_date": date, "currency_code": currency,
               "lines": [{"description": "x", "unit_price": amount, "revenue_account_id": account}]})
    } else {
        json!({"bill_number": code("BILL"), "supplier_id": party, "bill_date": date, "currency_code": currency,
               "lines": [{"description": "x", "unit_cost": amount, "expense_account_id": account}]})
    };
    let id = create(c, &format!("/api/{collection}"), body).await;
    post(c, collection, id).await;
    id
}

fn decimal(v: &Value) -> String {
    let s = v.as_str().unwrap();
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[tokio::test]
async fn customer_payments_apply_oldest_first() {
    let mut c = common::admin().await;
    let y = common::open_year(&mut c).await;
    let (cust, l) = common::party(&mut c, "customers").await;
    let bank = common::account(&mut c, "asset").await;
    let inv1 = posted(&mut c, "sales-invoices", cust, format!("{y}-01-10"), "USD", "100", l.detail).await;
    let inv2 = posted(&mut c, "sales-invoices", cust, format!("{y}-02-10"), "USD", "80", l.detail).await;
    let inv3 = posted(&mut c, "sales-invoices", cust, format!("{y}-03-10"), "USD", "50", l.detail).await;

    let payment = |amount: &str, extra: Value| {
        let mut b = json!({"customer_id": cust, "payment_date": format!("{y}-04-01"), "currency_code": "USD", "amount": amount});
        b.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        b
    };
    assert_eq!(c.post("/api/customer-payments", json!({"customer_id": cust, "payment_date": format!("{y}-04-01"), "currency_code": "USD"})).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(c.post("/api/customer-payments", payment("0", json!({}))).await.status, UNPROCESSABLE);
    assert_eq!(c.post("/api/customer-payments", payment("5", json!({"method": "barter"}))).await.status, UNPROCESSABLE);
    let no_bank = create(&mut c, "/api/customer-payments", payment("5", json!({}))).await;
    assert_eq!(c.send("POST", &format!("/api/customer-payments/{no_bank}/post"), None).await.status, UNPROCESSABLE);

    let pay = create(&mut c, "/api/customer-payments", payment("150", json!({"method": "transfer", "deposit_account_id": bank}))).await;
    let p = c.get(&format!("/api/customer-payments/{pay}")).await.body;
    assert_eq!((decimal(&p["unapplied"]), p["status"].clone(), p["method"].clone()), ("150".into(), json!("draft"), json!("transfer")));
    assert_eq!(c.send("POST", &format!("/api/customer-payments/{pay}/apply"), None).await.status, CONFLICT, "not posted yet");
    let entry = post(&mut c, "customer-payments", pay).await;
    let mut want = vec![line(bank, "150", "0"), line(l.control, "0", "150")];
    want.sort();
    assert_eq!(entry_lines(&mut c, entry).await, want);
    assert_eq!(c.put(&format!("/api/customer-payments/{pay}"), payment("1", json!({}))).await.status, CONFLICT);

    let made = apply(&mut c, "customer-payments", pay).await;
    assert_eq!(made, [(inv1, "100.0000".into()), (inv2, "50.0000".into())]);
    let status = |v: &Value| (v["payment_status"].as_str().unwrap().to_string(), decimal(&v["balance"]));
    assert_eq!(status(&c.get(&format!("/api/sales-invoices/{inv1}")).await.body), ("paid".into(), "0".into()));
    assert_eq!(status(&c.get(&format!("/api/sales-invoices/{inv2}")).await.body), ("partial".into(), "30".into()));
    assert_eq!(status(&c.get(&format!("/api/sales-invoices/{inv3}")).await.body), ("unpaid".into(), "50".into()));
    assert_eq!(decimal(&c.get(&format!("/api/customer-payments/{pay}")).await.body["unapplied"]), "0");
    let listed = c.get(&format!("/api/customer-payments/{pay}/applications")).await.body;
    assert_eq!(listed[0]["document_id"], inv1);
    assert!(listed[0]["document_number"].is_string());
    assert!(apply(&mut c, "customer-payments", pay).await.is_empty(), "nothing left to apply");
    assert_eq!(c.send("POST", &format!("/api/sales-invoices/{inv1}/unpost"), None).await.status, CONFLICT);

    let pay2 = create(&mut c, "/api/customer-payments", payment("100", json!({"deposit_account_id": bank}))).await;
    post(&mut c, "customer-payments", pay2).await;
    assert_eq!(apply(&mut c, "customer-payments", pay2).await, [(inv2, "30.0000".into()), (inv3, "50.0000".into())]);
    assert_eq!(decimal(&c.get(&format!("/api/customer-payments/{pay2}")).await.body["unapplied"]), "20");

    // Unposting a payment deletes its applications and reopens the invoices.
    let r = c.send("POST", &format!("/api/customer-payments/{pay}/unpost"), None).await;
    assert_eq!(r.status, OK);
    assert_eq!(c.get(&format!("/api/customer-payments/{pay}/applications")).await.body, json!([]));
    assert_eq!(status(&c.get(&format!("/api/sales-invoices/{inv1}")).await.body), ("unpaid".into(), "100".into()));
    assert_eq!(c.send("POST", &format!("/api/sales-invoices/{inv1}/unpost"), None).await.status, OK);
    assert_eq!(c.send("DELETE", &format!("/api/customer-payments/{pay}"), None).await.status, NO_CONTENT);
}

#[tokio::test]
async fn credit_notes_share_an_invoices_availability() {
    let mut c = common::admin().await;
    let y = common::open_year(&mut c).await;
    let (cust, l) = common::party(&mut c, "customers").await;
    let bank = common::account(&mut c, "asset").await;
    let inv = posted(&mut c, "sales-invoices", cust, format!("{y}-01-10"), "USD", "200", l.detail).await;
    let note = create(&mut c, "/api/sales-credit-notes", json!({"credit_note_number": code("CN"), "customer_id": cust, "credit_note_date": format!("{y}-02-01"),
        "currency_code": "USD", "lines": [{"description": "Return", "unit_price": "50", "revenue_account_id": l.detail}]})).await;
    assert_eq!(c.send("POST", &format!("/api/sales-credit-notes/{note}/apply"), None).await.status, CONFLICT);
    post(&mut c, "sales-credit-notes", note).await;
    assert_eq!(apply(&mut c, "sales-credit-notes", note).await, [(inv, "50.0000".into())]);
    assert_eq!(c.get(&format!("/api/sales-credit-notes/{note}")).await.body["application_status"], "applied");

    let pay = create(&mut c, "/api/customer-payments", json!({"customer_id": cust, "payment_date": format!("{y}-03-01"), "currency_code": "USD", "amount": "500", "deposit_account_id": bank})).await;
    post(&mut c, "customer-payments", pay).await;
    assert_eq!(apply(&mut c, "customer-payments", pay).await, [(inv, "150.0000".into())]);
    assert_eq!(c.get(&format!("/api/sales-invoices/{inv}")).await.body["payment_status"], "paid");
    assert_eq!(c.send("POST", &format!("/api/sales-credit-notes/{note}/unpost"), None).await.status, CONFLICT, "an applied note");

    // The supplier side: a purchase credit note reduces a bill.
    let (sup, sl) = common::party(&mut c, "suppliers").await;
    let bill = posted(&mut c, "purchase-bills", sup, format!("{y}-01-20"), "USD", "100", sl.detail).await;
    let pnote = create(&mut c, "/api/purchase-credit-notes", json!({"credit_note_number": code("PCN"), "supplier_id": sup, "credit_note_date": format!("{y}-02-20"),
        "currency_code": "USD", "lines": [{"description": "Damaged", "unit_cost": "22", "expense_account_id": sl.detail}]})).await;
    post(&mut c, "purchase-credit-notes", pnote).await;
    assert_eq!(apply(&mut c, "purchase-credit-notes", pnote).await, [(bill, "22.0000".into())]);
    assert_eq!(decimal(&c.get(&format!("/api/purchase-bills/{bill}")).await.body["balance"]), "78");
    assert_eq!(c.get("/api/purchase-credit-notes/999999/applications").await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn settling_at_another_rate_realizes_an_exchange_difference() {
    let mut c = common::admin().await;
    let y = common::open_year(&mut c).await;
    for (date, rate) in [("01-01", "1.10"), ("03-01", "1.20")] {
        assert_eq!(c.post("/api/exchange-rates", json!({"currency_code": "EUR", "rate_date": format!("{y}-{date}"), "rate": rate})).await.status, StatusCode::CREATED);
    }
    let fx = c.get("/api/settings").await.body["fx_gain_loss_account_id"].as_i64().unwrap();
    let bank = common::account(&mut c, "asset").await;
    let (cust, l) = common::party(&mut c, "customers").await;
    let inv = posted(&mut c, "sales-invoices", cust, format!("{y}-01-15"), "EUR", "100", l.detail).await; // 110 USD
    let pay = create(&mut c, "/api/customer-payments", json!({"customer_id": cust, "payment_date": format!("{y}-03-10"), "currency_code": "EUR", "amount": "100", "deposit_account_id": bank})).await;
    post(&mut c, "customer-payments", pay).await; // 120 USD

    // Without an FX account, the apply is refused and makes nothing.
    assert_eq!(c.put("/api/settings", json!({"base_currency": "USD", "fx_gain_loss_account_id": null})).await.status, NO_CONTENT);
    let refused = c.send("POST", &format!("/api/customer-payments/{pay}/apply"), None).await.status;
    assert_eq!(c.put("/api/settings", json!({"base_currency": "USD", "fx_gain_loss_account_id": fx})).await.status, NO_CONTENT);
    assert_eq!(refused, UNPROCESSABLE);
    assert_eq!(c.get(&format!("/api/customer-payments/{pay}/applications")).await.body, json!([]));

    assert_eq!(apply(&mut c, "customer-payments", pay).await, [(inv, "100.0000".into())]);
    let ar = |tb: Value| decimal(&tb.as_array().unwrap().iter().find(|r| r["account_id"] == l.control).unwrap()["balance"]);
    // 110 − 120 + a gain of 10 that trues A/R up: settled in base.
    assert_eq!(ar(c.get("/api/trial-balance").await.body), "0");
    let gain = c.get(&format!("/api/accounts/{fx}/ledger?from={y}-03-10&to={y}-03-10")).await.body;
    let gain_entry = gain.as_array().unwrap().iter().find(|r| decimal(&r["base_credit"]) == "10").expect("an FX gain").clone();
    let mut want = vec![line(l.control, "10", "0"), line(fx, "0", "10")];
    want.sort();
    assert_eq!(entry_lines(&mut c, gain_entry["journal_entry_id"].as_i64().unwrap()).await, want);

    // Unposting the payment reverses the FX entry too.
    assert_eq!(c.send("POST", &format!("/api/customer-payments/{pay}/unpost"), None).await.status, OK);
    assert_eq!(ar(c.get("/api/trial-balance").await.body), "110");

    // The supplier side: a bill at 1.10 paid at 1.20 is a loss.
    let (sup, sl) = common::party(&mut c, "suppliers").await;
    posted(&mut c, "purchase-bills", sup, format!("{y}-01-20"), "EUR", "100", sl.detail).await;
    let sp = create(&mut c, "/api/supplier-payments", json!({"supplier_id": sup, "payment_date": format!("{y}-03-15"), "currency_code": "EUR", "amount": "100", "payment_account_id": bank})).await;
    post(&mut c, "supplier-payments", sp).await;
    apply(&mut c, "supplier-payments", sp).await;
    let ap = c.get("/api/trial-balance").await.body;
    assert_eq!(decimal(&ap.as_array().unwrap().iter().find(|r| r["account_id"] == sl.control).unwrap()["balance"]), "0");
    let loss = c.get(&format!("/api/accounts/{fx}/ledger?from={y}-03-15&to={y}-03-15")).await.body;
    assert!(loss.as_array().unwrap().iter().any(|r| decimal(&r["base_debit"]) == "10"), "an FX loss: {loss}");
}
