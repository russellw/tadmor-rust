//! Sales and purchase orders (spec/api.md §5.10, domain §6): commercial
//! documents that never post, confirmed, closed or cancelled by hand, and
//! fulfilled by creating draft invoices, bills and stock movements from
//! their lines.
//!
//! Orders are written exactly as invoices and bills are, so their drafts are
//! created, edited and deleted through src/documents.rs with a `Kind` of
//! their own. The schema's fulfilment views derive every quantity invoiced,
//! billed, shipped or received, and the header statuses; a trigger refuses
//! an order-linked invoice or bill line that would over-invoice the order
//! line, or whose order is not open or has another party or currency.

use serde::Deserialize;
use serde_json::Value;
use sqlx::PgPool;

use crate::documents::{Kind, conflict, parse};
use crate::error::{Error, Result};

pub static SALES_ORDERS: Kind = Kind {
    path: "sales-orders",
    noun: "Sales order",
    table: "sales_orders",
    lines_table: "sales_order_lines",
    line_fk: "order_id",
    number: "order_number",
    party: "customer_id",
    party_table: "customers",
    control: "ar_account_id",
    date: "order_date",
    due: Some("expected_ship_date"),
    price: "unit_price",
    account: "revenue_account_id",
    fallback: "revenue_account_id",
    has_order_lines: false,
    balances_view: "sales_order_fulfilment",
    view_id: "order_id",
    settlement: "invoiced_status",
    detail_is_credit: true,
    detail_memo: "",
    tax_memo: "",
    control_memo: "",
    applications: &[],
};

pub static PURCHASE_ORDERS: Kind = Kind {
    path: "purchase-orders",
    noun: "Purchase order",
    table: "purchase_orders",
    lines_table: "purchase_order_lines",
    line_fk: "order_id",
    number: "order_number",
    party: "supplier_id",
    party_table: "suppliers",
    control: "ap_account_id",
    date: "order_date",
    due: Some("expected_receipt_date"),
    price: "unit_cost",
    account: "expense_account_id",
    fallback: "inventory_account_id",
    has_order_lines: false,
    balances_view: "purchase_order_fulfilment",
    view_id: "order_id",
    settlement: "billed_status",
    detail_is_credit: false,
    detail_memo: "",
    tax_memo: "",
    control_memo: "",
    applications: &[],
};

/// The order-specific names of each side: its fulfilment views, and its two
/// axes (invoiced or billed, shipped or received).
pub struct Side {
    pub kind: &'static Kind,
    pub line_view: &'static str,
    /// The header statuses of the two axes.
    pub document_status: &'static str,
    pub stock_status: &'static str,
    /// The per-line quantities of the two axes, done and to do.
    pub document_done: &'static str,
    pub document_todo: &'static str,
    pub stock_done: &'static str,
    pub stock_todo: &'static str,
}

pub static SALES: Side = Side {
    kind: &SALES_ORDERS,
    line_view: "sales_order_line_fulfilment",
    document_status: "invoiced_status",
    stock_status: "shipped_status",
    document_done: "qty_invoiced",
    document_todo: "qty_to_invoice",
    stock_done: "qty_shipped",
    stock_todo: "qty_to_ship",
};

pub static PURCHASE: Side = Side {
    kind: &PURCHASE_ORDERS,
    line_view: "purchase_order_line_fulfilment",
    document_status: "billed_status",
    stock_status: "received_status",
    document_done: "qty_billed",
    document_todo: "qty_to_bill",
    stock_done: "qty_received",
    stock_todo: "qty_to_receive",
};

fn read_sql(s: &Side) -> String {
    let k = s.kind;
    format!(
        "SELECT json_build_object(
             'id', o.id, 'order_number', o.order_number, '{party}', o.{party}, 'order_date', o.order_date::text,
             '{due}', o.{due}::text, 'currency_code', o.currency_code, 'status', o.status, 'total', o.total::text,
             '{document_status}', f.{document_status}, '{stock_status}', f.{stock_status},
             'reference', o.reference, 'memo', o.memo)::text
         FROM {table} o JOIN {view} f ON f.order_id = o.id",
        party = k.party,
        due = k.due.unwrap_or_default(),
        document_status = s.document_status,
        stock_status = s.stock_status,
        table = k.table,
        view = k.balances_view,
    )
}

/// Every order of the side, newest first, then by id descending.
pub async fn list(pool: &PgPool, s: &Side) -> Result<Vec<Value>> {
    let sql = format!("{} ORDER BY o.order_date DESC, o.id DESC", read_sql(s));
    sqlx::query_scalar::<_, String>(&sql).fetch_all(pool).await?.into_iter().map(parse).collect()
}

pub async fn get(pool: &PgPool, s: &Side, id: i64) -> Result<Value> {
    let sql = format!("{} WHERE o.id = $1::int8", read_sql(s));
    parse(sqlx::query_scalar::<_, String>(&sql).bind(id).fetch_optional(pool).await?.ok_or_else(Error::not_found)?)
}

/// The order's lines in order, each with its own id as `order_line_id` and
/// its fulfilment quantities; 404 for an unknown order.
pub async fn lines(pool: &PgPool, s: &Side, id: i64) -> Result<Vec<Value>> {
    status_of(pool, s, id).await?;
    let k = s.kind;
    let sql = format!(
        "SELECT json_build_object(
             'line_no', l.line_no, 'product_id', l.product_id, 'description', l.description,
             'quantity', l.quantity::text, '{price}', l.{price}::text, 'tax_code', l.tax_code, 'tax_rate', l.tax_rate::text,
             'line_subtotal', l.line_subtotal::text, 'tax_amount', l.tax_amount::text, 'line_total', l.line_total::text,
             '{account}', l.{account}, 'order_line_id', l.id,
             '{document_done}', f.{document_done}::numeric(19,4)::text, '{stock_done}', f.{stock_done}::numeric(19,4)::text,
             '{document_todo}', f.{document_todo}::numeric(19,4)::text, '{stock_todo}', f.{stock_todo}::numeric(19,4)::text)::text
         FROM {lines} l JOIN {view} f ON f.order_line_id = l.id
         WHERE l.order_id = $1::int8 ORDER BY l.line_no",
        price = k.price,
        account = k.account,
        document_done = s.document_done,
        stock_done = s.stock_done,
        document_todo = s.document_todo,
        stock_todo = s.stock_todo,
        lines = k.lines_table,
        view = s.line_view,
    );
    sqlx::query_scalar::<_, String>(&sql).bind(id).fetch_all(pool).await?.into_iter().map(parse).collect()
}

async fn status_of<'e, E: sqlx::PgExecutor<'e>>(executor: E, s: &Side, id: i64) -> Result<String> {
    let sql = format!("SELECT status FROM {} WHERE id = $1::int8", s.kind.table);
    sqlx::query_scalar::<_, String>(&sql).bind(id).fetch_optional(executor).await?.ok_or_else(Error::not_found)
}

async fn set_status(pool: &PgPool, s: &Side, id: i64, status: &str) -> Result<()> {
    let sql = format!("UPDATE {} SET status = $2 WHERE id = $1::int8", s.kind.table);
    sqlx::query(&sql).bind(id).bind(status).execute(pool).await?;
    Ok(())
}

/// draft → open; an order without lines cannot be confirmed (422).
pub async fn confirm(pool: &PgPool, s: &Side, id: i64) -> Result<()> {
    let status = status_of(pool, s, id).await?;
    if status != "draft" {
        return Err(conflict(format!("the order is {status}, not a draft")));
    }
    let sql = format!("SELECT EXISTS (SELECT 1 FROM {} WHERE order_id = $1::int8)", s.kind.lines_table);
    if !sqlx::query_scalar::<_, bool>(&sql).bind(id).fetch_one(pool).await? {
        return Err(Error::unprocessable("an order without lines cannot be confirmed"));
    }
    set_status(pool, s, id, "open").await
}

/// open → closed, by hand: fulfilled orders are not closed automatically.
pub async fn close(pool: &PgPool, s: &Side, id: i64) -> Result<()> {
    let status = status_of(pool, s, id).await?;
    if status != "open" {
        return Err(conflict(format!("the order is {status}, not open")));
    }
    set_status(pool, s, id, "closed").await
}

/// A draft always cancels; an open order only while nothing has been
/// fulfilled against it.
pub async fn cancel(pool: &PgPool, s: &Side, id: i64) -> Result<()> {
    match status_of(pool, s, id).await?.as_str() {
        "draft" => {}
        "open" => {
            let sql = format!(
                "SELECT COALESCE(bool_or({} > 0 OR {} > 0), false) FROM {} WHERE order_id = $1::int8",
                s.document_done, s.stock_done, s.line_view
            );
            if sqlx::query_scalar::<_, bool>(&sql).bind(id).fetch_one(pool).await? {
                return Err(conflict("the order has been partly fulfilled and cannot be cancelled"));
            }
        }
        status => return Err(conflict(format!("the order is {status}"))),
    }
    set_status(pool, s, id, "cancelled").await
}

/// A fulfilment request's choice of quantity for one order line.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct LineQuantity {
    pub order_line_id: i64,
    pub quantity: String,
}

fn split(lines: &[LineQuantity]) -> (Vec<i64>, Vec<String>) {
    lines.iter().map(|l| (l.order_line_id, l.quantity.clone())).unzip()
}

/// Invoicing a sales order, or billing a purchase order.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct DocumentRequest {
    #[serde(alias = "invoice_number", alias = "bill_number")]
    pub number: String,
    #[serde(alias = "invoice_date", alias = "bill_date")]
    pub date: String,
    pub due_date: Option<String>,
    /// Empty means the whole remainder of every line.
    pub lines: Vec<LineQuantity>,
}

/// Creates a draft invoice (or bill) for the order's party and currency,
/// referencing the order, with one line per order line that has something
/// left: the requested quantity capped at what remains, or all of it when
/// no lines are requested. Lines that come to nothing are skipped; if all
/// do, nothing is created (422). Returns the document's id.
pub async fn fulfil_document(pool: &PgPool, s: &Side, target: &Kind, id: i64, r: DocumentRequest) -> Result<i64> {
    if r.number.is_empty() {
        return Err(Error::bad_request(format!("{} is required", target.number)));
    }
    if r.date.is_empty() {
        return Err(Error::bad_request(format!("{} is required", target.date)));
    }
    let order = s.kind;
    let mut tx = pool.begin().await?;
    let status = status_of(&mut *tx, s, id).await?;
    if status != "open" {
        return Err(conflict(format!("the order is {status}, not open")));
    }
    let sql = format!(
        "INSERT INTO {table} ({number}, {party}, {date}, due_date, currency_code, reference)
         SELECT $2, o.{party}, $3::text::date, $4::text::date, o.currency_code, o.order_number
         FROM {orders} o WHERE o.id = $1::int8
         RETURNING id",
        table = target.table,
        number = target.number,
        party = target.party,
        date = target.date,
        orders = order.table,
    );
    let document = sqlx::query_scalar::<_, i32>(&sql).bind(id).bind(&r.number).bind(&r.date).bind(&r.due_date).fetch_one(&mut *tx).await?;
    let (ids, quantities) = split(&r.lines);
    let sql = format!(
        "WITH request AS (
             SELECT order_line_id, qty FROM unnest($2::int8[], $3::text[]::numeric(19,4)[]) AS r(order_line_id, qty)
         ), picked AS (
             SELECT l.id AS order_line_id, l.product_id, l.description, l.{price} AS price, l.{account} AS account_id,
                    l.tax_code, l.tax_rate,
                    CASE WHEN (SELECT count(*) FROM request) = 0 THEN f.{todo}
                         ELSE LEAST(f.{todo}, COALESCE(r.qty, 0)) END AS qty
             FROM {order_lines} l
             JOIN {view} f ON f.order_line_id = l.id
             LEFT JOIN request r ON r.order_line_id = l.id
             WHERE l.order_id = $4::int8
         ), numbered AS (
             SELECT *, row_number() OVER (ORDER BY order_line_id) AS line_no FROM picked WHERE qty > 0
         )
         INSERT INTO {lines} ({fk}, line_no, product_id, description, quantity, {target_price}, {target_account},
                              tax_code, tax_rate, order_line_id)
         SELECT $1, line_no, product_id, description, qty, price, account_id, tax_code, tax_rate, order_line_id
         FROM numbered",
        price = order.price,
        account = order.account,
        todo = s.document_todo,
        order_lines = order.lines_table,
        view = s.line_view,
        lines = target.lines_table,
        fk = target.line_fk,
        target_price = target.price,
        target_account = target.account,
    );
    let inserted = sqlx::query(&sql).bind(document).bind(&ids).bind(&quantities).bind(id).execute(&mut *tx).await?;
    if inserted.rows_affected() == 0 {
        return Err(Error::unprocessable("the order has nothing left to fulfil"));
    }
    tx.commit().await?;
    Ok(i64::from(document))
}

/// Shipping a sales order, or receiving a purchase order.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct StockRequest {
    pub warehouse_id: i64,
    /// Defaults to today (the UTC date).
    pub movement_date: Option<String>,
    pub reference: Option<String>,
    /// Empty means the whole remainder of every line.
    pub lines: Vec<LineQuantity>,
}

impl StockRequest {
    fn check(&self) -> Result<()> {
        if self.warehouse_id <= 0 {
            return Err(Error::bad_request("warehouse_id is required"));
        }
        Ok(())
    }

    fn date(&self) -> Option<String> {
        self.movement_date.clone().filter(|d| !d.is_empty())
    }
}

/// Creates a draft issue for each line of a tracked, active product with
/// something left to ship, valued at the warehouse's moving-average cost of
/// the product (0 with no stock there). Returns the movements' ids.
pub async fn ship(pool: &PgPool, id: i64, r: StockRequest) -> Result<Vec<i32>> {
    r.check()?;
    let mut tx = pool.begin().await?;
    let status = status_of(&mut *tx, &SALES, id).await?;
    if status != "open" {
        return Err(conflict(format!("the order is {status}, not open")));
    }
    let (ids, quantities) = split(&r.lines);
    let movements = sqlx::query_scalar!(
        "WITH request AS (
             SELECT order_line_id, qty FROM unnest($1::int8[], $2::text[]::numeric(19,4)[]) AS r(order_line_id, qty)
         ), picked AS (
             SELECT l.id AS order_line_id, l.product_id, COALESCE(soh.avg_unit_cost, 0) AS unit_cost,
                    CASE WHEN (SELECT count(*) FROM request) = 0 THEN f.qty_to_ship
                         ELSE LEAST(f.qty_to_ship, COALESCE(r.qty, 0)) END AS qty
             FROM sales_order_lines l
             JOIN sales_order_line_fulfilment f ON f.order_line_id = l.id
             JOIN products p ON p.id = l.product_id AND p.track_inventory AND p.is_active
             LEFT JOIN stock_on_hand soh ON soh.product_id = l.product_id AND soh.warehouse_id = $3::int8
             LEFT JOIN request r ON r.order_line_id = l.id
             WHERE l.order_id = $4::int8
         )
         INSERT INTO stock_movements (product_id, warehouse_id, movement_type, movement_date, quantity, unit_cost,
                                      source_type, source_id, reference)
         SELECT product_id, $3::int8, 'issue', COALESCE($5::text::date, current_date), -qty, unit_cost,
                'sales_order_line', order_line_id, $6
         FROM picked WHERE qty > 0 ORDER BY order_line_id
         RETURNING id",
        &ids,
        &quantities,
        r.warehouse_id,
        id,
        r.date(),
        r.reference
    )
    .fetch_all(&mut *tx)
    .await?;
    if movements.is_empty() {
        return Err(Error::unprocessable("the order has nothing left to ship"));
    }
    tx.commit().await?;
    Ok(movements)
}

/// Creates a draft receipt for each line of a tracked, active product with
/// something left to receive, valued in the base currency: the line's cost
/// at the order currency's latest rate on or before the movement date
/// (422 with none). Returns the movements' ids.
pub async fn receive(pool: &PgPool, id: i64, r: StockRequest) -> Result<Vec<i32>> {
    r.check()?;
    let mut tx = pool.begin().await?;
    let status = status_of(&mut *tx, &PURCHASE, id).await?;
    if status != "open" {
        return Err(conflict(format!("the order is {status}, not open")));
    }
    let rate = sqlx::query_scalar!(
        "SELECT CASE WHEN o.currency_code = (SELECT base_currency FROM gl_settings) THEN 1::numeric
                     ELSE (SELECT rate FROM exchange_rates
                           WHERE currency_code = o.currency_code AND rate_date <= COALESCE($2::text::date, current_date)
                           ORDER BY rate_date DESC LIMIT 1)
                END::text
         FROM purchase_orders o WHERE o.id = $1::int8",
        id,
        r.date()
    )
    .fetch_one(&mut *tx)
    .await?
    .ok_or_else(|| Error::unprocessable("no exchange rate for the order's currency on or before the movement date"))?;
    let (ids, quantities) = split(&r.lines);
    let movements = sqlx::query_scalar!(
        "WITH request AS (
             SELECT order_line_id, qty FROM unnest($1::int8[], $2::text[]::numeric(19,4)[]) AS r(order_line_id, qty)
         ), picked AS (
             SELECT l.id AS order_line_id, l.product_id, round(l.unit_cost * $7::text::numeric, 4) AS unit_cost,
                    CASE WHEN (SELECT count(*) FROM request) = 0 THEN f.qty_to_receive
                         ELSE LEAST(f.qty_to_receive, COALESCE(r.qty, 0)) END AS qty
             FROM purchase_order_lines l
             JOIN purchase_order_line_fulfilment f ON f.order_line_id = l.id
             JOIN products p ON p.id = l.product_id AND p.track_inventory AND p.is_active
             LEFT JOIN request r ON r.order_line_id = l.id
             WHERE l.order_id = $4::int8
         )
         INSERT INTO stock_movements (product_id, warehouse_id, movement_type, movement_date, quantity, unit_cost,
                                      source_type, source_id, reference)
         SELECT product_id, $3::int8, 'receipt', COALESCE($5::text::date, current_date), qty, unit_cost,
                'purchase_order_line', order_line_id, $6
         FROM picked WHERE qty > 0 ORDER BY order_line_id
         RETURNING id",
        &ids,
        &quantities,
        r.warehouse_id,
        id,
        r.date(),
        r.reference,
        rate
    )
    .fetch_all(&mut *tx)
    .await?;
    if movements.is_empty() {
        return Err(Error::unprocessable("the order has nothing left to receive"));
    }
    tx.commit().await?;
    Ok(movements)
}
