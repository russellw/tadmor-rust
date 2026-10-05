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
