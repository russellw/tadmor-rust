//! Year-end close and reopen (spec/api.md §5.7, domain §9.3), both
//! administrator-only, each one transaction.

use axum::http::StatusCode;
use serde::Serialize;
use sqlx::PgPool;

use crate::error::{Error, Result};
use crate::posting;

#[derive(Debug, Serialize)]
pub struct Closed {
    /// Null when no revenue or expense account had a balance to sweep.
    pub closing_entry_id: Option<i32>,
    /// Null when a year already covers the next day, or its name is taken.
    pub next_fiscal_year_id: Option<i32>,
}

/// Closes an open year: sweeps every revenue and expense balance up to its
/// end into retained earnings with a closing entry, closes its periods and
/// itself, and rolls forward into a new year if none follows.
pub async fn close(pool: &PgPool, year: i64, retained_earnings: i64) -> Result<Closed> {
    let mut tx = pool.begin().await?;
    let y = sqlx::query!(
        r#"SELECT name, end_date::text AS "end_date!", status,
               EXISTS (SELECT 1 FROM fiscal_years e WHERE e.id <> y.id AND e.status = 'open' AND e.start_date < y.start_date)
                   AS "earlier_open!"
           FROM fiscal_years y WHERE id = $1::int8"#,
        year
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(Error::not_found)?;
    if y.status != "open" {
        return Err(Error::new(StatusCode::CONFLICT, format!("{} is {}, not open", y.name, y.status)));
    }
    if y.earlier_open {
        return Err(Error::unprocessable(format!("an earlier fiscal year than {} is still open", y.name)));
    }
    let usable = sqlx::query_scalar!(
        r#"SELECT (is_postable AND is_active AND account_type = 'equity') AS "usable!" FROM accounts WHERE id = $1::int8"#,
        retained_earnings
    )
    .fetch_optional(&mut *tx)
    .await?;
    if usable != Some(true) {
        return Err(Error::unprocessable("retained_earnings_account_id must be a postable, active equity account"));
    }

    // Revenue and expense balances in base up to the year end, closing
    // entries included, so a year already swept has nothing left.
    let to_sweep = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM (
               SELECT jl.account_id FROM journal_lines jl
               JOIN journal_entries je ON je.id = jl.journal_entry_id
               JOIN accounts a ON a.id = jl.account_id
               WHERE je.status = 'posted' AND a.account_type IN ('revenue', 'expense') AND je.entry_date <= $1::text::date
               GROUP BY jl.account_id HAVING sum(jl.base_debit - jl.base_credit) <> 0) b"#,
        y.end_date
    )
    .fetch_one(&mut *tx)
    .await?;
    let mut closing_entry = None;
    if to_sweep > 0 {
        // The period covering the end date: created if missing, and used
        // even if it is closed.
        let period = match sqlx::query!(
            "SELECT id, status FROM accounting_periods WHERE $1::text::date BETWEEN start_date AND end_date ORDER BY id LIMIT 1",
            y.end_date
        )
        .fetch_optional(&mut *tx)
        .await?
        {
            Some(p) => {
                if p.status == "closed" {
                    sqlx::query!("UPDATE accounting_periods SET status = 'open' WHERE id = $1", p.id).execute(&mut *tx).await?;
                }
                p.id
            }
            None => posting::period_for_date(&mut tx, &y.end_date).await?,
        };
        let entry = sqlx::query_scalar!(
            "INSERT INTO journal_entries (entry_date, period_id, currency_code, memo, reference, status, posted_at, is_closing)
             SELECT $1::text::date, $2, base_currency, 'Year-end close ' || $3, $3, 'posted', now(), true FROM gl_settings
             RETURNING id",
            y.end_date,
            period,
            y.name
        )
        .fetch_one(&mut *tx)
        .await?;
        // One line per account on the side that zeroes it, and the net to
        // retained earnings: a credit for income, a debit for a loss.
        sqlx::query!(
            "WITH balance AS (
                 SELECT jl.account_id, sum(jl.base_debit - jl.base_credit) AS amount
                 FROM journal_lines jl
                 JOIN journal_entries je ON je.id = jl.journal_entry_id
                 JOIN accounts a ON a.id = jl.account_id
                 WHERE je.status = 'posted' AND a.account_type IN ('revenue', 'expense') AND je.entry_date <= $1::text::date
                 GROUP BY jl.account_id HAVING sum(jl.base_debit - jl.base_credit) <> 0
             ), lines AS (
                 SELECT 0 AS ord, account_id, greatest(-amount, 0) AS debit, greatest(amount, 0) AS credit,
                        'Year-end close' AS memo
                 FROM balance
                 UNION ALL
                 SELECT 1, $3::int8::int4, greatest(total, 0), greatest(-total, 0), 'Net income (loss) for ' || $4
                 FROM (SELECT sum(amount) AS total FROM balance) t WHERE total <> 0
             )
             INSERT INTO journal_lines (journal_entry_id, line_no, account_id, debit, credit, memo, base_debit, base_credit)
             SELECT $2, row_number() OVER (ORDER BY ord, account_id), account_id, debit, credit, memo, debit, credit
             FROM lines",
            y.end_date,
            entry,
            retained_earnings,
            y.name
        )
        .execute(&mut *tx)
        .await?;
        closing_entry = Some(entry);
    }

    sqlx::query!("UPDATE accounting_periods SET status = 'closed' WHERE fiscal_year_id = $1::int8 AND status = 'open'", year)
        .execute(&mut *tx)
        .await?;
    sqlx::query!("UPDATE fiscal_years SET status = 'closed', closing_entry_id = $2 WHERE id = $1::int8", year, closing_entry)
        .execute(&mut *tx)
        .await?;
    // Roll forward: a year from the next day, named after its end's calendar year.
    let next = sqlx::query_scalar!(
        "INSERT INTO fiscal_years (name, start_date, end_date)
         SELECT 'FY' || to_char(($1::text::date + 1 + interval '1 year' - interval '1 day')::date, 'YYYY'),
                $1::text::date + 1, ($1::text::date + 1 + interval '1 year' - interval '1 day')::date
         WHERE NOT EXISTS (SELECT 1 FROM fiscal_years WHERE $1::text::date + 1 BETWEEN start_date AND end_date)
         ON CONFLICT (name) DO NOTHING
         RETURNING id",
        y.end_date
    )
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Closed { closing_entry_id: closing_entry, next_fiscal_year_id: next })
}

/// Reopens a closed year with no later closed year: sets it open, reopens
/// the period holding its closing entry, and reverses that entry. Returns
/// the reversal's id, or None if the close posted nothing.
pub async fn reopen(pool: &PgPool, year: i64) -> Result<Option<i32>> {
    let mut tx = pool.begin().await?;
    let y = sqlx::query!(
        r#"SELECT name, status, closing_entry_id,
               EXISTS (SELECT 1 FROM fiscal_years l WHERE l.status = 'closed' AND l.start_date > y.start_date) AS "later_closed!"
           FROM fiscal_years y WHERE id = $1::int8"#,
        year
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(Error::not_found)?;
    if y.status != "closed" {
        return Err(Error::new(StatusCode::CONFLICT, format!("{} is {}, not closed", y.name, y.status)));
    }
    if y.later_closed {
        return Err(Error::unprocessable(format!("a fiscal year later than {} is closed", y.name)));
    }
    sqlx::query!("UPDATE fiscal_years SET status = 'open', closing_entry_id = NULL WHERE id = $1::int8", year)
        .execute(&mut *tx)
        .await?;
    let mut reversal = None;
    if let Some(entry) = y.closing_entry_id {
        sqlx::query!(
            "UPDATE accounting_periods SET status = 'open'
             WHERE id = (SELECT period_id FROM journal_entries WHERE id = $1) AND status = 'closed'",
            entry
        )
        .execute(&mut *tx)
        .await?;
        reversal = Some(posting::reverse_entry(&mut tx, entry).await?);
    }
    tx.commit().await?;
    Ok(reversal)
}
