//! Stock movements and inventory valuation (spec/api.md §5.12, domain §4,
//! §10).

mod common;

use axum::http::StatusCode;
use common::{Client, code, create, entry_lines, line};
use serde_json::{Value, json};

const CONFLICT: StatusCode = StatusCode::CONFLICT;
const OK: StatusCode = StatusCode::OK;
const UNPROCESSABLE: StatusCode = StatusCode::UNPROCESSABLE_ENTITY;

fn decimal(v: &Value) -> String {
    let s = v.as_str().unwrap();
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s.to_string() }
}

/// A tracked product with inventory and COGS accounts.
async fn stocked(c: &mut Client) -> (i64, i64, i64) {
    let inventory = common::account(c, "asset").await;
    let cogs = common::account(c, "expense").await;
    let product = create(c, "/api/products", json!({"sku": code("SKU"), "name": "Stocked", "track_inventory": true,
        "inventory_account_id": inventory, "cogs_account_id": cogs})).await;
    (product, inventory, cogs)
}

async fn valuation(c: &mut Client, product: i64) -> (String, String, String) {
    let rows = c.get("/api/inventory-valuation").await.body;
    let r = rows.as_array().unwrap().iter().find(|r| r["product_id"] == product).expect("the product's valuation").clone();
    (decimal(&r["qty_on_hand"]), decimal(&r["value_on_hand"]), decimal(&r["avg_unit_cost"]))
}

#[tokio::test]
async fn movements_from_entry_to_posting() {
    let mut c = common::admin().await;
    let y = common::open_year(&mut c).await;
    let (product, inventory, cogs) = stocked(&mut c).await;
    let wh = create(&mut c, "/api/warehouses", json!({"code": code("WH"), "name": "Main"})).await;
    let grni = common::account(&mut c, "liability").await;
    let untracked = create(&mut c, "/api/products", json!({"sku": code("SKU"), "name": "Service"})).await;
    let mv = |kind: &str, qty: &str, cost: &str| json!({"product_id": product, "warehouse_id": wh, "movement_type": kind,
        "movement_date": format!("{y}-02-01"), "quantity": qty, "unit_cost": cost});

    assert_eq!(c.post("/api/stock-movements", json!({"warehouse_id": wh, "movement_type": "receipt", "quantity": "1"})).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(c.post("/api/stock-movements", json!({"product_id": product, "warehouse_id": wh, "quantity": "1"})).await.status, StatusCode::BAD_REQUEST);
    for (bad, why) in [
        (mv("receipt", "-1", "5"), "a negative receipt"),
        (mv("issue", "1", "5"), "a positive issue"),
        (mv("adjustment", "0", "5"), "zero"),
        (mv("teleport", "1", "5"), "an unknown type"),
        (mv("receipt", "1", "-5"), "a negative cost"),
        (json!({"product_id": untracked, "warehouse_id": wh, "movement_type": "receipt", "quantity": "1"}), "an untracked product"),
    ] {
        assert_eq!(c.post("/api/stock-movements", bad).await.status, UNPROCESSABLE, "{why}");
    }

    let receipt = create(&mut c, "/api/stock-movements", mv("receipt", "10", "5")).await;
    let m = c.get(&format!("/api/stock-movements/{receipt}")).await.body;
    assert_eq!((decimal(&m["total_cost"]), m["status"].clone(), m["source_type"].clone()), ("50".into(), json!("draft"), Value::Null));
    create(&mut c, "/api/stock-movements", mv("adjustment", "-1", "5")).await;
    let issue = create(&mut c, "/api/stock-movements", mv("issue", "-4", "5")).await;
    assert_eq!(valuation(&mut c, product).await, ("5".into(), "25".into(), "5".into()));
    assert_eq!(c.put(&format!("/api/stock-movements/{receipt}"), mv("receipt", "12", "5")).await.status, StatusCode::NO_CONTENT);
    assert_eq!(valuation(&mut c, product).await.0, "7");

    // A receipt posts against a postable credit account; the body is optional.
    let post = |id: i64| format!("/api/stock-movements/{id}/post");
    assert_eq!(c.send("POST", &post(receipt), None).await.status, UNPROCESSABLE);
    let header = create(&mut c, "/api/accounts", json!({"code": code("A"), "name": "Header", "account_type": "liability"})).await;
    assert_eq!(c.post(&post(receipt), json!({"credit_account_id": header})).await.status, UNPROCESSABLE);
    let r = c.post(&post(receipt), json!({"credit_account_id": grni})).await;
    assert_eq!(r.status, OK, "{}", r.body);
    let mut want = vec![line(inventory, "60", "0"), line(grni, "0", "60")];
    want.sort();
    assert_eq!(entry_lines(&mut c, r.body["journal_entry_id"].as_i64().unwrap()).await, want);
    assert_eq!(c.get(&format!("/api/stock-movements/{receipt}")).await.body["status"], "posted");
    assert_eq!(c.post(&post(receipt), json!({"credit_account_id": grni})).await.status, CONFLICT);
    assert_eq!(c.send("POST", &post(receipt), None).await.status, CONFLICT, "409 comes before the missing account");
    assert_eq!(c.put(&format!("/api/stock-movements/{receipt}"), mv("receipt", "1", "5")).await.status, CONFLICT);
    assert_eq!(c.send("DELETE", &format!("/api/stock-movements/{receipt}"), None).await.status, CONFLICT);

    // An issue debits COGS and credits inventory, at the cost's magnitude.
    let r = c.send("POST", &post(issue), None).await;
    let mut want = vec![line(cogs, "20", "0"), line(inventory, "0", "20")];
    want.sort();
    assert_eq!(entry_lines(&mut c, r.body["journal_entry_id"].as_i64().unwrap()).await, want);
    let adjustment = create(&mut c, "/api/stock-movements", mv("adjustment", "1", "5")).await;
    assert_eq!(c.send("POST", &post(adjustment), None).await.status, UNPROCESSABLE, "adjustments do not post");
    let free = create(&mut c, "/api/stock-movements", mv("issue", "-1", "0")).await;
    assert_eq!(c.send("POST", &post(free), None).await.status, UNPROCESSABLE, "nothing to post");

    // Unposting keeps the quantity record.
    let r = c.send("POST", &format!("/api/stock-movements/{receipt}/unpost"), None).await;
    assert_eq!(r.status, OK);
    let m = c.get(&format!("/api/stock-movements/{receipt}")).await.body;
    assert_eq!((m["journal_entry_id"].clone(), decimal(&m["quantity"])), (Value::Null, "12".into()));
    assert_eq!(c.send("POST", &format!("/api/stock-movements/{receipt}/unpost"), None).await.status, CONFLICT);
    assert_eq!(c.send("DELETE", &format!("/api/stock-movements/{receipt}"), None).await.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn the_date_defaults_to_today_and_average_cost_is_exact() {
    let mut c = common::admin().await;
    let (product, _, _) = stocked(&mut c).await;
    let wh = create(&mut c, "/api/warehouses", json!({"code": code("WH"), "name": "Main"})).await;
    let receipt = |qty: &str, cost: &str| json!({"product_id": product, "warehouse_id": wh, "movement_type": "receipt", "quantity": qty, "unit_cost": cost});
    let id = create(&mut c, "/api/stock-movements", receipt("123456788.0123", "0")).await;
    let date = c.get(&format!("/api/stock-movements/{id}")).await.body["movement_date"].as_str().unwrap().to_string();
    let today: String = sqlx::query_scalar("SELECT (now() AT TIME ZONE 'UTC')::date::text").fetch_one(&common::pool().await).await.unwrap();
    assert_eq!(date, today);
    create(&mut c, "/api/stock-movements", receipt("1", "920907399.1189")).await;
    // 920907399.1189 / 123456789.0123 = 7.45934999999999995…, so 7.4593.
    assert_eq!(valuation(&mut c, product).await.2, "7.4593");
}
