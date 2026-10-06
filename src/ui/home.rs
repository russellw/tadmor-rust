//! The home screen (spec/domain.md §13.2, H1 to H5).

use axum::Router;
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use sqlx::PgPool;

use super::forms::money;
use super::{Action, Cell, Section, Session, Table, failure, page};
use crate::error::Result;
use crate::http::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/", get(home))
}

async fn home(State(st): State<AppState>, session: Session) -> Response {
    match sections(&st.pool).await {
        Ok(sections) => page(&session, "Home", sections),
        Err(err) => failure(&session, err),
    }
}

async fn sections(pool: &PgPool) -> Result<Vec<Section>> {
    // H5: one-step starts.
    let starts = Section::Actions(
        vec![
            Action::link("New invoice", "/sales-invoices/new"),
            Action::link("New customer payment", "/customer-payments/new"),
            Action::link("New bill", "/purchase-bills/new"),
            Action::link("New supplier payment", "/supplier-payments/new"),
            Action::link("New sales order", "/sales-orders/new"),
            Action::link("New purchase order", "/purchase-orders/new"),
        ],
        None,
    );

    // H1: outstanding per currency, with the overdue part.
    let receivable = sqlx::query!(
        r#"SELECT currency_code::text AS "currency!", sum(balance)::numeric(19,4)::text AS "outstanding!",
               COALESCE(sum(balance) FILTER (WHERE due_date < current_date), 0)::numeric(19,4)::text AS "overdue!"
           FROM sales_invoice_balances WHERE status = 'posted' AND balance > 0 GROUP BY currency_code ORDER BY currency_code"#
    )
    .fetch_all(pool)
    .await?;
    let payable = sqlx::query!(
        r#"SELECT currency_code::text AS "currency!", sum(balance)::numeric(19,4)::text AS "outstanding!",
               COALESCE(sum(balance) FILTER (WHERE due_date < current_date), 0)::numeric(19,4)::text AS "overdue!"
           FROM purchase_bill_balances WHERE status = 'posted' AND balance > 0 GROUP BY currency_code ORDER BY currency_code"#
    )
    .fetch_all(pool)
    .await?;
    let mut outstanding = Table::new(&["", "Currency", "Outstanding#", "Overdue#"]).titled("Outstanding");
    outstanding.empty = "Nothing outstanding.".into();
    for r in receivable {
        outstanding.rows.push(vec![Cell::link("Receivable", "/reports/ar-aging"), Cell::text(r.currency), Cell::number(money(&r.outstanding)), Cell::number(money(&r.overdue))]);
    }
    for r in payable {
        outstanding.rows.push(vec![Cell::link("Payable", "/reports/ap-aging"), Cell::text(r.currency), Cell::number(money(&r.outstanding)), Cell::number(money(&r.overdue))]);
    }

    // H2: counts.
    let c = sqlx::query!(
        r#"SELECT (SELECT count(*) FROM sales_orders WHERE status = 'open') AS "sales_orders!",
                  (SELECT count(*) FROM purchase_orders WHERE status = 'open') AS "purchase_orders!",
                  (SELECT count(*) FROM sales_invoices WHERE status = 'draft') AS "draft_invoices!",
                  (SELECT count(*) FROM purchase_bills WHERE status = 'draft') AS "draft_bills!""#
    )
    .fetch_one(pool)
    .await?;
    let counts = Section::Facts(vec![
        ("Open sales orders".into(), Cell::link(c.sales_orders.to_string(), "/sales-orders")),
        ("Open purchase orders".into(), Cell::link(c.purchase_orders.to_string(), "/purchase-orders")),
        ("Draft invoices".into(), Cell::link(c.draft_invoices.to_string(), "/sales-invoices")),
        ("Draft bills".into(), Cell::link(c.draft_bills.to_string(), "/purchase-bills")),
    ]);

    // H3: the most overdue invoices.
    let overdue = sqlx::query!(
        r#"SELECT b.invoice_id AS "id!", b.invoice_number AS "number!", o.name AS "party!", b.due_date::text AS "due!",
               b.currency_code::text AS "currency!", b.balance::numeric(19,4)::text AS "balance!"
           FROM sales_invoice_balances b
           JOIN customers c ON c.id = b.customer_id JOIN organizations o ON o.id = c.organization_id
           WHERE b.status = 'posted' AND b.balance > 0 AND b.due_date < current_date
           ORDER BY b.due_date, b.invoice_id LIMIT 10"#
    )
    .fetch_all(pool)
    .await?;
    let mut late = Table::new(&["Invoice", "Customer", "Due", "Balance#"]).titled("Most overdue invoices");
    late.empty = "No invoice is overdue.".into();
    late.rows = overdue
        .into_iter()
        .map(|r| vec![Cell::link(r.number, format!("/sales-invoices/{}", r.id)), Cell::text(r.party), Cell::text(r.due), Cell::number(format!("{} {}", r.currency, money(&r.balance)))])
        .collect();

    // H4: bills due within 14 days.
    let due = sqlx::query!(
        r#"SELECT b.bill_id AS "id!", b.bill_number AS "number!", o.name AS "party!", b.due_date::text AS "due!",
               b.currency_code::text AS "currency!", b.balance::numeric(19,4)::text AS "balance!"
           FROM purchase_bill_balances b
           JOIN suppliers s ON s.id = b.supplier_id JOIN organizations o ON o.id = s.organization_id
           WHERE b.status = 'posted' AND b.balance > 0 AND b.due_date BETWEEN current_date AND current_date + 14
           ORDER BY b.due_date, b.bill_id"#
    )
    .fetch_all(pool)
    .await?;
    let mut soon = Table::new(&["Bill", "Supplier", "Due", "Balance#"]).titled("Bills due in the next 14 days");
    soon.empty = "No bill falls due in the next 14 days.".into();
    soon.rows = due
        .into_iter()
        .map(|r| vec![Cell::link(r.number, format!("/purchase-bills/{}", r.id)), Cell::text(r.party), Cell::text(r.due), Cell::number(format!("{} {}", r.currency, money(&r.balance)))])
        .collect();

    Ok(vec![
        starts,
        Section::Table(outstanding),
        counts,
        Section::Table(late),
        Section::Actions(vec![Action::link("A/R aging", "/reports/ar-aging")], None),
        Section::Table(soon),
        Section::Actions(vec![Action::link("A/P aging", "/reports/ap-aging")], None),
    ])
}
