//! Applying payments and credit notes to invoices and bills (domain §5),
//! and the realized exchange difference an application can carry (§7.3).
//!
//! The four settling documents differ only in names, so each is described
//! once by a `Settler`, as the documents themselves are (src/documents.rs).
//! The schema guards every application: both documents posted, the same
//! party and currency, and neither over-applied.

use serde::Serialize;
use serde_json::Value;
use sqlx::{PgConnection, PgPool};

use crate::documents::{conflict, parse};
use crate::error::{Error, Result};
use crate::posting;

pub struct Settler {
    /// The settling document's collection under /api/.
    pub path: &'static str,
    pub table: &'static str,
    pub date: &'static str,
    /// What there is to apply: a payment's amount or a credit note's total.
    pub amount: &'static str,
    pub party: &'static str,
    pub party_table: &'static str,
    pub control: &'static str,
    pub applications: &'static str,
    /// The applications' columns naming the settler and the document.
    pub settler_column: &'static str,
    pub document_column: &'static str,
    pub document_table: &'static str,
    pub document_date: &'static str,
    pub document_number: &'static str,
    pub document_noun: &'static str,
    /// The SQL function giving how much of a document is settled, by
    /// payments and credit notes together.
    pub settled: &'static str,
    /// Whether the control account is A/R, where a positive exchange
    /// difference is a gain; on the A/P side it is a loss.
    pub receivable: bool,
}

pub static CUSTOMER_PAYMENT: Settler = Settler {
    path: "customer-payments",
    table: "customer_payments",
    date: "payment_date",
    amount: "amount",
    party: "customer_id",
    party_table: "customers",
    control: "ar_account_id",
    applications: "payment_applications",
    settler_column: "payment_id",
    document_column: "invoice_id",
    document_table: "sales_invoices",
    document_date: "invoice_date",
    document_number: "invoice_number",
    document_noun: "invoice",
    settled: "invoice_amount_settled",
    receivable: true,
};

pub static SUPPLIER_PAYMENT: Settler = Settler {
    path: "supplier-payments",
    table: "supplier_payments",
    date: "payment_date",
    amount: "amount",
    party: "supplier_id",
    party_table: "suppliers",
    control: "ap_account_id",
    applications: "bill_applications",
    settler_column: "payment_id",
    document_column: "bill_id",
    document_table: "purchase_bills",
    document_date: "bill_date",
    document_number: "bill_number",
    document_noun: "bill",
    settled: "bill_amount_settled",
    receivable: false,
};

pub static SALES_CREDIT_NOTE: Settler = Settler {
    path: "sales-credit-notes",
    table: "sales_credit_notes",
    date: "credit_note_date",
    amount: "total",
    party: "customer_id",
    party_table: "customers",
    control: "ar_account_id",
    applications: "sales_credit_applications",
    settler_column: "credit_note_id",
    document_column: "invoice_id",
    document_table: "sales_invoices",
    document_date: "invoice_date",
    document_number: "invoice_number",
    document_noun: "invoice",
    settled: "invoice_amount_settled",
    receivable: true,
};

pub static PURCHASE_CREDIT_NOTE: Settler = Settler {
    path: "purchase-credit-notes",
    table: "purchase_credit_notes",
    date: "credit_note_date",
    amount: "total",
    party: "supplier_id",
    party_table: "suppliers",
    control: "ap_account_id",
    applications: "purchase_credit_applications",
    settler_column: "credit_note_id",
    document_column: "bill_id",
    document_table: "purchase_bills",
    document_date: "bill_date",
    document_number: "bill_number",
    document_noun: "bill",
    settled: "bill_amount_settled",
    receivable: false,
};

/// The settler whose collection is `path`, if it is one.
pub fn settler(path: &str) -> Option<&'static Settler> {
    [&CUSTOMER_PAYMENT, &SUPPLIER_PAYMENT, &SALES_CREDIT_NOTE, &PURCHASE_CREDIT_NOTE].into_iter().find(|s| s.path == path)
}

/// One application made by `apply`.
#[derive(Debug, Serialize)]
pub struct Application {
    pub document_id: i32,
    pub amount_applied: String,
}

/// Allocates the settler's unapplied remainder across the party's open
/// documents in its currency, oldest first, each receiving the least of its
/// availability and what is left (domain §5.1). Then posts the realized
/// exchange difference of each new application (§7.3). Returns the
/// applications made, which may be none. The settler must be posted (409).
pub async fn apply(pool: &PgPool, s: &Settler, id: i64) -> Result<Vec<Application>> {
    let mut tx = pool.begin().await?;
    let sql = format!("SELECT status FROM {} WHERE id = $1::int8", s.table);
    let status = sqlx::query_scalar::<_, String>(&sql).bind(id).fetch_optional(&mut *tx).await?.ok_or_else(Error::not_found)?;
    if status != "posted" {
        return Err(conflict(format!("the document is {status}; only a posted one can be applied")));
    }
    let sql = format!(
        "WITH settler AS (
             SELECT s.id, s.{party} AS party, s.currency_code,
                    s.{amount} - COALESCE((SELECT sum(amount_applied) FROM {apps} WHERE {settler_col} = s.id), 0) AS remaining
             FROM {table} s WHERE s.id = $1::int8
         ), open AS (
             SELECT d.id, d.{doc_date} AS date, d.total - {settled}(d.id) AS available
             FROM {docs} d JOIN settler ON d.{party} = settler.party AND d.currency_code = settler.currency_code
             WHERE d.status = 'posted'
         ), ranked AS (
             SELECT id, date, available,
                    COALESCE(sum(available) OVER (ORDER BY date, id ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING), 0) AS prior
             FROM open WHERE available > 0
         ), made AS (
             INSERT INTO {apps} ({settler_col}, {doc_col}, amount_applied)
             SELECT (SELECT id FROM settler), r.id, LEAST(r.available, GREATEST((SELECT remaining FROM settler) - r.prior, 0))
             FROM ranked r
             WHERE LEAST(r.available, GREATEST((SELECT remaining FROM settler) - r.prior, 0)) > 0
             ORDER BY r.date, r.id
             RETURNING id, {doc_col} AS document_id, amount_applied
         )
         SELECT document_id, amount_applied::text FROM made ORDER BY id",
        party = s.party,
        amount = s.amount,
        apps = s.applications,
        settler_col = s.settler_column,
        doc_col = s.document_column,
        table = s.table,
        doc_date = s.document_date,
        settled = s.settled,
        docs = s.document_table,
    );
    let made = sqlx::query_as::<_, (i32, String)>(&sql).bind(id).fetch_all(&mut *tx).await?;
    post_exchange_differences(&mut tx, s, id).await?;
    tx.commit().await?;
    Ok(made.into_iter().map(|(document_id, amount_applied)| Application { document_id, amount_applied }).collect())
}

/// Posts an FX entry for each of the settler's applications whose applied
/// amount converts to different base amounts at the two documents' rates:
/// in the base currency, on the settler's date, between the party's
/// control account and the FX gain/loss account. With none configured, a
/// 422, which rolls the whole apply back.
async fn post_exchange_differences(tx: &mut PgConnection, s: &Settler, id: i64) -> Result<()> {
    let sql = format!(
        "SELECT a.id, d.{number}, p.{control}, s.{date}::text,
                (round(a.amount_applied * es.exchange_rate, 4) - round(a.amount_applied * ed.exchange_rate, 4))::text
         FROM {apps} a
         JOIN {table} s ON s.id = a.{settler_col}
         JOIN {parties} p ON p.id = s.{party}
         JOIN {docs} d ON d.id = a.{doc_col}
         JOIN journal_entries es ON es.id = s.journal_entry_id
         JOIN journal_entries ed ON ed.id = d.journal_entry_id
         WHERE a.{settler_col} = $1::int8 AND a.fx_journal_entry_id IS NULL
           AND round(a.amount_applied * es.exchange_rate, 4) <> round(a.amount_applied * ed.exchange_rate, 4)
         ORDER BY a.id",
        number = s.document_number,
        control = s.control,
        date = s.date,
        apps = s.applications,
        table = s.table,
        settler_col = s.settler_column,
        parties = s.party_table,
        party = s.party,
        docs = s.document_table,
        doc_col = s.document_column,
    );
    let differences = sqlx::query_as::<_, (i32, String, i32, String, String)>(&sql).bind(id).fetch_all(&mut *tx).await?;
    if differences.is_empty() {
        return Ok(());
    }
    let settings = sqlx::query!("SELECT base_currency, fx_gain_loss_account_id FROM gl_settings").fetch_one(&mut *tx).await?;
    let Some(fx_account) = settings.fx_gain_loss_account_id else {
        return Err(Error::unprocessable("settling at a different rate needs an FX gain/loss account in the settings"));
    };
    // Oriented so that a positive value debits the control account: the
    // difference itself on the A/R side, its negation on the A/P side.
    let sign = if s.receivable { 1 } else { -1 };
    for (application, number, control, date, difference) in differences {
        let period = posting::period_for_date(&mut *tx, &date).await?;
        let memo = format!("Exchange difference on settlement of {} {number}", s.document_noun);
        let entry = posting::create_entry(&mut *tx, &date, period, &settings.base_currency, &memo, Some(&number)).await?;
        // Both lines are in the base currency, so each base amount equals its amount.
        let lines = [(1, control, sign, "Settlement revaluation"), (2, fx_account, -sign, "Exchange gain (loss)")];
        for (line_no, account, orientation, line_memo) in lines {
            sqlx::query!(
                "INSERT INTO journal_lines (journal_entry_id, line_no, account_id, debit, credit, memo, base_debit, base_credit)
                 SELECT $1, $2, $3, greatest(v, 0), greatest(-v, 0), $4, greatest(v, 0), greatest(-v, 0)
                 FROM (SELECT $5::text::numeric * $6::int4 AS v) d",
                entry,
                line_no,
                account,
                line_memo,
                difference,
                orientation
            )
            .execute(&mut *tx)
            .await?;
        }
        let sql = format!("UPDATE {} SET fx_journal_entry_id = $1 WHERE id = $2", s.applications);
        sqlx::query(&sql).bind(entry).bind(application).execute(&mut *tx).await?;
    }
    Ok(())
}

/// The settler's applications in the order they were made; 404 for an
/// unknown settler.
pub async fn applications(pool: &PgPool, s: &Settler, id: i64) -> Result<Vec<Value>> {
    let sql = format!("SELECT EXISTS (SELECT 1 FROM {} WHERE id = $1::int8)", s.table);
    if !sqlx::query_scalar::<_, bool>(&sql).bind(id).fetch_one(pool).await? {
        return Err(Error::not_found());
    }
    let sql = format!(
        "SELECT json_build_object('document_id', a.{doc_col}, 'document_number', d.{number},
                                  'amount_applied', a.amount_applied::text)::text
         FROM {apps} a JOIN {docs} d ON d.id = a.{doc_col}
         WHERE a.{settler_col} = $1::int8 ORDER BY a.id",
        doc_col = s.document_column,
        number = s.document_number,
        apps = s.applications,
        docs = s.document_table,
        settler_col = s.settler_column,
    );
    sqlx::query_scalar::<_, String>(&sql).bind(id).fetch_all(pool).await?.into_iter().map(parse).collect()
}
