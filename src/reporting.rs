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

/// A report row for one account, in its natural sign.
#[derive(Debug, Serialize)]
pub struct ActivityRow {
    pub account_id: i32,
    pub code: String,
    pub name: String,
    pub account_type: String,
    pub amount: String,
}

/// Revenue and expense accounts with lines in range, closing entries
/// excluded: revenue credit-positive, expenses debit-positive.
pub async fn profit_and_loss(pool: &PgPool, from: Option<&str>, to: Option<&str>) -> Result<Vec<ActivityRow>> {
    Ok(sqlx::query_as!(
        ActivityRow,
        r#"SELECT a.id AS account_id, a.code, a.name, a.account_type,
               sum(CASE WHEN a.account_type = 'revenue' THEN jl.base_credit - jl.base_debit
                        ELSE jl.base_debit - jl.base_credit END)::numeric(19,4)::text AS "amount!"
           FROM journal_lines jl
           JOIN journal_entries je ON je.id = jl.journal_entry_id
           JOIN accounts a ON a.id = jl.account_id
           WHERE je.status = 'posted' AND NOT je.is_closing AND a.account_type IN ('revenue', 'expense')
             AND ($1::text IS NULL OR je.entry_date >= $1::text::date)
             AND ($2::text IS NULL OR je.entry_date <= $2::text::date)
           GROUP BY a.id ORDER BY a.code"#,
        from,
        to
    )
    .fetch_all(pool)
    .await?)
}

#[derive(Debug, Serialize)]
pub struct BalanceSheet {
    pub rows: Vec<ActivityRow>,
    /// Revenue less expenses up to the date, closing entries included.
    pub current_earnings: String,
}

/// Asset, liability and equity accounts with lines on or before the date:
/// assets debit-positive, the others credit-positive.
pub async fn balance_sheet(pool: &PgPool, as_of: Option<&str>) -> Result<BalanceSheet> {
    let rows = sqlx::query_as!(
        ActivityRow,
        r#"SELECT a.id AS account_id, a.code, a.name, a.account_type,
               sum(CASE WHEN a.account_type = 'asset' THEN jl.base_debit - jl.base_credit
                        ELSE jl.base_credit - jl.base_debit END)::numeric(19,4)::text AS "amount!"
           FROM journal_lines jl
           JOIN journal_entries je ON je.id = jl.journal_entry_id
           JOIN accounts a ON a.id = jl.account_id
           WHERE je.status = 'posted' AND a.account_type IN ('asset', 'liability', 'equity')
             AND ($1::text IS NULL OR je.entry_date <= $1::text::date)
           GROUP BY a.id ORDER BY a.code"#,
        as_of
    )
    .fetch_all(pool)
    .await?;
    let current_earnings = sqlx::query_scalar!(
        r#"SELECT COALESCE(sum(jl.base_credit - jl.base_debit), 0)::numeric(19,4)::text AS "earnings!"
           FROM journal_lines jl
           JOIN journal_entries je ON je.id = jl.journal_entry_id
           JOIN accounts a ON a.id = jl.account_id
           WHERE je.status = 'posted' AND a.account_type IN ('revenue', 'expense')
             AND ($1::text IS NULL OR je.entry_date <= $1::text::date)"#,
        as_of
    )
    .fetch_one(pool)
    .await?;
    Ok(BalanceSheet { rows, current_earnings })
}

#[derive(Debug, Serialize)]
pub struct CashFlowRow {
    pub account_id: i32,
    pub code: String,
    pub name: String,
    pub activity: String,
    pub amount: String,
}

#[derive(Debug, Serialize)]
pub struct CashFlow {
    pub net_income: String,
    pub rows: Vec<CashFlowRow>,
    pub net_cash_flow: String,
    pub opening_cash: String,
    pub closing_cash: String,
}

/// The indirect-method cash flow statement (domain §10): net income, then
/// the movement of every non-cash balance-sheet account (a source of cash
/// positive), and the cash accounts' opening, movement and closing.
pub async fn cash_flow(pool: &PgPool, from: Option<&str>, to: Option<&str>) -> Result<CashFlow> {
    let net_income = sqlx::query_scalar!(
        r#"SELECT COALESCE(sum(jl.base_credit - jl.base_debit), 0)::numeric(19,4)::text AS "net!"
           FROM journal_lines jl
           JOIN journal_entries je ON je.id = jl.journal_entry_id
           JOIN accounts a ON a.id = jl.account_id
           WHERE je.status = 'posted' AND NOT je.is_closing AND a.account_type IN ('revenue', 'expense')
             AND ($1::text IS NULL OR je.entry_date >= $1::text::date)
             AND ($2::text IS NULL OR je.entry_date <= $2::text::date)"#,
        from,
        to
    )
    .fetch_one(pool)
    .await?;
    let rows = sqlx::query_as!(
        CashFlowRow,
        r#"SELECT a.id AS account_id, a.code, a.name, a.cash_flow_activity AS activity,
               sum(jl.base_credit - jl.base_debit)::numeric(19,4)::text AS "amount!"
           FROM journal_lines jl
           JOIN journal_entries je ON je.id = jl.journal_entry_id
           JOIN accounts a ON a.id = jl.account_id
           WHERE je.status = 'posted' AND NOT je.is_closing AND NOT a.is_cash
             AND a.account_type IN ('asset', 'liability', 'equity')
             AND ($1::text IS NULL OR je.entry_date >= $1::text::date)
             AND ($2::text IS NULL OR je.entry_date <= $2::text::date)
           GROUP BY a.id ORDER BY a.code"#,
        from,
        to
    )
    .fetch_all(pool)
    .await?;
    let cash = sqlx::query!(
        r#"SELECT
               COALESCE(sum(jl.base_debit - jl.base_credit)
                   FILTER (WHERE $1::text IS NOT NULL AND je.entry_date < $1::text::date), 0)::numeric(19,4)::text AS "opening!",
               COALESCE(sum(jl.base_debit - jl.base_credit)
                   FILTER (WHERE ($1::text IS NULL OR je.entry_date >= $1::text::date)
                             AND ($2::text IS NULL OR je.entry_date <= $2::text::date)), 0)::numeric(19,4)::text AS "movement!",
               COALESCE(sum(jl.base_debit - jl.base_credit)
                   FILTER (WHERE $2::text IS NULL OR je.entry_date <= $2::text::date), 0)::numeric(19,4)::text AS "closing!"
           FROM journal_lines jl
           JOIN journal_entries je ON je.id = jl.journal_entry_id
           JOIN accounts a ON a.id = jl.account_id
           WHERE je.status = 'posted' AND a.is_cash"#,
        from,
        to
    )
    .fetch_one(pool)
    .await?;
    Ok(CashFlow { net_income, rows, net_cash_flow: cash.movement, opening_cash: cash.opening, closing_cash: cash.closing })
}

#[derive(Debug, Serialize)]
pub struct AgingRow {
    pub party_id: i32,
    pub party_name: String,
    pub total_outstanding: String,
    pub not_yet_due: String,
    pub days_1_30: String,
    pub days_31_60: String,
    pub days_61_90: String,
    pub days_over_90: String,
}

/// Per customer with a positive posted balance, bucketed by due date
/// against today (the UTC date, since every session runs in UTC).
pub async fn ar_aging(pool: &PgPool) -> Result<Vec<AgingRow>> {
    Ok(sqlx::query_as!(
        AgingRow,
        r#"SELECT a.customer_id AS "party_id!", o.name AS party_name,
               COALESCE(a.total_outstanding, 0)::numeric(19,4)::text AS "total_outstanding!",
               COALESCE(a.not_yet_due, 0)::numeric(19,4)::text AS "not_yet_due!",
               COALESCE(a.days_1_30, 0)::numeric(19,4)::text AS "days_1_30!",
               COALESCE(a.days_31_60, 0)::numeric(19,4)::text AS "days_31_60!",
               COALESCE(a.days_61_90, 0)::numeric(19,4)::text AS "days_61_90!",
               COALESCE(a.days_over_90, 0)::numeric(19,4)::text AS "days_over_90!"
           FROM ar_aging a
           JOIN customers c ON c.id = a.customer_id
           JOIN organizations o ON o.id = c.organization_id
           ORDER BY a.customer_id"#
    )
    .fetch_all(pool)
    .await?)
}

/// Per supplier, as `ar_aging` is per customer.
pub async fn ap_aging(pool: &PgPool) -> Result<Vec<AgingRow>> {
    Ok(sqlx::query_as!(
        AgingRow,
        r#"SELECT a.supplier_id AS "party_id!", o.name AS party_name,
               COALESCE(a.total_outstanding, 0)::numeric(19,4)::text AS "total_outstanding!",
               COALESCE(a.not_yet_due, 0)::numeric(19,4)::text AS "not_yet_due!",
               COALESCE(a.days_1_30, 0)::numeric(19,4)::text AS "days_1_30!",
               COALESCE(a.days_31_60, 0)::numeric(19,4)::text AS "days_31_60!",
               COALESCE(a.days_61_90, 0)::numeric(19,4)::text AS "days_61_90!",
               COALESCE(a.days_over_90, 0)::numeric(19,4)::text AS "days_over_90!"
           FROM ap_aging a
           JOIN suppliers s ON s.id = a.supplier_id
           JOIN organizations o ON o.id = s.organization_id
           ORDER BY a.supplier_id"#
    )
    .fetch_all(pool)
    .await?)
}

#[derive(Debug, Serialize)]
pub struct ValuationRow {
    pub product_id: i32,
    pub sku: String,
    pub name: String,
    pub qty_on_hand: String,
    pub value_on_hand: String,
    pub avg_unit_cost: String,
}

/// Per product with movements, posted or not, across all warehouses.
pub async fn inventory_valuation(pool: &PgPool) -> Result<Vec<ValuationRow>> {
    Ok(sqlx::query_as!(
        ValuationRow,
        r#"SELECT v.product_id AS "product_id!", p.sku, p.name,
               v.qty_on_hand::numeric(19,4)::text AS "qty_on_hand!",
               v.value_on_hand::numeric(19,4)::text AS "value_on_hand!",
               v.avg_unit_cost::numeric(19,4)::text AS "avg_unit_cost!"
           FROM stock_valuation v JOIN products p ON p.id = v.product_id
           ORDER BY p.sku"#
    )
    .fetch_all(pool)
    .await?)
}
