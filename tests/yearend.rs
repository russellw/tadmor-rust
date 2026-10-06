//! Year-end close and reopen (domain §9.3) and the financial statements
//! (domain §10). Closing needs every earlier year closed, so the year-end
//! test uses 1901 to 1903, before any year the other tests here create.

mod common;

use axum::http::StatusCode;
use common::{Client, code, create, entry_lines, line};
use serde_json::{Value, json};

const OK: StatusCode = StatusCode::OK;
const UNPROCESSABLE: StatusCode = StatusCode::UNPROCESSABLE_ENTITY;

async fn posted_invoice(c: &mut Client, collection: &str, party: i64, date: &str, amount: &str, account: i64) -> i64 {
    let body = if collection == "sales-invoices" {
        json!({"invoice_number": code("INV"), "customer_id": party, "invoice_date": date, "currency_code": "USD",
               "lines": [{"description": "x", "unit_price": amount, "revenue_account_id": account}]})
    } else {
        json!({"bill_number": code("BILL"), "supplier_id": party, "bill_date": date, "currency_code": "USD",
               "lines": [{"description": "x", "unit_cost": amount, "expense_account_id": account}]})
    };
    let id = create(c, &format!("/api/{collection}"), body).await;
    let r = c.send("POST", &format!("/api/{collection}/{id}/post"), None).await;
    assert_eq!(r.status, OK, "{}", r.body);
    id
}

fn decimal(v: &Value) -> String {
    let s = v.as_str().unwrap();
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s.to_string() }
}

fn amount_of(rows: &Value, account: i64) -> String {
    decimal(&rows.as_array().unwrap().iter().find(|r| r["account_id"] == account).unwrap_or_else(|| panic!("account {account} in {rows}"))["amount"])
}

#[tokio::test]
async fn closing_sweeps_income_into_retained_earnings_and_reopening_undoes_it() {
    let mut c = common::admin().await;
    let (cust, cl) = common::party(&mut c, "customers").await;
    let (sup, sl) = common::party(&mut c, "suppliers").await;
    let re = common::account(&mut c, "equity").await;
    let fy1 = create(&mut c, "/api/fiscal-years", json!({"name": code("Y1"), "start_date": "1901-01-01", "end_date": "1901-12-31"})).await;
    posted_invoice(&mut c, "sales-invoices", cust, "1901-06-15", "1000", cl.detail).await;
    posted_invoice(&mut c, "purchase-bills", sup, "1901-07-10", "400", sl.detail).await;

    let close = |id: i64| format!("/api/fiscal-years/{id}/close");
    let asset = common::account(&mut c, "asset").await;
    assert_eq!(c.post(&close(fy1), json!({"retained_earnings_account_id": asset})).await.status, UNPROCESSABLE);
    assert_eq!(c.post(&close(fy1), json!({})).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(c.post(&close(999999), json!({"retained_earnings_account_id": re})).await.status, StatusCode::NOT_FOUND);
    assert_eq!(c.send("POST", &format!("/api/fiscal-years/{fy1}/reopen"), None).await.status, StatusCode::CONFLICT);

    let r = c.post(&close(fy1), json!({"retained_earnings_account_id": re})).await;
    assert_eq!(r.status, OK, "{}", r.body);
    let (closing, fy2) = (r.body["closing_entry_id"].as_i64().unwrap(), r.body["next_fiscal_year_id"].as_i64().unwrap());
    let mut want = vec![line(cl.detail, "1000", "0"), line(sl.detail, "0", "400"), line(re, "0", "600")];
    want.sort();
    assert_eq!(entry_lines(&mut c, closing).await, want);
    let next = c.get(&format!("/api/fiscal-years/{fy2}")).await.body;
    assert_eq!((next["name"].clone(), next["start_date"].clone(), next["end_date"].clone()), (json!("FY1902"), json!("1902-01-01"), json!("1902-12-31")));
    assert_eq!(c.get(&format!("/api/fiscal-years/{fy1}")).await.body["status"], "closed");
    let periods = c.get("/api/accounting-periods").await.body;
    assert!(periods.as_array().unwrap().iter().filter(|p| p["fiscal_year_id"] == fy1).all(|p| p["status"] == "closed"));
    assert_eq!(c.post(&close(fy1), json!({"retained_earnings_account_id": re})).await.status, StatusCode::CONFLICT);

    // The income statement ignores the closing entry; the balance sheet does not.
    let pl = c.get("/api/profit-and-loss?from=1901-01-01&to=1901-12-31").await.body;
    assert_eq!((amount_of(&pl, cl.detail), amount_of(&pl, sl.detail)), ("1000".into(), "400".into()));
    let bs = c.get("/api/balance-sheet?as_of=1901-12-31").await.body;
    assert_eq!((amount_of(&bs["rows"], re), decimal(&bs["current_earnings"])), ("600".into(), "0".into()));

    // Nothing to sweep in 1902, and 1903 follows.
    let r = c.post(&close(fy2), json!({"retained_earnings_account_id": re})).await;
    assert_eq!(r.body["closing_entry_id"], Value::Null);
    let fy3 = r.body["next_fiscal_year_id"].as_i64().unwrap();
    assert_eq!(c.get(&format!("/api/fiscal-years/{fy3}")).await.body["name"], "FY1903");

    // Reopen newest first.
    let reopen = |id: i64| format!("/api/fiscal-years/{id}/reopen");
    assert_eq!(c.send("POST", &reopen(fy1), None).await.status, UNPROCESSABLE);
    assert_eq!(c.send("POST", &reopen(fy2), None).await.body, json!({"reversal_entry_id": null}));
    let reversal = c.send("POST", &reopen(fy1), None).await.body["reversal_entry_id"].as_i64().unwrap();
    let mut want = vec![line(cl.detail, "0", "1000"), line(sl.detail, "400", "0"), line(re, "600", "0")];
    want.sort();
    assert_eq!(entry_lines(&mut c, reversal).await, want);
    assert_eq!(c.post(&close(fy2), json!({"retained_earnings_account_id": re})).await.status, UNPROCESSABLE, "1901 is open again");
    let bs = c.get("/api/balance-sheet?as_of=1901-12-31").await.body;
    assert_eq!((amount_of(&bs["rows"], re), decimal(&bs["current_earnings"])), ("0".into(), "600".into()));
    // The reopened period takes postings again.
    posted_invoice(&mut c, "sales-invoices", cust, "1901-12-20", "5", cl.detail).await;
}

#[tokio::test]
async fn statements_satisfy_their_identities() {
    let mut c = common::admin().await;
    let y = common::open_year(&mut c).await;
    let (cust, l) = common::party(&mut c, "customers").await;
    let bank = create(&mut c, "/api/accounts", json!({"code": code("A"), "name": "Bank", "account_type": "asset", "is_postable": true, "is_cash": true})).await;
    posted_invoice(&mut c, "sales-invoices", cust, &format!("{y}-02-01"), "300", l.detail).await;
    let pay = create(&mut c, "/api/customer-payments", json!({"customer_id": cust, "payment_date": format!("{y}-03-01"), "currency_code": "USD", "amount": "120", "deposit_account_id": bank})).await;
    assert_eq!(c.send("POST", &format!("/api/customer-payments/{pay}/post"), None).await.status, OK);

    let from_to = format!("from={y}-01-01&to={y}-12-31");
    let pl = c.get(&format!("/api/profit-and-loss?{from_to}")).await.body;
    assert_eq!(amount_of(&pl, l.detail), "300");
    let cf = c.get(&format!("/api/cash-flow?{from_to}")).await.body;
    // A/R grew by 180, a use of cash; cash rose by 120.
    assert_eq!(amount_of(&cf["rows"], l.control), "-180");
    assert_eq!(cf["rows"].as_array().unwrap().iter().find(|r| r["account_id"] == l.control).unwrap()["activity"], "operating");
    assert!(cf["rows"].as_array().unwrap().iter().all(|r| r["account_id"] != bank), "cash accounts are not rows");
    let bs = c.get(&format!("/api/balance-sheet?as_of={y}-12-31")).await.body;
    assert_eq!((amount_of(&bs["rows"], l.control), amount_of(&bs["rows"], bank)), ("180".into(), "120".into()));

    for (path, status) in [
        ("/api/profit-and-loss?from=someday", StatusCode::BAD_REQUEST),
        ("/api/cash-flow?to=2101-02-30", StatusCode::BAD_REQUEST),
        ("/api/balance-sheet?as_of=nope", StatusCode::BAD_REQUEST),
        ("/api/balance-sheet", OK),
        ("/api/inventory-valuation", OK),
    ] {
        assert_eq!(c.get(path).await.status, status, "{path}");
    }

    // Aging counts only applications, and the payment is unapplied, so the
    // whole invoice is outstanding; with no due date, it is not yet due.
    let aging = c.get("/api/ar-aging").await.body;
    let row = aging.as_array().unwrap().iter().find(|r| r["party_id"] == cust).expect("the customer in aging").clone();
    assert_eq!((decimal(&row["total_outstanding"]), decimal(&row["not_yet_due"])), ("300".into(), "300".into()));
    assert!(row["party_name"].is_string());
    assert_eq!(c.get("/api/ap-aging").await.status, OK);
}
