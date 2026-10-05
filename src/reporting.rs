//! Reports over posted journal entries (spec/api.md §5.14), all in the base
//! currency.

use serde::Serialize;
use sqlx::PgPool;

use crate::error::{Error, Result};

#[derive(Debug, Serialize)]
pub struct LedgerRow {
    pub journal_entry_id: i32,
    pub entry_date: String,
    pub reference: Option<String>,
    pub memo: Option<String>,
    pub currency_code: String,
    pub debit: String,
    pub credit: String,
    pub base_debit: String,
    pub base_credit: String,
}

/// An account's posted lines, within optional inclusive date bounds (each
/// already a valid `YYYY-MM-DD`), ordered by entry date, entry, then line.
pub async fn account_ledger(pool: &PgPool, account_id: i64, from: Option<&str>, to: Option<&str>) -> Result<Vec<LedgerRow>> {
    let exists = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM accounts WHERE id = $1::int8) AS "exists!""#, account_id)
        .fetch_one(pool)
        .await?;
    if !exists {
        return Err(Error::not_found());
    }
    Ok(sqlx::query_as!(
        LedgerRow,
        r#"SELECT je.id AS journal_entry_id, je.entry_date::text AS "entry_date!", je.reference,
               COALESCE(jl.memo, je.memo) AS memo, je.currency_code,
               jl.debit::text AS "debit!", jl.credit::text AS "credit!",
               jl.base_debit::text AS "base_debit!", jl.base_credit::text AS "base_credit!"
           FROM journal_lines jl
           JOIN journal_entries je ON je.id = jl.journal_entry_id
           WHERE jl.account_id = $1::int8 AND je.status = 'posted'
             AND ($2::text IS NULL OR je.entry_date >= $2::text::date)
             AND ($3::text IS NULL OR je.entry_date <= $3::text::date)
           ORDER BY je.entry_date, je.id, jl.line_no"#,
        account_id,
        from,
        to
    )
    .fetch_all(pool)
    .await?)
}

#[derive(Debug, Serialize)]
pub struct JournalEntry {
    pub id: i32,
    pub entry_date: String,
    pub currency_code: String,
    pub exchange_rate: String,
    pub reference: Option<String>,
    pub memo: Option<String>,
    pub status: String,
    pub lines: Vec<JournalLine>,
}

#[derive(Debug, Serialize)]
pub struct JournalLine {
    pub line_no: i32,
    pub account_id: i32,
    pub account_code: String,
    pub account_name: String,
    pub memo: Option<String>,
    pub debit: String,
    pub credit: String,
    pub base_debit: String,
    pub base_credit: String,
}

pub async fn journal_entry(pool: &PgPool, id: i64) -> Result<JournalEntry> {
    let e = sqlx::query!(
        r#"SELECT id, entry_date::text AS "entry_date!", currency_code, trim_scale(exchange_rate)::text AS "exchange_rate!",
               reference, memo, status
           FROM journal_entries WHERE id = $1::int8"#,
        id
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(Error::not_found)?;
    let lines = sqlx::query_as!(
        JournalLine,
        r#"SELECT jl.line_no, jl.account_id, a.code AS account_code, a.name AS account_name, jl.memo,
               jl.debit::text AS "debit!", jl.credit::text AS "credit!",
               jl.base_debit::text AS "base_debit!", jl.base_credit::text AS "base_credit!"
           FROM journal_lines jl JOIN accounts a ON a.id = jl.account_id
           WHERE jl.journal_entry_id = $1 ORDER BY jl.line_no"#,
        e.id
    )
    .fetch_all(pool)
    .await?;
    Ok(JournalEntry {
        id: e.id,
        entry_date: e.entry_date,
        currency_code: e.currency_code,
        exchange_rate: e.exchange_rate,
        reference: e.reference,
        memo: e.memo,
        status: e.status,
        lines,
    })
}

#[derive(Debug, Serialize)]
pub struct TrialBalanceRow {
    pub account_id: i32,
    pub code: String,
    pub name: String,
    pub account_type: String,
    pub total_debit: String,
    pub total_credit: String,
    /// Debit-positive.
    pub balance: String,
}

/// Every account, with or without activity, ordered by code.
pub async fn trial_balance(pool: &PgPool) -> Result<Vec<TrialBalanceRow>> {
    Ok(sqlx::query_as!(
        TrialBalanceRow,
        r#"SELECT account_id AS "account_id!", code AS "code!", name AS "name!", account_type AS "account_type!",
               total_debit::numeric(19,4)::text AS "total_debit!", total_credit::numeric(19,4)::text AS "total_credit!",
               balance::numeric(19,4)::text AS "balance!"
           FROM trial_balance ORDER BY code"#
    )
    .fetch_all(pool)
    .await?)
}
