//! The choices pickers offer (spec/domain.md §13: "a picker over active
//! records for each reference"), and the names lists show for ids.

use std::collections::HashMap;

use sqlx::PgPool;

use crate::error::Result;

/// (value, label) pairs for a select.
pub type Options = Vec<(String, String)>;

/// Prepends a blank choice, and keeps `current` choosable even if it is no
/// longer active, so editing a record never silently drops a reference.
pub fn with_blank(blank: &str, mut options: Options, current: &str) -> Options {
    if !current.is_empty() && !options.iter().any(|(v, _)| v == current) {
        options.push((current.to_string(), current.to_string()));
    }
    options.insert(0, (String::new(), blank.to_string()));
    options
}

/// Which accounts a picker offers.
#[derive(Clone, Copy)]
pub enum Accounts {
    /// Every active account (parents).
    Active,
    /// Active, postable accounts (anything that carries journal lines).
    Postable,
    /// Active, postable cash accounts (bank statements, payments).
    Cash,
    /// Active, postable equity accounts (retained earnings).
    Equity,
}

pub async fn accounts(pool: &PgPool, which: Accounts) -> Result<Options> {
    let rows = sqlx::query!(
        "SELECT id, code, name, is_postable, is_cash, account_type FROM accounts WHERE is_active ORDER BY code"
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter(|a| match which {
            Accounts::Active => true,
            Accounts::Postable => a.is_postable,
            Accounts::Cash => a.is_postable && a.is_cash,
            Accounts::Equity => a.is_postable && a.account_type == "equity",
        })
        .map(|a| (a.id.to_string(), format!("{} {}", a.code, a.name)))
        .collect())
}

pub async fn organizations(pool: &PgPool) -> Result<Options> {
    Ok(sqlx::query!("SELECT id, name FROM organizations ORDER BY name")
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|o| (o.id.to_string(), o.name))
        .collect())
}

/// Active customers (`customers`) or suppliers (anything else), by name.
pub async fn parties(pool: &PgPool, customers: bool) -> Result<Options> {
    let rows = if customers {
        sqlx::query!(
            r#"SELECT c.id, o.name AS "name!" FROM customers c JOIN organizations o ON o.id = c.organization_id
               WHERE c.is_active ORDER BY o.name"#
        )
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|r| (r.id.to_string(), r.name))
        .collect()
    } else {
        sqlx::query!(
            r#"SELECT s.id, o.name AS "name!" FROM suppliers s JOIN organizations o ON o.id = s.organization_id
               WHERE s.is_active ORDER BY o.name"#
        )
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|r| (r.id.to_string(), r.name))
        .collect()
    };
    Ok(rows)
}

/// Every customer's (or supplier's) name, active or not, by id.
pub async fn party_names(pool: &PgPool, customers: bool) -> Result<HashMap<i64, String>> {
    let rows: Vec<(i32, String)> = if customers {
        sqlx::query!(r#"SELECT c.id, o.name AS "name!" FROM customers c JOIN organizations o ON o.id = c.organization_id"#)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|r| (r.id, r.name))
            .collect()
    } else {
        sqlx::query!(r#"SELECT s.id, o.name AS "name!" FROM suppliers s JOIN organizations o ON o.id = s.organization_id"#)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|r| (r.id, r.name))
            .collect()
    };
    Ok(rows.into_iter().map(|(id, name)| (i64::from(id), name)).collect())
}

/// A product as the line editor needs it.
pub struct ProductChoice {
    pub id: i32,
    pub label: String,
    pub description: String,
    pub unit_price: String,
    pub tax_code: Option<String>,
    pub revenue_account_id: Option<i32>,
    pub track_inventory: bool,
}

pub async fn products(pool: &PgPool) -> Result<Vec<ProductChoice>> {
    Ok(sqlx::query!(
        r#"SELECT id, sku, name, COALESCE(description, name) AS "description!", unit_price::text AS "unit_price!",
               tax_code, revenue_account_id, track_inventory
           FROM products WHERE is_active ORDER BY sku"#
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|p| ProductChoice {
        id: p.id,
        label: format!("{} {}", p.sku, p.name),
        description: p.description,
        unit_price: p.unit_price,
        tax_code: p.tax_code,
        revenue_account_id: p.revenue_account_id,
        track_inventory: p.track_inventory,
    })
    .collect())
}

/// Every product's "sku name", by id.
pub async fn product_names(pool: &PgPool) -> Result<HashMap<i64, String>> {
    Ok(sqlx::query!("SELECT id, sku, name FROM products")
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|p| (i64::from(p.id), format!("{} {}", p.sku, p.name)))
        .collect())
}

/// Active tax codes, with their rates.
pub async fn tax_codes(pool: &PgPool) -> Result<Vec<(String, String, String)>> {
    Ok(sqlx::query!(r#"SELECT code, name, trim_scale(rate)::text AS "rate!" FROM tax_codes WHERE is_active ORDER BY code"#)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|t| (t.code, t.name, t.rate))
        .collect())
}

pub async fn tax_code_options(pool: &PgPool) -> Result<Options> {
    Ok(tax_codes(pool).await?.into_iter().map(|(code, name, _)| (code.clone(), format!("{code} {name}"))).collect())
}

pub async fn payment_terms(pool: &PgPool) -> Result<Options> {
    Ok(sqlx::query!("SELECT code, name FROM payment_terms ORDER BY due_days, code")
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|t| (t.code.clone(), format!("{} {}", t.code, t.name)))
        .collect())
}

pub async fn warehouses(pool: &PgPool) -> Result<Options> {
    Ok(sqlx::query!("SELECT id, code, name FROM warehouses WHERE is_active ORDER BY code")
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|w| (w.id.to_string(), format!("{} {}", w.code, w.name)))
        .collect())
}

pub async fn warehouse_names(pool: &PgPool) -> Result<HashMap<i64, String>> {
    Ok(sqlx::query!("SELECT id, code FROM warehouses").fetch_all(pool).await?.into_iter().map(|w| (i64::from(w.id), w.code)).collect())
}

pub async fn currencies(pool: &PgPool) -> Result<Options> {
    Ok(sqlx::query!(r#"SELECT code::text AS "code!" FROM currencies ORDER BY code"#)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|c| (c.code.clone(), c.code))
        .collect())
}

pub async fn countries(pool: &PgPool) -> Result<Options> {
    Ok(sqlx::query!(r#"SELECT code::text AS "code!", name FROM countries ORDER BY code"#)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|c| (c.code.clone(), format!("{} {}", c.code, c.name)))
        .collect())
}

/// Every account's "code name", by id.
pub async fn account_names(pool: &PgPool) -> Result<HashMap<i64, String>> {
    Ok(sqlx::query!("SELECT id, code, name FROM accounts")
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|a| (i64::from(a.id), format!("{} {}", a.code, a.name)))
        .collect())
}

/// The base currency.
pub async fn base_currency(pool: &PgPool) -> Result<String> {
    Ok(sqlx::query_scalar!(r#"SELECT base_currency::text AS "c!" FROM gl_settings"#).fetch_one(pool).await?)
}
