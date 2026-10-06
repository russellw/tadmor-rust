//! Stock movements (spec/api.md §5.12, domain §4): the quantity records of
//! inventory, and the receipts and issues that post to the ledger.
//!
//! The schema keeps the rules: the quantity's sign agrees with the type
//! (receipts and transfers in add, issues and transfers out remove,
//! adjustments go either way), it is never zero, the cost is not negative,
//! and the product is inventory-tracked and active. A movement's status is
//! derived: posted exactly when it has a journal entry.

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::error::{Error, Result};
use crate::master::require;
use crate::posting;

#[derive(Debug, Serialize)]
pub struct StockMovement {
    pub id: i32,
    pub product_id: i32,
    pub warehouse_id: i32,
    pub movement_date: String,
    pub movement_type: String,
    pub status: String,
    pub quantity: String,
    pub unit_cost: String,
    pub total_cost: String,
    pub reference: Option<String>,
    pub notes: Option<String>,
    pub journal_entry_id: Option<i32>,
    pub source_type: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct MovementInput {
    pub product_id: i64,
    pub warehouse_id: i64,
    pub movement_type: String,
    /// Defaults to today (the UTC date).
    pub movement_date: Option<String>,
    /// Signed decimal.
    pub quantity: String,
    /// Decimal; absent or empty means 0.
    pub unit_cost: Option<String>,
    pub reference: Option<String>,
    pub notes: Option<String>,
}

impl MovementInput {
    fn check(&self) -> Result<()> {
        if self.product_id <= 0 {
            return Err(Error::bad_request("product_id is required"));
        }
        if self.warehouse_id <= 0 {
            return Err(Error::bad_request("warehouse_id is required"));
        }
        require(&self.movement_type, "movement_type")?;
        require(&self.quantity, "quantity")
    }

    fn date(&self) -> Option<String> {
        self.movement_date.clone().filter(|d| !d.is_empty())
    }

    fn unit_cost(&self) -> String {
        self.unit_cost.clone().filter(|c| !c.is_empty()).unwrap_or_else(|| "0".to_string())
    }
}

fn conflict(message: &str) -> Error {
    Error::new(StatusCode::CONFLICT, message)
}

/// Newest first, then by id descending.
pub async fn list(pool: &PgPool) -> Result<Vec<StockMovement>> {
    Ok(sqlx::query_as!(
        StockMovement,
        r#"SELECT id, product_id, warehouse_id, movement_date::text AS "movement_date!", movement_type,
               CASE WHEN journal_entry_id IS NULL THEN 'draft' ELSE 'posted' END AS "status!",
               quantity::text AS "quantity!", unit_cost::text AS "unit_cost!", total_cost::text AS "total_cost!",
               reference, notes, journal_entry_id, source_type
           FROM stock_movements ORDER BY movement_date DESC, id DESC"#
    )
    .fetch_all(pool)
    .await?)
}

pub async fn get(pool: &PgPool, id: i64) -> Result<StockMovement> {
    sqlx::query_as!(
        StockMovement,
        r#"SELECT id, product_id, warehouse_id, movement_date::text AS "movement_date!", movement_type,
               CASE WHEN journal_entry_id IS NULL THEN 'draft' ELSE 'posted' END AS "status!",
               quantity::text AS "quantity!", unit_cost::text AS "unit_cost!", total_cost::text AS "total_cost!",
               reference, notes, journal_entry_id, source_type
           FROM stock_movements WHERE id = $1::int8"#,
        id
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(Error::not_found)
}

pub async fn create(pool: &PgPool, m: MovementInput) -> Result<i32> {
    m.check()?;
    Ok(sqlx::query_scalar!(
        "INSERT INTO stock_movements (product_id, warehouse_id, movement_type, movement_date, quantity, unit_cost, reference, notes)
         VALUES ($1::int8, $2::int8, $3, COALESCE($4::text::date, current_date), $5::text::numeric, $6::text::numeric, $7, $8)
         RETURNING id",
        m.product_id,
        m.warehouse_id,
        m.movement_type,
        m.date(),
        m.quantity,
        m.unit_cost(),
        m.reference,
        m.notes
    )
    .fetch_one(pool)
    .await?)
}

/// Replaces an unposted movement entered by hand. One posted, or produced
/// by order fulfilment, is a 409.
pub async fn update(pool: &PgPool, id: i64, m: MovementInput) -> Result<()> {
    m.check()?;
    let done = sqlx::query!(
        "UPDATE stock_movements SET product_id = $2::int8, warehouse_id = $3::int8, movement_type = $4,
             movement_date = COALESCE($5::text::date, current_date), quantity = $6::text::numeric,
             unit_cost = $7::text::numeric, reference = $8, notes = $9
         WHERE id = $1::int8 AND journal_entry_id IS NULL AND source_type IS NULL",
        id,
        m.product_id,
        m.warehouse_id,
        m.movement_type,
        m.date(),
        m.quantity,
        m.unit_cost(),
        m.reference,
        m.notes
    )
    .execute(pool)
    .await?;
    if done.rows_affected() == 0 {
        let existing = get(pool, id).await?;
        return Err(conflict(if existing.journal_entry_id.is_some() {
            "the movement is posted"
        } else {
            "the movement was produced by order fulfilment and cannot be edited"
        }));
    }
    Ok(())
}

/// Deletes an unposted movement, fulfilment ones included.
pub async fn delete(pool: &PgPool, id: i64) -> Result<()> {
    let done = sqlx::query!("DELETE FROM stock_movements WHERE id = $1::int8 AND journal_entry_id IS NULL", id)
        .execute(pool)
        .await?;
    if done.rows_affected() == 0 {
        get(pool, id).await?;
        return Err(conflict("the movement is posted"));
    }
    Ok(())
}

/// Posts a receipt or an issue (domain §4.2, §4.3) in the base currency:
/// an issue debits the product's COGS and credits its inventory; a receipt
/// debits its inventory and credits `credit_account`, which must be
/// postable and active. Returns the journal entry's id.
pub async fn post(pool: &PgPool, id: i64, credit_account: Option<i64>) -> Result<i32> {
    let mut tx = pool.begin().await?;
    let m = sqlx::query!(
        r#"SELECT sm.movement_type, sm.movement_date::text AS "date!", sm.journal_entry_id, sm.total_cost <> 0 AS "has_cost!",
               abs(sm.total_cost)::text AS "cost!", p.inventory_account_id, p.cogs_account_id
           FROM stock_movements sm JOIN products p ON p.id = sm.product_id
           WHERE sm.id = $1::int8"#,
        id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(Error::not_found)?;
    if m.journal_entry_id.is_some() {
        return Err(conflict("the movement is already posted"));
    }
    if m.movement_type != "receipt" && m.movement_type != "issue" {
        return Err(Error::unprocessable(format!("an {} does not post to the ledger; only receipts and issues do", m.movement_type)));
    }
    if !m.has_cost {
        return Err(Error::unprocessable("the movement's total cost is zero"));
    }
    let Some(inventory) = m.inventory_account_id else {
        return Err(Error::unprocessable("the product has no inventory account"));
    };
    let (debit, credit) = if m.movement_type == "issue" {
        let Some(cogs) = m.cogs_account_id else {
            return Err(Error::unprocessable("the product has no COGS account"));
        };
        (cogs, inventory)
    } else {
        let usable = match credit_account.filter(|a| *a > 0) {
            None => None,
            Some(account) => sqlx::query_scalar!(
                r#"SELECT (is_postable AND is_active) AS "usable!" FROM accounts WHERE id = $1::int8"#,
                account
            )
            .fetch_optional(&mut *tx)
            .await?
            .filter(|usable| *usable)
            .map(|_| account),
        };
        let Some(account) = usable else {
            return Err(Error::unprocessable("a receipt needs a credit_account_id naming a postable, active account"));
        };
        (inventory, i32::try_from(account).map_err(|_| Error::unprocessable("unknown credit account"))?)
    };
    let period = posting::period_for_date(&mut tx, &m.date).await?;
    let base = sqlx::query_scalar!("SELECT base_currency::text AS \"base!\" FROM gl_settings").fetch_one(&mut *tx).await?;
    let memo = if m.movement_type == "issue" { "Stock issue" } else { "Stock receipt" };
    let entry = posting::create_entry(&mut tx, &m.date, period, &base, memo, None).await?;
    for (line_no, account, is_debit) in [(1, debit, true), (2, credit, false)] {
        sqlx::query!(
            "INSERT INTO journal_lines (journal_entry_id, line_no, account_id, debit, credit, base_debit, base_credit)
             VALUES ($1, $2, $3, CASE WHEN $4 THEN $5::text::numeric ELSE 0 END, CASE WHEN $4 THEN 0 ELSE $5::text::numeric END,
                     CASE WHEN $4 THEN $5::text::numeric ELSE 0 END, CASE WHEN $4 THEN 0 ELSE $5::text::numeric END)",
            entry,
            line_no,
            account,
            is_debit,
            m.cost
        )
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query!("UPDATE stock_movements SET journal_entry_id = $1, period_id = $2 WHERE id = $3::int8", entry, period, id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(entry)
}

/// Reverses a posted movement's entry and unlinks it; the quantity record
/// stays as it is.
pub async fn unpost(pool: &PgPool, id: i64) -> Result<i32> {
    let mut tx = pool.begin().await?;
    let entry = sqlx::query_scalar!("SELECT journal_entry_id FROM stock_movements WHERE id = $1::int8", id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(Error::not_found)?
        .ok_or_else(|| conflict("the movement is not posted"))?;
    let reversal = posting::reverse_entry(&mut tx, entry).await?;
    sqlx::query!("UPDATE stock_movements SET journal_entry_id = NULL, period_id = NULL WHERE id = $1::int8", id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(reversal)
}
