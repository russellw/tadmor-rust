//! The general ledger side of posting (domain §4, §7, §9.2): resolving the
//! accounting period, creating journal entries at the right exchange rate,
//! and reversing them. Every function runs inside the caller's transaction,
//! so a posting either fully happens or changes nothing; the schema checks
//! that each entry balances (in both currencies) when it commits.

use sqlx::PgConnection;

use crate::error::{Error, Result};

async fn open_period(tx: &mut PgConnection, date: &str) -> Result<Option<i32>> {
    Ok(sqlx::query_scalar!(
        "SELECT id FROM accounting_periods
         WHERE $1::text::date BETWEEN start_date AND end_date AND status = 'open'
         ORDER BY id LIMIT 1",
        date
    )
    .fetch_optional(tx)
    .await?)
}

/// The open accounting period covering `date` (a valid `YYYY-MM-DD`).
///
/// If no period covers the date but an open fiscal year does, the calendar
/// month is created as a period, named `YYYY-MM` and clipped to the year. A
/// closed period covering the date, no open year, or a month that would
/// overlap an existing period, is a 422.
pub async fn period_for_date(tx: &mut PgConnection, date: &str) -> Result<i32> {
    if let Some(id) = open_period(&mut *tx, date).await? {
        return Ok(id);
    }
    let covered = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM accounting_periods WHERE $1::text::date BETWEEN start_date AND end_date) AS "covered!""#,
        date
    )
    .fetch_one(&mut *tx)
    .await?;
    if !covered {
        // ON CONFLICT DO NOTHING absorbs a month that would overlap an
        // existing period; the re-select below then finds nothing.
        sqlx::query!(
            "INSERT INTO accounting_periods (fiscal_year_id, name, start_date, end_date)
             SELECT fy.id, to_char($1::text::date, 'YYYY-MM'),
                    GREATEST(date_trunc('month', $1::text::date)::date, fy.start_date),
                    LEAST((date_trunc('month', $1::text::date) + interval '1 month - 1 day')::date, fy.end_date)
             FROM fiscal_years fy
             WHERE $1::text::date BETWEEN fy.start_date AND fy.end_date AND fy.status = 'open'
             ORDER BY fy.start_date LIMIT 1
             ON CONFLICT DO NOTHING",
            date
        )
        .execute(&mut *tx)
        .await?;
        if let Some(id) = open_period(&mut *tx, date).await? {
            return Ok(id);
        }
    }
    Err(Error::unprocessable(format!("no open accounting period covers {date}")))
}

/// Inserts a posted journal entry header and returns its id. Its exchange
/// rate is 1 for the base currency, and otherwise the currency's latest rate
/// on or before the date; with no such rate, a 422.
pub async fn create_entry(
    tx: &mut PgConnection,
    date: &str,
    period_id: i32,
    currency: &str,
    memo: &str,
    reference: Option<&str>,
) -> Result<i32> {
    sqlx::query_scalar!(
        "INSERT INTO journal_entries (entry_date, period_id, currency_code, exchange_rate, memo, reference, status, posted_at)
         SELECT $1::text::date, $2, $3, r.rate, $4, $5, 'posted', now()
         FROM (SELECT CASE WHEN $3 = (SELECT base_currency FROM gl_settings) THEN 1::numeric
                           ELSE (SELECT rate FROM exchange_rates
                                 WHERE currency_code = $3 AND rate_date <= $1::text::date
                                 ORDER BY rate_date DESC LIMIT 1)
                      END AS rate) r
         WHERE r.rate IS NOT NULL
         RETURNING id",
        date,
        period_id,
        currency,
        memo,
        reference
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| Error::unprocessable(format!("no {currency} exchange rate on or before {date}")))
}

/// Posts the mirror of a journal entry: same date, currency and exchange
/// rate, every line's sides swapped in both currencies, linked back to the
/// original, which stays posted. An entry is reversed at most once, and
/// never while a line of it is matched on a bank statement (409); the
/// reversal needs an open period on the original date (422).
pub async fn reverse_entry(tx: &mut PgConnection, original: i32) -> Result<i32> {
    let entry = sqlx::query!(
        r#"SELECT entry_date::text AS "date!",
               EXISTS (SELECT 1 FROM journal_entries r WHERE r.reverses_entry_id = e.id) AS "reversed!",
               EXISTS (SELECT 1 FROM bank_statement_lines b JOIN journal_lines jl ON jl.id = b.journal_line_id
                       WHERE jl.journal_entry_id = e.id) AS "matched!"
           FROM journal_entries e WHERE id = $1"#,
        original
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(Error::not_found)?;
    if entry.reversed {
        return Err(Error::new(axum::http::StatusCode::CONFLICT, "the journal entry is already reversed"));
    }
    if entry.matched {
        return Err(Error::new(axum::http::StatusCode::CONFLICT, "the journal entry has lines matched on a bank statement"));
    }
    let period = period_for_date(&mut *tx, &entry.date).await?;
    // The reversal keeps is_closing, so reversing a year-end closing entry
    // stays out of the income statement like the entry it undoes, and the
    // original's rate, so it undoes exactly the base amounts posted.
    let reversal = sqlx::query_scalar!(
        "INSERT INTO journal_entries (entry_date, period_id, currency_code, exchange_rate, memo, reverses_entry_id,
             status, posted_at, is_closing)
         SELECT entry_date, $2, currency_code, exchange_rate, 'Reversal of journal entry ' || id, id, 'posted', now(), is_closing
         FROM journal_entries WHERE id = $1
         RETURNING id",
        original,
        period
    )
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query!(
        "INSERT INTO journal_lines (journal_entry_id, line_no, account_id, debit, credit, memo, base_debit, base_credit)
         SELECT $1, line_no, account_id, credit, debit, memo, base_credit, base_debit
         FROM journal_lines WHERE journal_entry_id = $2",
        reversal,
        original
    )
    .execute(&mut *tx)
    .await?;
    Ok(reversal)
}
