//! Ledger settings and exchange rates (spec/api.md §5.8, domain §7.1).
//!
//! The schema keeps the single settings row, refuses an unknown currency
//! (a foreign key, 422), and refuses a change of base currency once any
//! journal entry exists (a trigger, 422). Exchange rates are positive and
//! unique per currency and date.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::error::{Error, Result};
use crate::master::require;

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    pub base_currency: String,
    pub fx_gain_loss_account_id: Option<i64>,
}

pub async fn get_settings(pool: &PgPool) -> Result<Settings> {
    Ok(sqlx::query_as!(
        Settings,
        r#"SELECT base_currency, fx_gain_loss_account_id::int8 AS fx_gain_loss_account_id FROM gl_settings"#
    )
    .fetch_one(pool)
    .await?)
}

/// Replaces the settings, upper-casing the currency. The FX gain/loss
/// account, if any, must be postable and active.
pub async fn update_settings(pool: &PgPool, s: Settings) -> Result<()> {
    require(&s.base_currency, "base_currency")?;
    if let Some(account) = s.fx_gain_loss_account_id {
        let usable = sqlx::query_scalar!(
            r#"SELECT (is_postable AND is_active) AS "usable!" FROM accounts WHERE id = $1::int8"#,
            account
        )
        .fetch_optional(pool)
        .await?;
        if usable != Some(true) {
            return Err(Error::unprocessable("fx_gain_loss_account_id must be a postable, active account"));
        }
    }
    sqlx::query!(
        "UPDATE gl_settings SET base_currency = upper($1), fx_gain_loss_account_id = $2::int8",
        s.base_currency,
        s.fx_gain_loss_account_id
    )
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct ExchangeRate {
    pub currency_code: String,
    pub rate_date: String,
    pub rate: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ExchangeRateInput {
    pub currency_code: String,
    pub rate_date: String,
    pub rate: String,
}

/// The key of a rate, as stored.
#[derive(Debug, Serialize)]
pub struct ExchangeRateKey {
    pub currency_code: String,
    pub rate_date: String,
}

/// Ordered by currency, then newest first. Rates are shown with trailing
/// zeros trimmed.
pub async fn list_exchange_rates(pool: &PgPool) -> Result<Vec<ExchangeRate>> {
    Ok(sqlx::query_as!(
        ExchangeRate,
        r#"SELECT currency_code, rate_date::text AS "rate_date!", trim_scale(rate)::text AS "rate!"
           FROM exchange_rates ORDER BY currency_code, rate_date DESC"#
    )
    .fetch_all(pool)
    .await?)
}

pub async fn create_exchange_rate(pool: &PgPool, r: ExchangeRateInput) -> Result<ExchangeRateKey> {
    require(&r.currency_code, "currency_code")?;
    require(&r.rate_date, "rate_date")?;
    require(&r.rate, "rate")?;
    Ok(sqlx::query_as!(
        ExchangeRateKey,
        r#"INSERT INTO exchange_rates (currency_code, rate_date, rate)
           VALUES (upper($1), $2::text::date, $3::text::numeric)
           RETURNING currency_code, rate_date::text AS "rate_date!""#,
        r.currency_code,
        r.rate_date,
        r.rate
    )
    .fetch_one(pool)
    .await?)
}

/// Sets the rate for an existing key. A malformed date in the path matches
/// nothing (404).
pub async fn update_exchange_rate(pool: &PgPool, currency: &str, date: &str, rate: &str) -> Result<()> {
    require(rate, "rate")?;
    let done = sqlx::query!(
        "UPDATE exchange_rates SET rate = $3::text::numeric
         WHERE currency_code = upper($1) AND rate_date::text = $2",
        currency,
        date,
        rate
    )
    .execute(pool)
    .await?;
    if done.rows_affected() == 0 { Err(Error::not_found()) } else { Ok(()) }
}

pub async fn delete_exchange_rate(pool: &PgPool, currency: &str, date: &str) -> Result<()> {
    let done = sqlx::query!(
        "DELETE FROM exchange_rates WHERE currency_code = upper($1) AND rate_date::text = $2",
        currency,
        date
    )
    .execute(pool)
    .await?;
    if done.rows_affected() == 0 { Err(Error::not_found()) } else { Ok(()) }
}
