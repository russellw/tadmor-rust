//! Printed documents and email (spec/api.md §5.11, domain §11). Email is
//! off in tests, as in the conformance configuration, so sending is a 501.

mod common;

use axum::http::StatusCode;
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use common::{code, create};
use serde_json::json;
use tower::ServiceExt;

#[tokio::test]
async fn documents_print_and_email() {
    let mut c = common::admin().await;
    let (cust, l) = common::party(&mut c, "customers").await;
    let number = format!("INV/{} 01", code("P"));
    let inv = create(&mut c, "/api/sales-invoices", json!({"invoice_number": number, "customer_id": cust, "invoice_date": "2101-04-01",
        "currency_code": "USD", "memo": "Thanks (really)", "lines": [{"description": "Item ★", "quantity": "2", "unit_price": "12.50", "revenue_account_id": l.detail}]})).await;

    let req = axum::http::Request::get(format!("/api/sales-invoices/{inv}/pdf"))
        .header("cookie", c.cookie.clone().unwrap())
        .body(axum::body::Body::empty())
        .unwrap();
    let response = c.app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CONTENT_TYPE], "application/pdf");
    let expected = format!("inline; filename=\"invoice-{}.pdf\"", number.replace(['/', ' '], "-"));
    assert_eq!(response.headers()[CONTENT_DISPOSITION], expected.as_str());
    let pdf = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(pdf.starts_with(b"%PDF-1.4") && pdf.ends_with(b"%%EOF\n"));
    assert!(pdf.windows(9).any(|w| w == b"(25.00) T"), "the line amount");
    if let Ok(dir) = std::env::var("TADMOR_PDF_DIR") {
        std::fs::write(format!("{dir}/invoice.pdf"), &pdf).unwrap();
    }
    assert_eq!(c.get("/api/sales-invoices/999999/pdf").await.status, StatusCode::NOT_FOUND);
    assert_eq!(c.get("/api/purchase-orders/999999/pdf").await.status, StatusCode::NOT_FOUND);

    let email = format!("/api/sales-invoices/{inv}/email");
    assert_eq!(c.post(&email, json!({"to": ["someone@example.com"]})).await.status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(c.send("POST", &email, None).await.status, StatusCode::UNPROCESSABLE_ENTITY, "no recipient");
    assert_eq!(c.post(&email, json!({"to": []})).await.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(c.send("POST", &email, Some("{oops")).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(c.post("/api/sales-invoices/999999/email", json!({"to": ["a@b.c"]})).await.status, StatusCode::NOT_FOUND);
    let org = c.get(&format!("/api/customers/{cust}")).await.body["organization_id"].clone();
    let name = c.get(&format!("/api/organizations/{org}")).await.body["name"].clone();
    assert_eq!(c.put(&format!("/api/organizations/{org}"), json!({"name": name, "email": "ap@customer.example"})).await.status, StatusCode::NO_CONTENT);
    assert_eq!(c.send("POST", &email, None).await.status, StatusCode::NOT_IMPLEMENTED, "the organization's email");
}
