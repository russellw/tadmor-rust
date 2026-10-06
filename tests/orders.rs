//! Sales and purchase orders and their fulfilment (spec/api.md §5.10,
//! domain §6).

mod common;

use axum::http::StatusCode;
use common::{Client, code, create};
use serde_json::{Value, json};

const CONFLICT: StatusCode = StatusCode::CONFLICT;
const CREATED: StatusCode = StatusCode::CREATED;
const NO_CONTENT: StatusCode = StatusCode::NO_CONTENT;
const UNPROCESSABLE: StatusCode = StatusCode::UNPROCESSABLE_ENTITY;

fn decimal(v: &Value) -> String {
    let s = v.as_str().unwrap_or_else(|| panic!("not a decimal: {v}"));
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s.to_string() }
}

async fn act(c: &mut Client, path: String) -> StatusCode {
    c.send("POST", &path, None).await.status
}

async fn stocked(c: &mut Client) -> i64 {
    let inventory = common::account(c, "asset").await;
    let cogs = common::account(c, "expense").await;
    create(c, "/api/products", json!({"sku": code("SKU"), "name": "Widget", "track_inventory": true, "inventory_account_id": inventory, "cogs_account_id": cogs})).await
}

#[tokio::test]
async fn a_sales_order_from_draft_through_fulfilment() {
    let mut c = common::admin().await;
    let y = common::open_year(&mut c).await;
    let (cust, l) = common::party(&mut c, "customers").await;
    let product = stocked(&mut c).await;
    let wh = create(&mut c, "/api/warehouses", json!({"code": code("WH"), "name": "Main"})).await;
    create(&mut c, "/api/stock-movements", json!({"product_id": product, "warehouse_id": wh, "movement_type": "receipt",
        "movement_date": format!("{y}-01-02"), "quantity": "10", "unit_cost": "4"})).await;

    let number = code("SO");
    let body = json!({"order_number": number, "customer_id": cust, "order_date": format!("{y}-01-10"), "expected_ship_date": format!("{y}-01-20"),
        "currency_code": "USD", "lines": [
            {"description": "Installation", "unit_price": "100", "revenue_account_id": l.detail},
            {"description": "Widgets", "product_id": product, "quantity": "6", "unit_price": "20", "revenue_account_id": l.detail}
        ]});
    let so = create(&mut c, "/api/sales-orders", body.clone()).await;
    assert_eq!(c.post("/api/sales-orders", body.clone()).await.status, CONFLICT);
    let mut zero = body.clone();
    zero["order_number"] = json!(code("SO"));
    zero["lines"] = json!([{"description": "x", "quantity": "0"}]);
    assert_eq!(c.post("/api/sales-orders", zero).await.status, UNPROCESSABLE, "order quantities are positive");
    let o = c.get(&format!("/api/sales-orders/{so}")).await.body;
    assert_eq!((o["status"].clone(), o["invoiced_status"].clone(), o["shipped_status"].clone()), (json!("draft"), json!("none"), json!("none")));
    assert_eq!((decimal(&o["total"]), o["expected_ship_date"].clone()), ("220".into(), json!(format!("{y}-01-20"))));
    let lines = c.get(&format!("/api/sales-orders/{so}/lines")).await.body;
    let (service, widgets) = (lines[0]["order_line_id"].as_i64().unwrap(), lines[1]["order_line_id"].as_i64().unwrap());
    assert_eq!((decimal(&lines[1]["qty_to_ship"]), decimal(&lines[0]["qty_to_ship"])), ("6".into(), "0".into()), "a service never ships");

    let invoice = |n: &str, lines: Value| json!({"invoice_number": n, "invoice_date": format!("{y}-01-11"), "lines": lines});
    assert_eq!(c.post(&format!("/api/sales-orders/{so}/invoice"), invoice(&code("INV"), json!([]))).await.status, CONFLICT, "not open");
    assert_eq!(act(&mut c, format!("/api/sales-orders/{so}/close")).await, CONFLICT);
    assert_eq!(act(&mut c, format!("/api/sales-orders/{so}/confirm")).await, NO_CONTENT);
    assert_eq!(act(&mut c, format!("/api/sales-orders/{so}/confirm")).await, CONFLICT);
    assert_eq!(c.put(&format!("/api/sales-orders/{so}"), body.clone()).await.status, CONFLICT);

    // A partial invoice, then deleting it returns the quantity.
    let r = c.post(&format!("/api/sales-orders/{so}/invoice"), invoice(&code("INV"), json!([{"order_line_id": widgets, "quantity": "2"}]))).await;
    assert_eq!(r.status, CREATED, "{}", r.body);
    let partial = r.body["invoice_id"].as_i64().unwrap();
    let inv = c.get(&format!("/api/sales-invoices/{partial}")).await.body;
    assert_eq!((inv["customer_id"].clone(), inv["reference"].clone(), decimal(&inv["total"])), (json!(cust), json!(number), "40".into()));
    assert_eq!(c.get(&format!("/api/sales-invoices/{partial}/lines")).await.body[0]["order_line_id"], widgets);
    assert_eq!(c.get(&format!("/api/sales-orders/{so}")).await.body["invoiced_status"], "partial");
    let edit = json!({"invoice_number": code("INV"), "customer_id": cust, "invoice_date": format!("{y}-01-11"), "currency_code": "USD"});
    assert_eq!(c.put(&format!("/api/sales-invoices/{partial}"), edit).await.status, CONFLICT, "order-linked");
    assert_eq!(c.send("DELETE", &format!("/api/sales-invoices/{partial}"), None).await.status, NO_CONTENT);
    assert_eq!(c.get(&format!("/api/sales-orders/{so}")).await.body["invoiced_status"], "none");

    // Requests are capped at what remains.
    let r = c.post(&format!("/api/sales-orders/{so}/invoice"), invoice(&code("INV"), json!([{"order_line_id": widgets, "quantity": "99"}, {"order_line_id": service, "quantity": "1"}]))).await;
    assert_eq!(decimal(&c.get(&format!("/api/sales-invoices/{}", r.body["invoice_id"])).await.body["total"]), "220");
    assert_eq!(c.post(&format!("/api/sales-orders/{so}/invoice"), invoice(&code("INV"), json!([]))).await.status, UNPROCESSABLE, "nothing left");
    assert_eq!(act(&mut c, format!("/api/sales-orders/{so}/cancel")).await, CONFLICT, "fulfilled");

    // Shipping at the warehouse's moving-average cost.
    assert_eq!(c.post(&format!("/api/sales-orders/{so}/ship"), json!({})).await.status, StatusCode::BAD_REQUEST);
    let r = c.post(&format!("/api/sales-orders/{so}/ship"), json!({"warehouse_id": wh, "movement_date": format!("{y}-01-15")})).await;
    assert_eq!(r.status, CREATED, "{}", r.body);
    let ids = r.body["movement_ids"].as_array().unwrap().clone();
    assert_eq!(ids.len(), 1, "the service line is skipped");
    let m = c.get(&format!("/api/stock-movements/{}", ids[0])).await.body;
    assert_eq!((m["movement_type"].clone(), m["source_type"].clone(), decimal(&m["quantity"]), decimal(&m["unit_cost"])),
               (json!("issue"), json!("sales_order_line"), "-6".into(), "4".into()));
    assert_eq!(c.get(&format!("/api/sales-orders/{so}")).await.body["shipped_status"], "shipped");
    let edit = json!({"product_id": product, "warehouse_id": wh, "movement_type": "issue", "quantity": "-1"});
    assert_eq!(c.put(&format!("/api/stock-movements/{}", ids[0]), edit).await.status, CONFLICT, "a fulfilment movement");

    assert_eq!(act(&mut c, format!("/api/sales-orders/{so}/close")).await, NO_CONTENT);
    assert_eq!(c.post(&format!("/api/sales-orders/{so}/ship"), json!({"warehouse_id": wh})).await.status, CONFLICT);
    assert_eq!(act(&mut c, format!("/api/sales-orders/{so}/cancel")).await, CONFLICT);
    assert_eq!(act(&mut c, "/api/sales-orders/999999/confirm".into()).await, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn confirming_and_cancelling() {
    let mut c = common::admin().await;
    let (cust, _) = common::party(&mut c, "customers").await;
    let order = |lines: Value| json!({"order_number": code("SO"), "customer_id": cust, "order_date": "2101-02-01", "currency_code": "USD", "lines": lines});
    let empty = create(&mut c, "/api/sales-orders", order(json!([]))).await;
    assert_eq!(act(&mut c, format!("/api/sales-orders/{empty}/confirm")).await, UNPROCESSABLE, "no lines");
    assert_eq!(act(&mut c, format!("/api/sales-orders/{empty}/cancel")).await, NO_CONTENT, "a draft always cancels");
    assert_eq!(act(&mut c, format!("/api/sales-orders/{empty}/confirm")).await, CONFLICT);
    let open = create(&mut c, "/api/sales-orders", order(json!([{"description": "x", "unit_price": "1"}]))).await;
    assert_eq!(act(&mut c, format!("/api/sales-orders/{open}/confirm")).await, NO_CONTENT);
    assert_eq!(act(&mut c, format!("/api/sales-orders/{open}/cancel")).await, NO_CONTENT, "unfulfilled");
    let draft = create(&mut c, "/api/sales-orders", order(json!([]))).await;
    assert_eq!(c.send("DELETE", &format!("/api/sales-orders/{draft}"), None).await.status, NO_CONTENT);
    assert_eq!(c.get(&format!("/api/sales-orders/{draft}/lines")).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn receiving_values_stock_in_the_base_currency() {
    let mut c = common::admin().await;
    let y = common::open_year(&mut c).await;
    let (sup, _) = common::party(&mut c, "suppliers").await;
    let product = stocked(&mut c).await;
    let wh = create(&mut c, "/api/warehouses", json!({"code": code("WH"), "name": "Main"})).await;
    let grni = common::account(&mut c, "liability").await;
    // Only this test uses GBP.
    let po = create(&mut c, "/api/purchase-orders", json!({"order_number": code("PO"), "supplier_id": sup, "order_date": format!("{y}-01-05"),
        "currency_code": "GBP", "lines": [{"description": "Widgets", "product_id": product, "quantity": "4", "unit_cost": "1234.5", "expense_account_id": grni}]})).await;
    assert_eq!(act(&mut c, format!("/api/purchase-orders/{po}/confirm")).await, NO_CONTENT);
    let receive = |date: String, lines: Value| json!({"warehouse_id": wh, "movement_date": date, "lines": lines});
    assert_eq!(c.post(&format!("/api/purchase-orders/{po}/receive"), receive(format!("{y}-01-10"), json!([]))).await.status, UNPROCESSABLE, "no rate");
    assert_eq!(c.post("/api/exchange-rates", json!({"currency_code": "GBP", "rate_date": format!("{y}-01-01"), "rate": "1.25001"})).await.status, CREATED);

    let line = c.get(&format!("/api/purchase-orders/{po}/lines")).await.body[0]["order_line_id"].as_i64().unwrap();
    let r = c.post(&format!("/api/purchase-orders/{po}/receive"), receive(format!("{y}-02-15"), json!([{"order_line_id": line, "quantity": "1"}]))).await;
    assert_eq!(r.status, CREATED, "{}", r.body);
    let m = c.get(&format!("/api/stock-movements/{}", r.body["movement_ids"][0])).await.body;
    // 1234.5 × 1.25001 = 1543.137345, so 1543.1373 in base.
    assert_eq!((m["movement_type"].clone(), decimal(&m["unit_cost"])), (json!("receipt"), "1543.1373".into()));
    assert_eq!(c.get(&format!("/api/purchase-orders/{po}")).await.body["received_status"], "partial");

    let r = c.post(&format!("/api/purchase-orders/{po}/bill"), json!({"bill_number": code("BILL"), "bill_date": format!("{y}-02-20")})).await;
    assert_eq!(r.status, CREATED, "{}", r.body);
    let bill = c.get(&format!("/api/purchase-bills/{}", r.body["bill_id"])).await.body;
    assert_eq!((bill["currency_code"].clone(), decimal(&bill["total"])), (json!("GBP"), "4938".into()));
    let o = c.get(&format!("/api/purchase-orders/{po}")).await.body;
    assert_eq!((o["billed_status"].clone(), o["received_status"].clone()), (json!("billed"), json!("partial")));
    assert_eq!(c.post(&format!("/api/purchase-orders/{po}/bill"), json!({"bill_number": "", "bill_date": format!("{y}-02-20")})).await.status, StatusCode::BAD_REQUEST);
}
