//! Bank reconciliation (spec/api.md §5.13, domain §8).

mod common;

use axum::http::StatusCode;
use common::{code, create};
use serde_json::{Value, json};

const CONFLICT: StatusCode = StatusCode::CONFLICT;
const NO_CONTENT: StatusCode = StatusCode::NO_CONTENT;
const OK: StatusCode = StatusCode::OK;
const UNPROCESSABLE: StatusCode = StatusCode::UNPROCESSABLE_ENTITY;

fn decimal(v: &Value) -> String {
    let s = v.as_str().unwrap();
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s.to_string() }
}

#[tokio::test]
async fn a_statement_from_import_to_reconciliation() {
    let mut c = common::admin().await;
    let y = common::open_year(&mut c).await;
    let bank = create(&mut c, "/api/accounts", json!({"code": code("A"), "name": "Bank", "account_type": "asset", "is_postable": true, "is_cash": true})).await;
    let (cust, l) = common::party(&mut c, "customers").await;
    let (sup, _) = common::party(&mut c, "suppliers").await;
    let mut posted = Vec::new();
    for (collection, party, date, amount) in [("customer-payments", cust, "05-02", "100"), ("supplier-payments", sup, "05-03", "40"), ("customer-payments", cust, "05-20", "25")] {
        let (party_field, account_field) = if collection == "customer-payments" { ("customer_id", "deposit_account_id") } else { ("supplier_id", "payment_account_id") };
        let id = create(&mut c, &format!("/api/{collection}"), json!({party_field: party, "payment_date": format!("{y}-{date}"), "currency_code": "USD", "amount": amount, account_field: bank})).await;
        assert_eq!(c.send("POST", &format!("/api/{collection}/{id}/post"), None).await.status, OK);
        posted.push(id);
    }

    assert_eq!(c.post("/api/bank-statements", json!({"account_id": l.control, "statement_date": format!("{y}-05-31"), "opening_balance": "0", "closing_balance": "0"})).await.status, UNPROCESSABLE, "not a cash account");
    let st = create(&mut c, "/api/bank-statements", json!({"account_id": bank, "statement_date": format!("{y}-05-31"), "opening_balance": "0", "closing_balance": "85", "reference": "MAY"})).await;
    let s = c.get(&format!("/api/bank-statements/{st}")).await.body;
    assert_eq!((s["status"].clone(), s["line_count"].clone(), decimal(&s["difference"])), (json!("open"), json!(0), "-85".into()));
    assert_eq!(c.post(&format!("/api/bank-statements/{st}/lines"), json!({"txn_date": format!("{y}-05-02"), "description": "Nothing", "amount": "0"})).await.status, UNPROCESSABLE);
    let first = create(&mut c, &format!("/api/bank-statements/{st}/lines"), json!({"txn_date": format!("{y}-05-02"), "description": "Deposit", "amount": "100"})).await;

    assert_eq!(c.post(&format!("/api/bank-statements/{st}/import"), json!({"csv": ""})).await.status, StatusCode::BAD_REQUEST);
    let bad = format!("{y}-05-04,Ok,1\nnot-a-date,Bad,1\n");
    assert_eq!(c.post(&format!("/api/bank-statements/{st}/import"), json!({"csv": bad})).await.status, UNPROCESSABLE);
    assert_eq!(c.get(&format!("/api/bank-statements/{st}")).await.body["line_count"], 1, "a bad import adds nothing");
    let csv = format!("date,description,amount,reference\n{y}-05-04,\"Cheque, to supplier\",-40,CHQ-1\n\n{y}-05-21,Deposit,25\n");
    let r = c.post(&format!("/api/bank-statements/{st}/import"), json!({"csv": csv})).await;
    assert_eq!(r.body, json!({"imported": 2}));
    let lines = c.get(&format!("/api/bank-statements/{st}/lines")).await.body;
    assert_eq!((lines[1]["description"].clone(), lines[1]["line_no"].clone(), decimal(&lines[1]["amount"])), (json!("Cheque, to supplier"), json!(2), "-40".into()));

    let candidates = c.get(&format!("/api/bank-statements/{st}/candidates")).await.body;
    assert_eq!(candidates.as_array().unwrap().len(), 3);
    let by_amount = |a: &str| candidates.as_array().unwrap().iter().find(|c| decimal(&c["amount"]) == a).unwrap()["journal_line_id"].as_i64().unwrap();
    let (jl100, jl40) = (by_amount("100"), by_amount("-40"));
    let matching = |line: i64| format!("/api/bank-statement-lines/{line}/match");
    assert_eq!(c.send("POST", &format!("/api/bank-statements/{st}/reconcile"), None).await.status, UNPROCESSABLE, "unmatched lines");
    assert_eq!(c.post(&matching(first), json!({})).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(c.post(&matching(first), json!({"journal_line_id": jl40})).await.status, UNPROCESSABLE, "a different amount");
    assert_eq!(c.post(&matching(first), json!({"journal_line_id": 999999})).await.status, UNPROCESSABLE, "no such journal line");
    assert_eq!(c.post(&matching(first), json!({"journal_line_id": jl100})).await.status, NO_CONTENT);
    assert_eq!(c.post(&matching(first), json!({"journal_line_id": jl100})).await.status, CONFLICT, "already matched");
    let other = create(&mut c, "/api/bank-statements", json!({"account_id": bank, "statement_date": format!("{y}-06-30"), "opening_balance": "0", "closing_balance": "0"})).await;
    let again = create(&mut c, &format!("/api/bank-statements/{other}/lines"), json!({"txn_date": format!("{y}-05-02"), "description": "Again", "amount": "100"})).await;
    assert_eq!(c.post(&matching(again), json!({"journal_line_id": jl100})).await.status, CONFLICT, "a journal line backs one statement line");
    assert_eq!(c.send("DELETE", &format!("/api/bank-statements/{other}"), None).await.status, NO_CONTENT);

    assert_eq!(c.send("POST", &format!("/api/bank-statements/{st}/auto-match"), None).await.body, json!({"matched": 2}));
    assert_eq!(c.get(&format!("/api/bank-statements/{st}/lines")).await.body[1]["journal_line_id"], jl40);
    assert_eq!(c.get(&format!("/api/bank-statements/{st}/candidates")).await.body, json!([]));
    assert_eq!(c.send("POST", &format!("/api/bank-statements/{st}/reconcile"), None).await.status, NO_CONTENT);

    // Reconciled: frozen, and so are the payments it matched.
    assert_eq!(c.post(&format!("/api/bank-statements/{st}/lines"), json!({"txn_date": format!("{y}-05-30"), "description": "Late", "amount": "1"})).await.status, CONFLICT);
    assert_eq!(c.send("POST", &format!("/api/bank-statement-lines/{first}/unmatch"), None).await.status, CONFLICT);
    assert_eq!(c.send("POST", &format!("/api/customer-payments/{}/unpost", posted[0]), None).await.status, CONFLICT);
    assert_eq!(c.send("POST", &format!("/api/bank-statements/{st}/reopen"), None).await.status, NO_CONTENT);
    assert_eq!(c.send("POST", &format!("/api/bank-statements/{st}/reopen"), None).await.status, CONFLICT);
    assert_eq!(c.send("POST", &format!("/api/bank-statement-lines/{first}/unmatch"), None).await.status, NO_CONTENT);
    assert_eq!(c.send("POST", &format!("/api/customer-payments/{}/unpost", posted[0]), None).await.status, OK);
    assert_eq!(c.send("DELETE", &format!("/api/bank-statement-lines/{first}"), None).await.status, NO_CONTENT);
    assert_eq!(c.get(&format!("/api/bank-statements/{st}")).await.body["line_count"], 2);
}
