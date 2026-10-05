//! Fiscal years and accounting periods (spec/api.md §5.7, domain §9.1).
//!
//! The schema enforces the rules: unique year names, period names unique
//! within a year, no overlapping periods across all years (an exclusion
//! constraint, 422), end not before start, and no open period in a closed
//! year (a trigger, 422). Dates arrive as text and are cast in SQL, so an
//! invalid one is a data exception (422). Year-end close and reopen live
//! with posting.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::error::{Error, Result};
use crate::master::require;

#[derive(Debug, Serialize)]
pub struct FiscalYear {
    pub id: i32,
    pub name: String,
    pub start_date: String,
    pub end_date: String,
    pub status: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct FiscalYearInput {
    pub name: String,
    pub start_date: String,
    pub end_date: String,
}

impl FiscalYearInput {
    fn check(&self) -> Result<()> {
        require(&self.name, "name")?;
        require(&self.start_date, "start_date")?;
        require(&self.end_date, "end_date")
    }
}

pub async fn list_fiscal_years(pool: &PgPool) -> Result<Vec<FiscalYear>> {
    Ok(sqlx::query_as!(
        FiscalYear,
        r#"SELECT id, name, start_date::text AS "start_date!", end_date::text AS "end_date!", status
           FROM fiscal_years ORDER BY start_date, id"#
    )
    .fetch_all(pool)
    .await?)
}

pub async fn get_fiscal_year(pool: &PgPool, id: i64) -> Result<FiscalYear> {
    sqlx::query_as!(
        FiscalYear,
        r#"SELECT id, name, start_date::text AS "start_date!", end_date::text AS "end_date!", status
           FROM fiscal_years WHERE id = $1::int8"#,
        id
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(Error::not_found)
}

/// Creates the year open.
pub async fn create_fiscal_year(pool: &PgPool, y: FiscalYearInput) -> Result<i32> {
    y.check()?;
    Ok(sqlx::query_scalar!(
        "INSERT INTO fiscal_years (name, start_date, end_date) VALUES ($1, $2::text::date, $3::text::date) RETURNING id",
        y.name,
        y.start_date,
        y.end_date
    )
    .fetch_one(pool)
    .await?)
}

/// Edits the name and dates. Status is not settable here: open and closed
/// belong to year-end close and reopen.
pub async fn update_fiscal_year(pool: &PgPool, id: i64, y: FiscalYearInput) -> Result<()> {
    y.check()?;
    let done = sqlx::query!(
        "UPDATE fiscal_years SET name = $2, start_date = $3::text::date, end_date = $4::text::date WHERE id = $1::int8",
        id,
        y.name,
        y.start_date,
        y.end_date
    )
    .execute(pool)
    .await?;
    if done.rows_affected() == 0 { Err(Error::not_found()) } else { Ok(()) }
}

#[derive(Debug, Serialize)]
pub struct AccountingPeriod {
    pub id: i32,
    pub fiscal_year_id: i32,
    pub name: String,
    pub start_date: String,
    pub end_date: String,
    pub status: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct AccountingPeriodInput {
    pub fiscal_year_id: i64,
    pub name: String,
    pub start_date: String,
    pub end_date: String,
    /// `open` or `closed`; used by update, where absent means open.
    pub status: Option<String>,
}

impl AccountingPeriodInput {
    fn check(&self) -> Result<()> {
        if self.fiscal_year_id <= 0 {
            return Err(Error::bad_request("fiscal_year_id is required"));
        }
        require(&self.name, "name")?;
        require(&self.start_date, "start_date")?;
        require(&self.end_date, "end_date")
    }
}

pub async fn list_accounting_periods(pool: &PgPool) -> Result<Vec<AccountingPeriod>> {
    Ok(sqlx::query_as!(
        AccountingPeriod,
        r#"SELECT id, fiscal_year_id, name, start_date::text AS "start_date!", end_date::text AS "end_date!", status
           FROM accounting_periods ORDER BY start_date, id"#
    )
    .fetch_all(pool)
    .await?)
}

pub async fn get_accounting_period(pool: &PgPool, id: i64) -> Result<AccountingPeriod> {
    sqlx::query_as!(
        AccountingPeriod,
        r#"SELECT id, fiscal_year_id, name, start_date::text AS "start_date!", end_date::text AS "end_date!", status
           FROM accounting_periods WHERE id = $1::int8"#,
        id
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(Error::not_found)
}

/// Creates the period open.
pub async fn create_accounting_period(pool: &PgPool, p: AccountingPeriodInput) -> Result<i32> {
    p.check()?;
    Ok(sqlx::query_scalar!(
        "INSERT INTO accounting_periods (fiscal_year_id, name, start_date, end_date)
         VALUES ($1::int8, $2, $3::text::date, $4::text::date) RETURNING id",
        p.fiscal_year_id,
        p.name,
        p.start_date,
        p.end_date
    )
    .fetch_one(pool)
    .await?)
}

/// Edits the period, which is also how it is closed or reopened.
pub async fn update_accounting_period(pool: &PgPool, id: i64, p: AccountingPeriodInput) -> Result<()> {
    p.check()?;
    let status = p.status.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| "open".to_string());
    let done = sqlx::query!(
        "UPDATE accounting_periods SET fiscal_year_id = $2::int8, name = $3, start_date = $4::text::date,
             end_date = $5::text::date, status = $6
         WHERE id = $1::int8",
        id,
        p.fiscal_year_id,
        p.name,
        p.start_date,
        p.end_date,
        status
    )
    .execute(pool)
    .await?;
    if done.rows_affected() == 0 { Err(Error::not_found()) } else { Ok(()) }
}
