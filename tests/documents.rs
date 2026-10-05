//! Invoices, bills and credit notes: drafts, line money, posting, and
//! unposting (spec/api.md §5.9, domain §2 and §4).

mod common;

use axum::http::StatusCode;
use common::{Client, code, create, entry_lines, line};
use serde_json::{Value, json};

const BAD_REQUEST: StatusCode = StatusCode::BAD_REQUEST;
const CONFLICT: StatusCode = StatusCode::CONFLICT;
const NO_CONTENT: StatusCode = StatusCode::NO_CONTENT;
const NOT_FOUND: StatusCode = StatusCode::NOT_FOUND;
const UNPROCESSABLE: StatusCode = StatusCode::UNPROCESSABLE_ENTITY;

async fn post(c: &mut Client, collection: &str, id: i64) -> i64 {
    let r = c.send("POST", &format!("/api/{collection}/{id}/post"), None).await;
    assert_eq!(r.status, StatusCode::OK, "posting {collection} {id}: {}", r.body);
    r.body["journal_entry_id"].as_i64().unwrap()
}

async fn status(c: &mut Client, method: &str, path: &str, body: Option<Value>) -> StatusCode {
    c.send(method, path, body.map(|b| b.to_string()).as_deref()).await.status
}

#[tokio::test]
async fn an_invoice_from_draft_to_posted_and_back() {
    let mut c = common::admin().await;
    let y = common::open_year(&mut c).await;
    let (cust, l) = common::party(&mut c, "customers").await;
    let tax = code("TX");
    assert_eq!(c.post("/api/tax-codes", json!({"code": tax, "name": "Tax", "tax_account_id": l.tax})).await.status, StatusCode::CREATED);
    let number = code("INV");
    let mut body = json!({
        "invoice_number": number, "customer_id": cust, "invoice_date": format!("{y}-03-15"), "due_date": format!("{y}-04-14"),
        "currency_code": "USD", "reference": "PO-77", "memo": "Thanks",
        "lines": [
            {"description": "Consulting", "quantity": "2", "unit_price": "10.005", "revenue_account_id": l.detail},
            {"description": "Licence", "quantity": "3", "unit_price": "7.333", "revenue_account_id": l.detail, "tax_code": tax, "tax_rate": "8.25"}
        ]
    });
    let with = |k: &str, v: Value| {
        let mut b = body.clone();
        b[k] = v;
        b
    };
    for (bad, want) in [
        (with("invoice_number", json!("")), BAD_REQUEST),
        (with("customer_id", json!(0)), BAD_REQUEST),
        (with("customer_id", json!("one")), BAD_REQUEST),
        (with("lines", json!([{"description": ""}])), BAD_REQUEST),
        (with("lines", json!("none")), BAD_REQUEST),
        (with("customer_id", json!(999999)), UNPROCESSABLE),
        (with("lines", json!([{"description": "Zero", "quantity": "0"}])), UNPROCESSABLE),
        (with("lines", json!([{"description": "Bad", "unit_price": "lots"}])), UNPROCESSABLE),
        (with("due_date", json!(format!("{y}-03-01"))), UNPROCESSABLE),
        (with("currency_code", json!("ZZZ")), UNPROCESSABLE),
    ] {
        assert_eq!(c.post("/api/sales-invoices", bad.clone()).await.status, want, "{bad}");
    }

    let id = create(&mut c, "/api/sales-invoices", body.clone()).await;
    assert_eq!(c.post("/api/sales-invoices", body.clone()).await.status, CONFLICT, "numbers are unique");
    let inv = c.get(&format!("/api/sales-invoices/{id}")).await.body;
    assert_eq!(inv["status"], "draft");
    assert_eq!(inv["payment_status"], "unpaid");
    assert_eq!(inv["total"], "43.8239"); // 20.01 + 21.999 + round(21.999 × 8.25%, 4)
    assert_eq!(inv["balance"], "43.8239");
    assert_eq!(inv["journal_entry_id"], Value::Null);
    let lines = c.get(&format!("/api/sales-invoices/{id}/lines")).await.body;
    assert_eq!(lines[1]["tax_amount"], "1.8149");
    assert_eq!(lines[1]["order_line_id"], Value::Null);

    body["lines"] = json!([
        {"description": "Consulting", "quantity": "4", "unit_price": "25", "revenue_account_id": l.detail},
        {"description": "Licence", "unit_price": "100", "revenue_account_id": l.detail, "tax_code": tax, "tax_rate": "10"}
    ]);
    body["memo"] = Value::Null;
    assert_eq!(c.put(&format!("/api/sales-invoices/{id}"), body.clone()).await.status, NO_CONTENT);
    let inv = c.get(&format!("/api/sales-invoices/{id}")).await.body;
    assert_eq!((inv["total"].clone(), inv["memo"].clone()), (json!("210.0000"), Value::Null));
    assert_eq!(c.put("/api/sales-invoices/999999", body.clone()).await.status, NOT_FOUND);

    // Posting creates the month's period, and the entry Dr A/R, Cr revenue and tax.
    let entry = post(&mut c, "sales-invoices", id).await;
    assert_eq!(entry_lines(&mut c, entry).await, {
        let mut want = vec![line(l.control, "210", "0"), line(l.detail, "0", "200"), line(l.tax, "0", "10")];
        want.sort();
        want
    });
    let e = c.get(&format!("/api/journal-entries/{entry}")).await.body;
    assert_eq!((e["reference"].clone(), e["exchange_rate"].clone(), e["entry_date"].clone()), (json!(number), json!("1"), json!(format!("{y}-03-15"))));
    let periods = c.get("/api/accounting-periods").await.body;
    let month = periods.as_array().unwrap().iter().find(|p| p["name"] == format!("{y}-03")).expect("the month's period");
    assert_eq!((month["start_date"].clone(), month["end_date"].clone()), (json!(format!("{y}-03-01")), json!(format!("{y}-03-31"))));
    assert_eq!(c.get(&format!("/api/sales-invoices/{id}")).await.body["journal_entry_id"], entry);
    for (method, path) in [("POST", format!("/api/sales-invoices/{id}/post")), ("PUT", format!("/api/sales-invoices/{id}")), ("DELETE", format!("/api/sales-invoices/{id}"))] {
        assert_eq!(status(&mut c, method, &path, (method == "PUT").then(|| body.clone())).await, CONFLICT, "{method} {path}");
    }

    // Unposting mirrors the entry and returns the invoice to draft.
    let r = c.send("POST", &format!("/api/sales-invoices/{id}/unpost"), None).await;
    assert_eq!(r.status, StatusCode::OK);
    let reversal = r.body["reversal_entry_id"].as_i64().unwrap();
    let mut want = vec![line(l.control, "0", "210"), line(l.detail, "200", "0"), line(l.tax, "10", "0")];
    want.sort();
    assert_eq!(entry_lines(&mut c, reversal).await, want);
    assert_eq!(c.get(&format!("/api/sales-invoices/{id}")).await.body["status"], "draft");
    assert_eq!(status(&mut c, "POST", &format!("/api/sales-invoices/{id}/unpost"), None).await, CONFLICT);
    let again = post(&mut c, "sales-invoices", id).await;
    assert!(again != entry && again != reversal);

    let tb = c.get("/api/trial-balance").await.body;
    let ar = tb.as_array().unwrap().iter().find(|r| r["account_id"] == l.control).unwrap();
    assert_eq!((ar["total_debit"].clone(), ar["total_credit"].clone(), ar["balance"].clone()), (json!("420.0000"), json!("210.0000"), json!("210.0000")));

    // Drafts are deleted outright.
    let draft = create(&mut c, "/api/sales-invoices", json!({"invoice_number": code("INV"), "customer_id": cust, "invoice_date": format!("{y}-05-01"), "currency_code": "USD"})).await;
    assert_eq!(status(&mut c, "DELETE", &format!("/api/sales-invoices/{draft}"), None).await, NO_CONTENT);
    assert_eq!(status(&mut c, "DELETE", &format!("/api/sales-invoices/{draft}"), None).await, NOT_FOUND);
    assert_eq!(c.get(&format!("/api/sales-invoices/{draft}/lines")).await.status, NOT_FOUND);
}

#[tokio::test]
async fn posting_refusals_in_order() {
    let mut c = common::admin().await;
    let y = common::open_year(&mut c).await;
    let (cust, l) = common::party(&mut c, "customers").await;
    let invoice = |customer: i64, date: String, lines: Value| {
        json!({"invoice_number": code("INV"), "customer_id": customer, "invoice_date": date, "currency_code": "USD", "lines": lines})
    };
    let five = json!([{"description": "x", "unit_price": "5", "revenue_account_id": l.detail}]);
    let try_post = async |c: &mut Client, body: Value| {
        let id = create(c, "/api/sales-invoices", body).await;
        status(c, "POST", &format!("/api/sales-invoices/{id}/post"), None).await
    };

    assert_eq!(status(&mut c, "POST", "/api/sales-invoices/999999/post", None).await, NOT_FOUND);
    assert_eq!(status(&mut c, "POST", "/api/sales-invoices/abc/post", None).await, BAD_REQUEST);
    let date = format!("{y}-02-10");
    assert_eq!(try_post(&mut c, invoice(cust, date.clone(), json!([]))).await, UNPROCESSABLE, "nothing to post");
    let refund = json!([{"description": "Refund", "quantity": "-1", "unit_price": "5", "revenue_account_id": l.detail}]);
    assert_eq!(try_post(&mut c, invoice(cust, date.clone(), refund)).await, UNPROCESSABLE, "a negative total");
    let org = create(&mut c, "/api/organizations", json!({"name": code("Org")})).await;
    let no_ar = create(&mut c, "/api/customers", json!({"organization_id": org})).await;
    assert_eq!(try_post(&mut c, invoice(no_ar, date.clone(), five.clone())).await, UNPROCESSABLE, "no A/R account");
    assert_eq!(try_post(&mut c, invoice(cust, date.clone(), json!([{"description": "x", "unit_price": "5"}]))).await, UNPROCESSABLE);
    let untaxable = json!([{"description": "x", "unit_price": "5", "revenue_account_id": l.detail, "tax_code": "ZERO", "tax_rate": "5"}]);
    assert_eq!(try_post(&mut c, invoice(cust, date.clone(), untaxable)).await, UNPROCESSABLE, "no tax account");
    assert_eq!(try_post(&mut c, invoice(cust, format!("{}-02-10", y + 5000), five.clone())).await, UNPROCESSABLE, "no fiscal year");
    assert_eq!(try_post(&mut c, invoice(cust, date.clone(), json!([{"description": "x", "unit_price": "5", "revenue_account_id": l.detail}]))).await, StatusCode::OK);

    // A product supplies the revenue account a line omits.
    let product = create(&mut c, "/api/products", json!({"sku": code("SKU"), "name": "P", "revenue_account_id": l.detail})).await;
    let id = create(&mut c, "/api/sales-invoices", invoice(cust, date.clone(), json!([{"description": "x", "product_id": product, "unit_price": "5"}]))).await;
    let entry = post(&mut c, "sales-invoices", id).await;
    let mut want = vec![line(l.control, "5", "0"), line(l.detail, "0", "5")];
    want.sort();
    assert_eq!(entry_lines(&mut c, entry).await, want);

    // Unposting into a period closed since posting is refused.
    let month = c.get("/api/accounting-periods").await.body.as_array().unwrap().iter().find(|p| p["name"] == format!("{y}-02")).unwrap().clone();
    let mut closed = month.clone();
    closed["status"] = json!("closed");
    assert_eq!(c.put(&format!("/api/accounting-periods/{}", month["id"]), closed).await.status, NO_CONTENT);
    assert_eq!(status(&mut c, "POST", &format!("/api/sales-invoices/{id}/unpost"), None).await, UNPROCESSABLE);
    assert_eq!(try_post(&mut c, invoice(cust, date, five)).await, UNPROCESSABLE, "a closed period");
}

#[tokio::test]
async fn foreign_currency_and_lines_netting_negative() {
    let mut c = common::admin().await;
    let y = common::open_year(&mut c).await;
    // Only this test uses JPY, so no other posting picks the rate up.
    assert_eq!(c.post("/api/exchange-rates", json!({"currency_code": "JPY", "rate_date": format!("{y}-01-01"), "rate": "0.00671234"})).await.status, StatusCode::CREATED);
    let (cust, l) = common::party(&mut c, "customers").await;
    let discount = common::account(&mut c, "revenue").await;
    let inv = create(&mut c, "/api/sales-invoices", json!({"invoice_number": code("INV"), "customer_id": cust, "invoice_date": format!("{y}-03-01"), "currency_code": "JPY",
        "lines": [{"description": "Goods", "unit_price": "10000", "revenue_account_id": l.detail}, {"description": "Discount", "unit_price": "-1500", "revenue_account_id": discount}]})).await;
    let entry = post(&mut c, "sales-invoices", inv).await;
    // 10000 × 0.00671234 = 67.1234 and 1500 × 0.00671234 = 10.06851 → 10.0685;
    // the discount nets negative, so it is debited, and A/R takes the net base.
    let mut want = vec![
        (l.control, "8500".into(), "0".into(), "57.0549".into(), "0".into()),
        (l.detail, "0".into(), "10000".into(), "0".into(), "67.1234".into()),
        (discount, "1500".into(), "0".into(), "10.0685".into(), "0".into()),
    ];
    want.sort();
    assert_eq!(entry_lines(&mut c, entry).await, want);
    assert_eq!(c.get(&format!("/api/journal-entries/{entry}")).await.body["exchange_rate"], "0.00671234");

    // No rate on or before the date: 422.
    let early = create(&mut c, "/api/sales-invoices", json!({"invoice_number": code("INV"), "customer_id": cust, "invoice_date": format!("{}-12-31", y - 1), "currency_code": "JPY",
        "lines": [{"description": "x", "unit_price": "1", "revenue_account_id": l.detail}]})).await;
    common::create(&mut c, "/api/fiscal-years", json!({"name": code("FY"), "start_date": format!("{}-12-01", y - 1), "end_date": format!("{}-12-31", y - 1)})).await;
    assert_eq!(status(&mut c, "POST", &format!("/api/sales-invoices/{early}/post"), None).await, UNPROCESSABLE);
}

#[tokio::test]
async fn bills_and_credit_notes_post_to_their_own_sides() {
    let mut c = common::admin().await;
    let y = common::open_year(&mut c).await;
    let (sup, sl) = common::party(&mut c, "suppliers").await;
    let inventory = common::account(&mut c, "asset").await;
    let stocked = create(&mut c, "/api/products", json!({"sku": code("SKU"), "name": "Stocked", "inventory_account_id": inventory})).await;
    let tax = code("TX");
    assert_eq!(c.post("/api/tax-codes", json!({"code": tax, "name": "Tax", "tax_account_id": sl.tax})).await.status, StatusCode::CREATED);
    let number = code("BILL");
    let bill = json!({"bill_number": number, "supplier_id": sup, "bill_date": format!("{y}-04-02"), "currency_code": "USD",
        "lines": [
            {"description": "Supplies", "quantity": "2", "unit_cost": "50", "expense_account_id": sl.detail, "tax_code": tax, "tax_rate": "20"},
            {"description": "Stock", "quantity": "10", "unit_cost": "3", "product_id": stocked}
        ]});
    let id = create(&mut c, "/api/purchase-bills", bill.clone()).await;
    assert_eq!(c.post("/api/purchase-bills", bill.clone()).await.status, CONFLICT);
    let (sup2, _) = common::party(&mut c, "suppliers").await;
    let mut other = bill.clone();
    other["supplier_id"] = json!(sup2);
    create(&mut c, "/api/purchase-bills", other).await; // bill numbers are per supplier
    assert_eq!(c.get(&format!("/api/purchase-bills/{id}/lines")).await.body[0]["unit_cost"], "50.0000");
    let entry = post(&mut c, "purchase-bills", id).await;
    let mut want = vec![line(sl.detail, "100", "0"), line(inventory, "30", "0"), line(sl.tax, "20", "0"), line(sl.control, "0", "150")];
    want.sort();
    assert_eq!(entry_lines(&mut c, entry).await, want);

    // A sales credit note debits revenue and credits A/R.
    let (cust, l) = common::party(&mut c, "customers").await;
    let note = create(&mut c, "/api/sales-credit-notes", json!({"credit_note_number": code("SCN"), "customer_id": cust, "credit_note_date": format!("{y}-05-01"),
        "currency_code": "USD", "due_date": "ignored", "lines": [{"description": "Return", "unit_price": "40", "revenue_account_id": l.detail}]})).await;
    let n = c.get(&format!("/api/sales-credit-notes/{note}")).await.body;
    assert_eq!((n["application_status"].clone(), n.get("due_date").is_none()), (json!("open"), true));
    let entry = post(&mut c, "sales-credit-notes", note).await;
    let mut want = vec![line(l.detail, "40", "0"), line(l.control, "0", "40")];
    want.sort();
    assert_eq!(entry_lines(&mut c, entry).await, want);

    // A purchase credit note debits A/P and credits the expense.
    let pnote = create(&mut c, "/api/purchase-credit-notes", json!({"credit_note_number": code("PCN"), "supplier_id": sup, "credit_note_date": format!("{y}-05-02"),
        "currency_code": "USD", "lines": [{"description": "Rebate", "unit_cost": "15", "expense_account_id": sl.detail}]})).await;
    let entry = post(&mut c, "purchase-credit-notes", pnote).await;
    let mut want = vec![line(sl.control, "15", "0"), line(sl.detail, "0", "15")];
    want.sort();
    assert_eq!(entry_lines(&mut c, entry).await, want);
    assert_eq!(c.get(&format!("/api/purchase-credit-notes/{pnote}/lines")).await.body[0]["order_line_id"], Value::Null);
}

#[tokio::test]
async fn ordinary_users_cannot_unpost() {
    let pool = common::pool().await;
    let email = common::email("plain");
    common::user(&pool, &email, "plain password", false).await;
    let mut c = Client::new(common::app().await);
    c.login(&email, "plain password").await;
    assert_eq!(status(&mut c, "POST", "/api/sales-invoices/1/unpost", None).await, StatusCode::FORBIDDEN);
    assert_eq!(c.get("/api/journal-entries/999999").await.status, NOT_FOUND);
}
