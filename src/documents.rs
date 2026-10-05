//! Invoices, bills, and both kinds of credit note (spec/api.md §5.9,
//! domain §2 and §4): the four subledger documents with lines.
//!
//! The four kinds share one lifecycle and differ only in names and in which
//! side of the ledger their lines post to, so each is described once by a
//! `Kind`, and the SQL is built from that description at run time. Table and
//! column names come only from the descriptions, never from a request, and
//! every value is a bind parameter. (Everything else in the crate uses
//! sqlx's compile-time-checked macros, which need literal SQL; for these,
//! the tests stand in for the compile-time check.)
//!
//! The schema does much of the work: line money is generated from quantity,
//! price and tax rate; triggers keep a draft's header totals current; the
//! balance views derive amounts applied, balances and statuses; and
//! constraints refuse a zero quantity, a due date before the document date,
//! and unknown references. Read shapes are built in SQL with
//! `json_build_object`, with every decimal cast to text, so money is never
//! a binary float on its way out.

use serde_json::{Map, Value};
use sqlx::{PgConnection, PgPool};

use crate::error::{Error, Result};
use crate::posting;

/// Everything that distinguishes one kind of document from another.
pub struct Kind {
    /// The collection's path under /api/, and its name in messages.
    pub path: &'static str,
    pub noun: &'static str,
    pub table: &'static str,
    pub lines_table: &'static str,
    /// The lines' column referring to the document.
    pub line_fk: &'static str,
    pub number: &'static str,
    pub party: &'static str,
    pub party_table: &'static str,
    /// The party's control account column (A/R or A/P).
    pub control: &'static str,
    pub date: &'static str,
    pub has_due_date: bool,
    pub price: &'static str,
    /// The line's own account column, and the product column it falls back to.
    pub account: &'static str,
    pub fallback: &'static str,
    pub has_order_lines: bool,
    pub balances_view: &'static str,
    pub view_id: &'static str,
    /// `payment_status` (invoices and bills) or `application_status`.
    pub settlement: &'static str,
    /// Whether the detail lines (revenue or expense, and tax) are credits;
    /// the control line takes the other side.
    pub detail_is_credit: bool,
    pub detail_memo: &'static str,
    pub tax_memo: &'static str,
    pub control_memo: &'static str,
    /// (table, column) pairs whose rows referring to the document are
    /// applications, which block unposting.
    pub applications: &'static [(&'static str, &'static str)],
}

pub static SALES_INVOICES: Kind = Kind {
    path: "sales-invoices",
    noun: "Sales invoice",
    table: "sales_invoices",
    lines_table: "sales_invoice_lines",
    line_fk: "invoice_id",
    number: "invoice_number",
    party: "customer_id",
    party_table: "customers",
    control: "ar_account_id",
    date: "invoice_date",
    has_due_date: true,
    price: "unit_price",
    account: "revenue_account_id",
    fallback: "revenue_account_id",
    has_order_lines: true,
    balances_view: "sales_invoice_balances",
    view_id: "invoice_id",
    settlement: "payment_status",
    detail_is_credit: true,
    detail_memo: "Revenue",
    tax_memo: "Sales tax",
    control_memo: "Accounts receivable",
    applications: &[("payment_applications", "invoice_id"), ("sales_credit_applications", "invoice_id")],
};

pub static PURCHASE_BILLS: Kind = Kind {
    path: "purchase-bills",
    noun: "Purchase bill",
    table: "purchase_bills",
    lines_table: "purchase_bill_lines",
    line_fk: "bill_id",
    number: "bill_number",
    party: "supplier_id",
    party_table: "suppliers",
    control: "ap_account_id",
    date: "bill_date",
    has_due_date: true,
    price: "unit_cost",
    account: "expense_account_id",
    fallback: "inventory_account_id",
    has_order_lines: true,
    balances_view: "purchase_bill_balances",
    view_id: "bill_id",
    settlement: "payment_status",
    detail_is_credit: false,
    detail_memo: "Expense",
    tax_memo: "Input tax",
    control_memo: "Accounts payable",
    applications: &[("bill_applications", "bill_id"), ("purchase_credit_applications", "bill_id")],
};

pub static SALES_CREDIT_NOTES: Kind = Kind {
    path: "sales-credit-notes",
    noun: "Sales credit note",
    table: "sales_credit_notes",
    lines_table: "sales_credit_note_lines",
    line_fk: "credit_note_id",
    number: "credit_note_number",
    party: "customer_id",
    party_table: "customers",
    control: "ar_account_id",
    date: "credit_note_date",
    has_due_date: false,
    price: "unit_price",
    account: "revenue_account_id",
    fallback: "revenue_account_id",
    has_order_lines: false,
    balances_view: "sales_credit_note_balances",
    view_id: "credit_note_id",
    settlement: "application_status",
    detail_is_credit: false,
    detail_memo: "Revenue",
    tax_memo: "Sales tax",
    control_memo: "Accounts receivable",
    applications: &[("sales_credit_applications", "credit_note_id")],
};

pub static PURCHASE_CREDIT_NOTES: Kind = Kind {
    path: "purchase-credit-notes",
    noun: "Purchase credit note",
    table: "purchase_credit_notes",
    lines_table: "purchase_credit_note_lines",
    line_fk: "credit_note_id",
    number: "credit_note_number",
    party: "supplier_id",
    party_table: "suppliers",
    control: "ap_account_id",
    date: "credit_note_date",
    has_due_date: false,
    price: "unit_cost",
    account: "expense_account_id",
    fallback: "inventory_account_id",
    has_order_lines: false,
    balances_view: "purchase_credit_note_balances",
    view_id: "credit_note_id",
    settlement: "application_status",
    detail_is_credit: true,
    detail_memo: "Expense",
    tax_memo: "Input tax",
    control_memo: "Accounts payable",
    applications: &[("purchase_credit_applications", "credit_note_id")],
};

pub static KINDS: [&Kind; 4] = [&SALES_INVOICES, &PURCHASE_BILLS, &SALES_CREDIT_NOTES, &PURCHASE_CREDIT_NOTES];

/// A document as written: its header and its full line set.
#[derive(Debug, Default)]
pub struct DocumentInput {
    pub number: String,
    pub party_id: i64,
    pub date: String,
    pub due_date: Option<String>,
    pub currency_code: String,
    pub reference: Option<String>,
    pub memo: Option<String>,
    pub lines: Vec<LineInput>,
}

#[derive(Debug, Default)]
pub struct LineInput {
    pub product_id: Option<i64>,
    pub description: String,
    /// Decimals; absent means quantity 1, price 0, tax rate 0.
    pub quantity: Option<String>,
    pub price: Option<String>,
    pub account_id: Option<i64>,
    pub tax_code: Option<String>,
    pub tax_rate: Option<String>,
}

fn wrong_type(field: &str) -> Error {
    Error::bad_request(format!("invalid JSON body: {field} has the wrong type"))
}

/// A string field; absent or null reads as empty.
fn string(m: &Map<String, Value>, key: &str) -> Result<String> {
    Ok(optional_string(m, key)?.unwrap_or_default())
}

fn optional_string(m: &Map<String, Value>, key: &str) -> Result<Option<String>> {
    match m.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(wrong_type(key)),
    }
}

fn optional_int(m: &Map<String, Value>, key: &str) -> Result<Option<i64>> {
    match m.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_i64().map(Some).ok_or_else(|| wrong_type(key)),
    }
}

impl DocumentInput {
    /// Reads a request body, named as the kind names its fields. A field of
    /// the wrong JSON type is a 400, as a body that does not decode is.
    pub fn from_json(kind: &Kind, body: Value) -> Result<DocumentInput> {
        let Value::Object(m) = body else { return Err(Error::bad_request("invalid JSON body: expected an object")) };
        let lines = match m.get("lines") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(lines)) => lines
                .iter()
                .map(|line| {
                    let Value::Object(l) = line else { return Err(wrong_type("lines")) };
                    Ok(LineInput {
                        product_id: optional_int(l, "product_id")?,
                        description: string(l, "description")?,
                        quantity: optional_string(l, "quantity")?,
                        price: optional_string(l, kind.price)?,
                        account_id: optional_int(l, kind.account)?,
                        tax_code: optional_string(l, "tax_code")?,
                        tax_rate: optional_string(l, "tax_rate")?,
                    })
                })
                .collect::<Result<_>>()?,
            Some(_) => return Err(wrong_type("lines")),
        };
        Ok(DocumentInput {
            number: string(&m, kind.number)?,
            party_id: optional_int(&m, kind.party)?.unwrap_or(0),
            date: string(&m, kind.date)?,
            due_date: if kind.has_due_date { optional_string(&m, "due_date")? } else { None },
            currency_code: string(&m, "currency_code")?,
            reference: optional_string(&m, "reference")?,
            memo: optional_string(&m, "memo")?,
            lines,
        })
    }

    /// The 400s: the required header fields, and a description on every line.
    fn check(&self, kind: &Kind) -> Result<()> {
        let missing = |field: &str| Err(Error::bad_request(format!("{field} is required")));
        if self.number.is_empty() {
            return missing(kind.number);
        }
        if self.party_id <= 0 {
            return missing(kind.party);
        }
        if self.date.is_empty() {
            return missing(kind.date);
        }
        if self.currency_code.is_empty() {
            return missing("currency_code");
        }
        if let Some(i) = self.lines.iter().position(|l| l.description.is_empty()) {
            return Err(Error::bad_request(format!("line {}: description is required", i + 1)));
        }
        Ok(())
    }
}

/// A decimal that defaults when absent or empty.
fn or(value: &Option<String>, default: &str) -> String {
    value.clone().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
}

fn conflict(message: impl Into<String>) -> Error {
    Error::new(axum::http::StatusCode::CONFLICT, message)
}

async fn insert_lines(tx: &mut PgConnection, kind: &Kind, id: i64, lines: &[LineInput]) -> Result<()> {
    let sql = format!(
        "INSERT INTO {lines} ({fk}, line_no, product_id, description, quantity, {price}, {account}, tax_code, tax_rate)
         VALUES ($1, $2, $3::int8, $4, $5::text::numeric, $6::text::numeric, $7::int8, $8, $9::text::numeric)",
        lines = kind.lines_table,
        fk = kind.line_fk,
        price = kind.price,
        account = kind.account,
    );
    for (i, l) in lines.iter().enumerate() {
        sqlx::query(&sql)
            .bind(id as i32)
            .bind(i as i32 + 1)
            .bind(l.product_id)
            .bind(&l.description)
            .bind(or(&l.quantity, "1"))
            .bind(or(&l.price, "0"))
            .bind(l.account_id)
            .bind(&l.tax_code)
            .bind(or(&l.tax_rate, "0"))
            .execute(&mut *tx)
            .await?;
    }
    Ok(())
}

/// Creates the document as a draft, with its lines, and returns its id.
pub async fn create(pool: &PgPool, kind: &Kind, doc: DocumentInput) -> Result<i64> {
    doc.check(kind)?;
    let due = if kind.has_due_date { ", due_date" } else { "" };
    let due_value = if kind.has_due_date { ", $7::text::date" } else { "" };
    let sql = format!(
        "INSERT INTO {table} ({number}, {party}, {date}, currency_code, reference, memo{due})
         VALUES ($1, $2::int8, $3::text::date, $4, $5, $6{due_value}) RETURNING id",
        table = kind.table,
        number = kind.number,
        party = kind.party,
        date = kind.date,
    );
    let mut tx = pool.begin().await?;
    let mut insert = sqlx::query_scalar::<_, i32>(&sql)
        .bind(&doc.number)
        .bind(doc.party_id)
        .bind(&doc.date)
        .bind(&doc.currency_code)
        .bind(&doc.reference)
        .bind(&doc.memo);
    if kind.has_due_date {
        insert = insert.bind(&doc.due_date);
    }
    let id = i64::from(insert.fetch_one(&mut *tx).await?);
    insert_lines(&mut tx, kind, id, &doc.lines).await?;
    tx.commit().await?;
    Ok(id)
}

/// Why a write guarded by the draft status matched nothing: the document is
/// missing (404) or past draft (409).
async fn missing_or_not_draft(tx: &mut PgConnection, kind: &Kind, id: i64) -> Error {
    let sql = format!("SELECT status FROM {} WHERE id = $1::int8", kind.table);
    match sqlx::query_scalar::<_, String>(&sql).bind(id).fetch_optional(tx).await {
        Ok(None) => Error::not_found(),
        Ok(Some(status)) => conflict(format!("the document is {status}, not a draft")),
        Err(err) => err.into(),
    }
}

/// Replaces a draft's header and its full line set. A document produced
/// from an order may not be edited (409), though it may be deleted.
pub async fn update(pool: &PgPool, kind: &Kind, id: i64, doc: DocumentInput) -> Result<()> {
    doc.check(kind)?;
    let mut tx = pool.begin().await?;
    if kind.has_order_lines {
        let sql = format!(
            "SELECT EXISTS (SELECT 1 FROM {} WHERE {} = $1::int8 AND order_line_id IS NOT NULL)",
            kind.lines_table, kind.line_fk
        );
        if sqlx::query_scalar::<_, bool>(&sql).bind(id).fetch_one(&mut *tx).await? {
            return Err(conflict("the document was produced from an order and cannot be edited"));
        }
    }
    let due = if kind.has_due_date { ", due_date = $8::text::date" } else { "" };
    let sql = format!(
        "UPDATE {table} SET {number} = $2, {party} = $3::int8, {date} = $4::text::date, currency_code = $5,
             reference = $6, memo = $7{due}
         WHERE id = $1::int8 AND status = 'draft'",
        table = kind.table,
        number = kind.number,
        party = kind.party,
        date = kind.date,
    );
    let mut update = sqlx::query(&sql)
        .bind(id)
        .bind(&doc.number)
        .bind(doc.party_id)
        .bind(&doc.date)
        .bind(&doc.currency_code)
        .bind(&doc.reference)
        .bind(&doc.memo);
    if kind.has_due_date {
        update = update.bind(&doc.due_date);
    }
    if update.execute(&mut *tx).await?.rows_affected() == 0 {
        return Err(missing_or_not_draft(&mut tx, kind, id).await);
    }
    let sql = format!("DELETE FROM {} WHERE {} = $1::int8", kind.lines_table, kind.line_fk);
    sqlx::query(&sql).bind(id).execute(&mut *tx).await?;
    insert_lines(&mut tx, kind, id, &doc.lines).await?;
    tx.commit().await?;
    Ok(())
}

/// Deletes a draft; its lines cascade.
pub async fn delete(pool: &PgPool, kind: &Kind, id: i64) -> Result<()> {
    let mut tx = pool.begin().await?;
    let sql = format!("DELETE FROM {} WHERE id = $1::int8 AND status = 'draft'", kind.table);
    if sqlx::query(&sql).bind(id).execute(&mut *tx).await?.rows_affected() == 0 {
        return Err(missing_or_not_draft(&mut tx, kind, id).await);
    }
    tx.commit().await?;
    Ok(())
}

/// The SELECT of a kind's read shape, as JSON text, over `d` (the document)
/// and `b` (its balance view row).
fn read_sql(kind: &Kind) -> String {
    let due = if kind.has_due_date { "'due_date', d.due_date::text," } else { "" };
    format!(
        "SELECT json_build_object(
             'id', d.id, '{number}', d.{number}, '{party}', d.{party}, '{date}', d.{date}::text, {due}
             '{settlement}', b.{settlement},
             'currency_code', d.currency_code, 'status', d.status, 'total', d.total::text,
             'amount_applied', b.amount_applied::numeric(19,4)::text, 'balance', b.balance::numeric(19,4)::text,
             'journal_entry_id', d.journal_entry_id, 'reference', d.reference, 'memo', d.memo)::text
         FROM {table} d JOIN {view} b ON b.{view_id} = d.id",
        number = kind.number,
        party = kind.party,
        date = kind.date,
        settlement = kind.settlement,
        table = kind.table,
        view = kind.balances_view,
        view_id = kind.view_id,
    )
}

fn parse(json: String) -> Result<Value> {
    serde_json::from_str(&json).map_err(Error::internal)
}

/// Every document of the kind, newest first, then by id descending.
pub async fn list(pool: &PgPool, kind: &Kind) -> Result<Vec<Value>> {
    let sql = format!("{} ORDER BY d.{} DESC, d.id DESC", read_sql(kind), kind.date);
    sqlx::query_scalar::<_, String>(&sql).fetch_all(pool).await?.into_iter().map(parse).collect()
}

pub async fn get(pool: &PgPool, kind: &Kind, id: i64) -> Result<Value> {
    let sql = format!("{} WHERE d.id = $1::int8", read_sql(kind));
    parse(sqlx::query_scalar::<_, String>(&sql).bind(id).fetch_optional(pool).await?.ok_or_else(Error::not_found)?)
}

/// The document's lines in order, with their generated money; 404 for an
/// unknown document.
pub async fn lines(pool: &PgPool, kind: &Kind, id: i64) -> Result<Vec<Value>> {
    let exists = format!("SELECT EXISTS (SELECT 1 FROM {} WHERE id = $1::int8)", kind.table);
    if !sqlx::query_scalar::<_, bool>(&exists).bind(id).fetch_one(pool).await? {
        return Err(Error::not_found());
    }
    let order_line = if kind.has_order_lines { "order_line_id" } else { "NULL::int" };
    let sql = format!(
        "SELECT json_build_object(
             'line_no', line_no, 'product_id', product_id, 'description', description,
             'quantity', quantity::text, '{price}', {price}::text, 'tax_code', tax_code, 'tax_rate', tax_rate::text,
             'line_subtotal', line_subtotal::text, 'tax_amount', tax_amount::text, 'line_total', line_total::text,
             '{account}', {account}, 'order_line_id', {order_line})::text
         FROM {lines} WHERE {fk} = $1::int8 ORDER BY line_no",
        price = kind.price,
        account = kind.account,
        lines = kind.lines_table,
        fk = kind.line_fk,
    );
    sqlx::query_scalar::<_, String>(&sql).bind(id).fetch_all(pool).await?.into_iter().map(parse).collect()
}

/// Posts a draft to the general ledger (domain §4.2, §4.3) and returns the
/// journal entry's id. The checks run in the spec's order, and the whole
/// posting is one transaction.
pub async fn post(pool: &PgPool, kind: &Kind, id: i64) -> Result<i32> {
    let mut tx = pool.begin().await?;
    let sql = format!(
        "SELECT d.status, d.currency_code::text, d.{date}::text, d.{number}, p.{control}, d.total > 0
         FROM {table} d JOIN {parties} p ON p.id = d.{party}
         WHERE d.id = $1::int8",
        date = kind.date,
        number = kind.number,
        control = kind.control,
        table = kind.table,
        parties = kind.party_table,
        party = kind.party,
    );
    let (status, currency, date, number, control, has_total) =
        sqlx::query_as::<_, (String, String, String, String, Option<i32>, bool)>(&sql)
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(Error::not_found)?;
    if status != "draft" {
        return Err(conflict(format!("the document is {status}, not a draft")));
    }
    if !has_total {
        return Err(Error::unprocessable("the document's total is not positive"));
    }
    if control.is_none() {
        return Err(Error::unprocessable(format!("the {} has no {}", kind.party.trim_end_matches("_id"), kind.control)));
    }
    let sql = format!(
        "SELECT
             (SELECT count(*) FROM {lines} l LEFT JOIN products p ON p.id = l.product_id
              WHERE l.{fk} = $1::int8 AND l.line_subtotal <> 0 AND COALESCE(l.{account}, p.{fallback}) IS NULL),
             (SELECT count(*) FROM {lines} l LEFT JOIN tax_codes tc ON tc.code = l.tax_code
              WHERE l.{fk} = $1::int8 AND l.tax_amount <> 0 AND tc.tax_account_id IS NULL)",
        lines = kind.lines_table,
        fk = kind.line_fk,
        account = kind.account,
        fallback = kind.fallback,
    );
    let (no_account, no_tax_account) = sqlx::query_as::<_, (i64, i64)>(&sql).bind(id).fetch_one(&mut *tx).await?;
    if no_account > 0 {
        return Err(Error::unprocessable(format!("{no_account} line(s) resolve to no {}", kind.account)));
    }
    if no_tax_account > 0 {
        return Err(Error::unprocessable(format!("{no_tax_account} taxed line(s) have no tax code with a tax account")));
    }

    let period = posting::period_for_date(&mut tx, &date).await?;
    let entry = posting::create_entry(&mut tx, &date, period, &currency, &format!("{} {number}", kind.noun), Some(&number)).await?;

    // Detail lines, summed per account and per tax account, converted to
    // base at the entry's rate. An account netting negative takes the other
    // side; one netting zero gets no line.
    let (debit, credit) = if kind.detail_is_credit { ("-", "") } else { ("", "-") };
    let sql = format!(
        "WITH detail AS (
             SELECT 0 AS ord, COALESCE(l.{account}, p.{fallback}) AS account_id, sum(l.line_subtotal) AS amount,
                    '{detail_memo}' AS memo
             FROM {lines} l LEFT JOIN products p ON p.id = l.product_id
             WHERE l.{fk} = $2::int8 GROUP BY 2 HAVING sum(l.line_subtotal) <> 0
             UNION ALL
             SELECT 1, tc.tax_account_id, sum(l.tax_amount), '{tax_memo}'
             FROM {lines} l JOIN tax_codes tc ON tc.code = l.tax_code
             WHERE l.{fk} = $2::int8 AND l.tax_amount <> 0 GROUP BY 2 HAVING sum(l.tax_amount) <> 0
         ), based AS (
             SELECT *, round(amount * (SELECT exchange_rate FROM journal_entries WHERE id = $1), 4) AS base FROM detail
         )
         INSERT INTO journal_lines (journal_entry_id, line_no, account_id, debit, credit, memo, base_debit, base_credit)
         SELECT $1, row_number() OVER (ORDER BY ord, account_id), account_id,
                greatest({debit}amount, 0), greatest({credit}amount, 0), memo,
                greatest({debit}base, 0), greatest({credit}base, 0)
         FROM based",
        account = kind.account,
        fallback = kind.fallback,
        detail_memo = kind.detail_memo,
        tax_memo = kind.tax_memo,
        lines = kind.lines_table,
        fk = kind.line_fk,
    );
    sqlx::query(&sql).bind(entry).bind(id).execute(&mut *tx).await?;

    // The control line carries the document total; its base amount is the
    // net of the detail lines' base amounts, so the entry balances in base.
    let (amounts, base_net) = if kind.detail_is_credit {
        ("d.total, 0", "(SELECT COALESCE(sum(base_credit - base_debit), 0) FROM journal_lines WHERE journal_entry_id = $1), 0")
    } else {
        ("0, d.total", "0, (SELECT COALESCE(sum(base_debit - base_credit), 0) FROM journal_lines WHERE journal_entry_id = $1)")
    };
    let sql = format!(
        "INSERT INTO journal_lines (journal_entry_id, line_no, account_id, debit, credit, memo, base_debit, base_credit)
         SELECT $1, (SELECT COALESCE(max(line_no), 0) + 1 FROM journal_lines WHERE journal_entry_id = $1),
                p.{control}, {amounts}, '{control_memo}', {base_net}
         FROM {table} d JOIN {parties} p ON p.id = d.{party}
         WHERE d.id = $2::int8",
        control = kind.control,
        control_memo = kind.control_memo,
        table = kind.table,
        parties = kind.party_table,
        party = kind.party,
    );
    sqlx::query(&sql).bind(entry).bind(id).execute(&mut *tx).await?;

    let sql = format!("UPDATE {} SET status = 'posted', journal_entry_id = $1, period_id = $2 WHERE id = $3::int8", kind.table);
    sqlx::query(&sql).bind(entry).bind(period).bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(entry)
}

/// Reverses a posted document's entry and returns it to draft (domain
/// §4.4), returning the reversal's id. Refused (409) while anything is
/// applied to or from the document.
pub async fn unpost(pool: &PgPool, kind: &Kind, id: i64) -> Result<i32> {
    let mut tx = pool.begin().await?;
    let sql = format!("SELECT status, journal_entry_id FROM {} WHERE id = $1::int8", kind.table);
    let (status, entry) = sqlx::query_as::<_, (String, Option<i32>)>(&sql)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(Error::not_found)?;
    let Some(entry) = entry.filter(|_| status == "posted") else {
        return Err(conflict(format!("the document is {status}, not posted")));
    };
    for (table, column) in kind.applications {
        let sql = format!("SELECT EXISTS (SELECT 1 FROM {table} WHERE {column} = $1::int8)");
        if sqlx::query_scalar::<_, bool>(&sql).bind(id).fetch_one(&mut *tx).await? {
            return Err(conflict("the document has applications; unwind them first"));
        }
    }
    let reversal = posting::reverse_entry(&mut tx, entry).await?;
    let sql = format!("UPDATE {} SET status = 'draft', journal_entry_id = NULL, period_id = NULL WHERE id = $1::int8", kind.table);
    sqlx::query(&sql).bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(reversal)
}
