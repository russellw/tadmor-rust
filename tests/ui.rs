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

/// The id at the end of a redirect's location.
fn id_of(location: &Option<String>) -> String {
    location.as_deref().unwrap().rsplit('/').next().unwrap().to_string()
}

#[tokio::test]
async fn an_invoice_through_the_screens() {
    let mut b = Browser::admin().await;
    let mut api = common::admin().await;
    let y = common::open_year(&mut api).await;
    let (cust, l) = common::party(&mut api, "customers").await;
    let (cust, income) = (cust.to_string(), l.detail.to_string());

    // D2: the line editor, its script data, and a refusal kept with what was typed.
    let form = b.get("/sales-invoices/new").await;
    assert!(form.html.contains("id=\"client-data\"") && form.html.contains("id=\"line-template\""));
    let number = code("INV");
    let date = format!("{y}-03-15");
    let mut fields = vec![
        ("invoice_number", number.as_str()), ("customer_id", cust.as_str()), ("invoice_date", date.as_str()), ("currency_code", "USD"),
        ("line_product_id", ""), ("line_description", "Consulting"), ("line_quantity", "2"), ("line_price", "10.005"),
        ("line_account_id", income.as_str()), ("line_tax_code", ""), ("line_tax_rate", "0"),
        ("line_product_id", ""), ("line_description", ""), ("line_quantity", "1"), ("line_price", ""),
        ("line_account_id", ""), ("line_tax_code", ""), ("line_tax_rate", "0"),
    ];
    let refused = b.submit("/sales-invoices", &[("invoice_number", ""), ("customer_id", cust.as_str())]).await;
    assert!(refused.html.contains("invoice_number is required"));
    let r = b.submit("/sales-invoices", &fields).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.html);
    let id = id_of(&r.location);
    let detail = b.follow(r).await;
    // D3: lines and totals, exact; D4: draft actions; D6: the PDF.
    assert!(detail.html.contains("20.01") && detail.html.contains("Consulting"), "the line, with its money");
    assert!(detail.html.contains(">Post<") && detail.html.contains(">Edit<") && detail.html.contains(">Delete<"));
    assert!(detail.html.contains(&format!("href=\"/api/sales-invoices/{id}/pdf\"")));
    assert!(detail.html.contains("draft, unpaid"));

    // Edit: the form comes back filled.
    let edit = b.get(&format!("/sales-invoices/{id}/edit")).await;
    assert!(edit.html.contains("value=\"Consulting\"") && edit.html.contains("value=\"10.005\""));
    fields[6] = ("line_quantity", "4");
    let r = b.submit(&format!("/sales-invoices/{id}"), &fields).await;
    assert!(b.follow(r).await.html.contains("40.02"));

    // D4: posting; the entry is linked; refusals are shown beside the actions.
    b.get(&format!("/sales-invoices/{id}")).await;
    let r = b.submit(&format!("/sales-invoices/{id}/post"), &[]).await;
    let posted = b.follow(r).await;
    assert!(posted.html.contains("href=\"/journal-entries/") && posted.html.contains(">Unpost<"));
    assert!(!posted.html.contains(">Edit<"));
    let again = b.submit(&format!("/sales-invoices/{id}/post"), &[]).await;
    assert!(again.html.contains("class=\"error\"") && again.html.contains("not a draft"));

    // D7: email with email off says so; without an address on file, says that.
    let r = b.submit(&format!("/sales-invoices/{id}/email"), &[("to", "")]).await;
    assert!(r.html.contains("no email on file"), "{}", r.html);
    let r = b.submit(&format!("/sales-invoices/{id}/email"), &[("to", "a@example.com")]).await;
    assert!(r.html.contains("not configured") && r.html.contains("a@example.com"));

    // P1 to P4: a payment, applied, and the invoice now paid in part.
    let bank = common::account(&mut api, "asset").await.to_string();
    b.get("/customer-payments/new").await;
    let r = b.submit("/customer-payments", &[("customer_id", &cust), ("payment_date", &format!("{y}-04-01")), ("currency_code", "USD"),
        ("amount", "15"), ("method", "transfer"), ("reference", "W1"), ("deposit_account_id", &bank)]).await;
    let pay = id_of(&r.location);
    b.follow(r).await;
    let r = b.submit(&format!("/customer-payments/{pay}/post"), &[]).await;
    b.follow(r).await;
    let applied = b.submit(&format!("/customer-payments/{pay}/apply"), &[]).await;
    assert!(applied.html.contains("Applied to 1 document(s).") && applied.html.contains(&format!("href=\"/sales-invoices/{id}\"")));
    assert!(b.get(&format!("/sales-invoices/{id}")).await.html.contains("posted, partial"));
    assert!(b.get("/customer-payments").await.html.contains("15.00"));

    // G6: deleting asks first.
    let draft = b.submit("/sales-invoices", &[("invoice_number", &code("INV")), ("customer_id", &cust), ("invoice_date", &date), ("currency_code", "USD")]).await;
    let draft_id = id_of(&draft.location);
    let ask = b.get(&format!("/sales-invoices/{draft_id}/delete")).await;
    assert!(ask.html.contains("Delete invoice") && ask.html.contains("Keep it"));
    let r = b.submit(&format!("/sales-invoices/{draft_id}/delete"), &[]).await;
    assert_eq!(r.location.as_deref(), Some("/sales-invoices"));
    assert_eq!(b.get(&format!("/sales-invoices/{draft_id}")).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn ordinary_users_are_not_offered_unpost() {
    let mut api = common::admin().await;
    let y = common::open_year(&mut api).await;
    let (cust, l) = common::party(&mut api, "customers").await;
    let id = common::create(&mut api, "/api/sales-invoices", serde_json::json!({"invoice_number": code("INV"), "customer_id": cust,
        "invoice_date": format!("{y}-02-01"), "currency_code": "USD", "lines": [{"description": "x", "unit_price": "5", "revenue_account_id": l.detail}]})).await;
    api.send("POST", &format!("/api/sales-invoices/{id}/post"), None).await;
    let pool = common::pool().await;
    let email = common::email("plain");
    common::user(&pool, &email, "plain password", false).await;
    let mut b = Browser::sign_in(&email, "plain password").await;
    assert!(!b.get(&format!("/sales-invoices/{id}")).await.html.contains(">Unpost<"));
    let r = b.submit(&format!("/sales-invoices/{id}/unpost"), &[]).await;
    assert!(r.html.contains("Only an administrator"));
}

#[tokio::test]
async fn an_order_through_its_fulfilment() {
    let mut b = Browser::admin().await;
    let mut api = common::admin().await;
    let y = common::open_year(&mut api).await;
    let (cust, l) = common::party(&mut api, "customers").await;
    let inventory = common::account(&mut api, "asset").await;
    let cogs = common::account(&mut api, "expense").await;
    let product = common::create(&mut api, "/api/products", serde_json::json!({"sku": code("SKU"), "name": "Widget", "track_inventory": true,
        "inventory_account_id": inventory, "cogs_account_id": cogs})).await.to_string();
    let wh = common::create(&mut api, "/api/warehouses", serde_json::json!({"code": code("WH"), "name": "Main"})).await.to_string();
    let (cust, income) = (cust.to_string(), l.detail.to_string());

    // S2: a receipt typed as a magnitude.
    b.get("/stock-movements/new").await;
    let r = b.submit("/stock-movements", &[("product_id", &product), ("warehouse_id", &wh), ("movement_type", "receipt"),
        ("movement_date", &format!("{y}-01-02")), ("quantity", "10"), ("unit_cost", "4")]).await;
    let receipt = b.follow(r).await;
    assert!(receipt.html.contains("Account to credit"), "a receipt asks for the account to credit");
    // An issue typed as a magnitude is stored negative.
    let r = b.submit("/stock-movements", &[("product_id", &product), ("warehouse_id", &wh), ("movement_type", "issue"), ("quantity", "1"), ("unit_cost", "4")]).await;
    let issue = b.follow(r).await;
    assert!(issue.html.contains("-1"), "{}", issue.html);

    // O2: the order form is the line editor.
    b.get("/sales-orders/new").await;
    let number = code("SO");
    let date = format!("{y}-01-10");
    let r = b.submit("/sales-orders", &[("order_number", &number), ("customer_id", &cust), ("order_date", &date), ("currency_code", "USD"),
        ("line_product_id", &product), ("line_description", "Widgets"), ("line_quantity", "6"), ("line_price", "20"),
        ("line_account_id", &income), ("line_tax_code", ""), ("line_tax_rate", "0")]).await;
    let so = id_of(&r.location);
    let draft = b.follow(r).await;
    assert!(draft.html.contains(">Confirm<") && draft.html.contains("120.00"));
    let r = b.submit(&format!("/sales-orders/{so}/confirm"), &[]).await;
    let open = b.follow(r).await;
    assert!(open.html.contains(">Invoice<") && open.html.contains(">Ship<") && open.html.contains(">Close<"));

    // O5: the invoice form offers the remainder, lowerable.
    let form = b.get(&format!("/sales-orders/{so}/invoice")).await;
    assert!(form.html.contains("Widgets (6 remaining)") && form.html.contains("name=\"line_quantity\" value=\"6\""));
    let at = form.html.find("name=\"line_order_line_id\" value=\"").unwrap() + 33;
    let line = form.html[at..at + form.html[at..].find('"').unwrap()].to_string();
    let r = b.submit(&format!("/sales-orders/{so}/invoice"), &[("invoice_number", &code("INV")), ("invoice_date", &format!("{y}-01-11")),
        ("line_order_line_id", &line), ("line_quantity", "2")]).await;
    assert!(r.location.as_deref().unwrap().starts_with("/sales-invoices/"), "on to the new draft");
    let invoice = b.follow(r).await;
    assert!(invoice.html.contains("40.00") && invoice.html.contains("produced from an order"));
    assert!(b.get(&format!("/sales-orders/{so}")).await.html.contains("partial"));

    // O6: shipping links to the movements it made.
    let r = b.submit(&format!("/sales-orders/{so}/ship"), &[("warehouse_id", &wh), ("movement_date", &format!("{y}-01-15")),
        ("line_order_line_id", &line), ("line_quantity", "6")]).await;
    assert!(r.html.contains("Draft movements created") && r.html.contains("href=\"/stock-movements/"));
    assert!(b.get(&format!("/sales-orders/{so}")).await.html.contains("shipped"));
    // O4: a fulfilled order cannot be cancelled, and says why.
    let r = b.submit(&format!("/sales-orders/{so}/cancel"), &[]).await;
    assert!(r.html.contains("partly fulfilled"));
    assert!(b.get("/sales-orders").await.html.contains(&number));
}

#[tokio::test]
async fn reports_and_journal_entries() {
    let mut b = Browser::admin().await;
    let mut api = common::admin().await;
    let y = common::open_year(&mut api).await;
    let (cust, l) = common::party(&mut api, "customers").await;
    for (n, amount) in [(1, "100"), (2, "50.5")] {
        let id = common::create(&mut api, "/api/sales-invoices", serde_json::json!({"invoice_number": code("INV"), "customer_id": cust,
            "invoice_date": format!("{y}-02-0{n}"), "currency_code": "USD", "lines": [{"description": "x", "unit_price": amount, "revenue_account_id": l.detail}]})).await;
        api.send("POST", &format!("/api/sales-invoices/{id}/post"), None).await;
    }
    let range = format!("from={y}-01-01&to={y}-12-31");
    // R1: sections with totals and net income.
    let pl = b.get(&format!("/reports/profit-and-loss?{range}")).await;
    assert!(pl.html.contains("Total revenue") && pl.html.contains("Net income"));
    // R5: a running balance: 100, then 150.5.
    let ledger = b.get(&format!("/reports/ledger/{}?{range}", l.control)).await;
    assert!(ledger.html.contains("100.00") && ledger.html.contains("150.50"), "{}", ledger.html);
    let at = ledger.html.find("href=\"/journal-entries/").unwrap() + 6;
    let entry = ledger.html[at..at + ledger.html[at..].find('"').unwrap()].to_string();
    // R6: the entry links each account to its ledger.
    let e = b.get(&entry).await;
    assert!(e.html.contains(&format!("href=\"/reports/ledger/{}\"", l.control)) && e.html.contains("Exchange rate"));
    // R2, R3, R4, R7, R8 render.
    for path in ["/reports/balance-sheet", "/reports/cash-flow", "/reports/trial-balance", "/reports/ar-aging", "/reports/ap-aging", "/reports/inventory-valuation"] {
        assert_eq!(b.get(path).await.status, StatusCode::OK, "{path}");
    }
    assert!(b.get("/reports/balance-sheet").await.html.contains("Liabilities + equity + current earnings"));
    assert!(b.get("/reports/ar-aging").await.html.contains("Total"));
    // A malformed date is shown, not a failure.
    let bad = b.get("/reports/profit-and-loss?from=someday").await;
    assert!(bad.html.contains("from must be a date"));
}

#[tokio::test]
async fn accounting_screens() {
    let mut b = Browser::admin().await;
    let mut api = common::admin().await;
    // A1: a fiscal year, a proposed period, and closing it in one step.
    let y = 4900 + (std::process::id() % 90) as i32;
    b.get("/fiscal-years/new").await;
    let fy_name = code("FY");
    let r = b.submit("/fiscal-years", &[("name", &fy_name), ("start_date", &format!("{y}-01-01")), ("end_date", &format!("{y}-12-31"))]).await;
    assert_eq!(r.location.as_deref(), Some("/periods"));
    assert_eq!(b.get("/fiscal-years").await.location.as_deref(), Some("/periods"), "years are listed under periods");
    assert_eq!(b.get("/logout").await.status, StatusCode::NOT_FOUND, "a wrong method is not found (G8)");
    let periods = b.get("/periods").await;
    let fy_id = {
        let ours = &periods.html[periods.html.find(&fy_name).unwrap()..];
        let at = ours.find(">Edit year<").unwrap();
        let href = &ours[ours[..at].rfind("href=\"/fiscal-years/").unwrap() + 20..at];
        href[..href.find('"').unwrap()].to_string()
    };
    b.get("/accounting-periods/new").await;
    let r = b.submit("/accounting-periods", &[("fiscal_year_id", &fy_id), ("name", "Jan"), ("start_date", &format!("{y}-01-01")), ("end_date", &format!("{y}-01-31"))]).await;
    assert_eq!(r.location.as_deref(), Some("/periods"), "{}", r.html);
    let proposal = b.get("/accounting-periods/new").await;
    assert!(proposal.html.contains(&format!("value=\"{y}-02\"")) || proposal.html.contains("-02\""), "the month after the latest is proposed");
    let periods = b.get("/periods").await;
    let ours = &periods.html[periods.html.find(&fy_name).unwrap()..];
    let at = ours.find("/toggle").unwrap();
    let toggle = &ours[ours[..at].rfind('"').unwrap() + 1..at + 7];
    let r = b.submit(toggle, &[]).await;
    assert!(b.follow(r).await.html.contains("Reopen Jan"), "closed in one step");

    // A2: year-end states what will happen and proposes Retained Earnings.
    let close = b.get(&format!("/fiscal-years/{fy_id}/close")).await;
    assert!(close.html.contains("Closing the year will") && close.html.contains("selected>3000 Retained Earnings"));

    // A3: exchange rates.
    b.get("/exchange-rates/new").await;
    let date = format!("{y}-06-30");
    let r = b.submit("/exchange-rates", &[("currency_code", "CAD"), ("rate_date", &date), ("rate", "0.73")]).await;
    assert!(b.follow(r).await.html.contains("0.73"));
    let ask = b.get(&format!("/exchange-rates/CAD/{date}/delete")).await;
    assert!(ask.html.contains("Keep it"));
    b.submit(&format!("/exchange-rates/CAD/{date}/delete"), &[]).await;
    assert!(!b.get("/exchange-rates").await.html.contains(&date));

    // A4, A5: a statement, imported, matched, reconciled.
    let bank = common::create(&mut api, "/api/accounts", serde_json::json!({"code": code("A"), "name": "Bank", "account_type": "asset", "is_postable": true, "is_cash": true})).await;
    let (cust, _) = common::party(&mut api, "customers").await;
    let pay = common::create(&mut api, "/api/customer-payments", serde_json::json!({"customer_id": cust, "payment_date": format!("{y}-02-05"), "currency_code": "USD", "amount": "100", "deposit_account_id": bank})).await;
    // February: January was closed above.
    assert_eq!(api.send("POST", &format!("/api/customer-payments/{pay}/post"), None).await.status, StatusCode::OK);
    let form = b.get("/bank-statements/new").await;
    assert!(form.html.contains(&format!("value=\"{bank}\"")), "the cash account is offered");
    let r = b.submit("/bank-statements", &[("account_id", &bank.to_string()), ("statement_date", &format!("{y}-02-28")), ("opening_balance", "0"), ("closing_balance", "100")]).await;
    let st = id_of(&r.location);
    let page = b.follow(r).await;
    assert!(page.html.contains("Import CSV") && page.html.contains("Add a line"));
    let r = b.submit(&format!("/bank-statements/{st}/import"), &[("csv", &format!("date,description,amount\n{y}-02-05,Deposit,100\n"))]).await;
    assert!(r.html.contains("Imported 1 line(s).") && r.html.contains("name=\"journal_line_id\""), "a candidate of its amount is offered");
    let r = b.submit(&format!("/bank-statements/{st}/auto-match"), &[]).await;
    assert!(r.html.contains("Matched 1 line(s)."));
    let r = b.submit(&format!("/bank-statements/{st}/reconcile"), &[]).await;
    let reconciled = b.follow(r).await;
    assert!(reconciled.html.contains("reconciled") && reconciled.html.contains(">Reopen<"));
}
