//! Master data (spec/api.md §5.2 to §5.6, domain §3): organizations, the
//! customer and supplier roles on them, products, the chart of accounts,
//! tax codes, payment terms, and warehouses.
//!
//! Most rules are the shared schema's: foreign keys, uniqueness (one
//! `is_self` organization, one role of each kind per organization), the
//! account checks, and non-negative amounts. Their refusals come back as
//! 409 and 422 through `Error`'s SQLSTATE mapping. Decimals arrive as text
//! and are cast in SQL, so Postgres rounds them to the column's scale before
//! any check, and a malformed one is a data exception (422). Ids bind as
//! int8 so that one out of int4's range is also a 422, not a 400.
//!
//! Every update is a full replacement (spec/api.md §1.3): an omitted
//! optional field becomes null and an omitted boolean false. Create ignores
//! `is_active`; new records start active.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::error::{Error, Result};

/// A 400 for a missing (empty) required field.
pub fn require(value: &str, field: &str) -> Result<()> {
    if value.is_empty() {
        return Err(Error::bad_request(format!("{field} is required")));
    }
    Ok(())
}

/// The error for an update or get that matched no row.
fn found(rows_affected: u64) -> Result<()> {
    if rows_affected == 0 { Err(Error::not_found()) } else { Ok(()) }
}

// ---------------------------------------------------------------------------
// Organizations
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct Organization {
    pub id: i32,
    pub name: String,
    pub legal_name: Option<String>,
    pub tax_id: Option<String>,
    pub country_code: Option<String>,
    pub default_currency: Option<String>,
    pub email: Option<String>,
    pub is_self: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct OrganizationInput {
    pub name: String,
    pub legal_name: Option<String>,
    pub tax_id: Option<String>,
    pub country_code: Option<String>,
    pub default_currency: Option<String>,
    pub email: Option<String>,
    pub is_self: bool,
}

pub async fn list_organizations(pool: &PgPool) -> Result<Vec<Organization>> {
    Ok(sqlx::query_as!(
        Organization,
        "SELECT id, name, legal_name, tax_id, country_code, default_currency, email, is_self
         FROM organizations ORDER BY name"
    )
    .fetch_all(pool)
    .await?)
}

pub async fn get_organization(pool: &PgPool, id: i64) -> Result<Organization> {
    sqlx::query_as!(
        Organization,
        "SELECT id, name, legal_name, tax_id, country_code, default_currency, email, is_self
         FROM organizations WHERE id = $1::int8",
        id
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(Error::not_found)
}

pub async fn create_organization(pool: &PgPool, o: OrganizationInput) -> Result<i32> {
    require(&o.name, "name")?;
    Ok(sqlx::query_scalar!(
        "INSERT INTO organizations (name, legal_name, tax_id, country_code, default_currency, email, is_self)
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
        o.name,
        o.legal_name,
        o.tax_id,
        o.country_code,
        o.default_currency,
        o.email,
        o.is_self
    )
    .fetch_one(pool)
    .await?)
}

pub async fn update_organization(pool: &PgPool, id: i64, o: OrganizationInput) -> Result<()> {
    require(&o.name, "name")?;
    found(
        sqlx::query!(
            "UPDATE organizations SET name = $2, legal_name = $3, tax_id = $4, country_code = $5,
                 default_currency = $6, email = $7, is_self = $8
             WHERE id = $1::int8",
            id,
            o.name,
            o.legal_name,
            o.tax_id,
            o.country_code,
            o.default_currency,
            o.email,
            o.is_self
        )
        .execute(pool)
        .await?
        .rows_affected(),
    )
}

// ---------------------------------------------------------------------------
// Customers and suppliers: roles on an organization
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct Customer {
    pub id: i32,
    pub organization_id: i32,
    pub customer_number: Option<String>,
    pub ar_account_id: Option<i32>,
    pub payment_terms_code: Option<String>,
    pub currency_code: Option<String>,
    pub tax_code: Option<String>,
    pub credit_limit: Option<String>,
    pub is_active: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct CustomerInput {
    pub organization_id: i64,
    pub customer_number: Option<String>,
    pub ar_account_id: Option<i64>,
    pub payment_terms_code: Option<String>,
    pub currency_code: Option<String>,
    pub tax_code: Option<String>,
    pub credit_limit: Option<String>,
    pub is_active: bool,
}

fn require_organization(organization_id: i64) -> Result<()> {
    if organization_id <= 0 {
        return Err(Error::bad_request("organization_id is required"));
    }
    Ok(())
}

pub async fn list_customers(pool: &PgPool) -> Result<Vec<Customer>> {
    Ok(sqlx::query_as!(
        Customer,
        "SELECT id, organization_id, customer_number, ar_account_id, payment_terms_code, currency_code,
             tax_code, credit_limit::text, is_active
         FROM customers ORDER BY id"
    )
    .fetch_all(pool)
    .await?)
}

pub async fn get_customer(pool: &PgPool, id: i64) -> Result<Customer> {
    sqlx::query_as!(
        Customer,
        "SELECT id, organization_id, customer_number, ar_account_id, payment_terms_code, currency_code,
             tax_code, credit_limit::text, is_active
         FROM customers WHERE id = $1::int8",
        id
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(Error::not_found)
}

pub async fn create_customer(pool: &PgPool, c: CustomerInput) -> Result<i32> {
    require_organization(c.organization_id)?;
    Ok(sqlx::query_scalar!(
        "INSERT INTO customers (organization_id, customer_number, ar_account_id, payment_terms_code,
             currency_code, tax_code, credit_limit)
         VALUES ($1::int8, $2, $3::int8, $4, $5, $6, $7::text::numeric) RETURNING id",
        c.organization_id,
        c.customer_number,
        c.ar_account_id,
        c.payment_terms_code,
        c.currency_code,
        c.tax_code,
        c.credit_limit
    )
    .fetch_one(pool)
    .await?)
}

pub async fn update_customer(pool: &PgPool, id: i64, c: CustomerInput) -> Result<()> {
    require_organization(c.organization_id)?;
    found(
        sqlx::query!(
            "UPDATE customers SET organization_id = $2::int8, customer_number = $3, ar_account_id = $4::int8,
                 payment_terms_code = $5, currency_code = $6, tax_code = $7,
                 credit_limit = $8::text::numeric, is_active = $9
             WHERE id = $1::int8",
            id,
            c.organization_id,
            c.customer_number,
            c.ar_account_id,
            c.payment_terms_code,
            c.currency_code,
            c.tax_code,
            c.credit_limit,
            c.is_active
        )
        .execute(pool)
        .await?
        .rows_affected(),
    )
}

#[derive(Debug, Serialize)]
pub struct Supplier {
    pub id: i32,
    pub organization_id: i32,
    pub supplier_number: Option<String>,
    pub ap_account_id: Option<i32>,
    pub payment_terms_code: Option<String>,
    pub currency_code: Option<String>,
    pub tax_code: Option<String>,
    pub is_active: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SupplierInput {
    pub organization_id: i64,
    pub supplier_number: Option<String>,
    pub ap_account_id: Option<i64>,
    pub payment_terms_code: Option<String>,
    pub currency_code: Option<String>,
    pub tax_code: Option<String>,
    pub is_active: bool,
}

pub async fn list_suppliers(pool: &PgPool) -> Result<Vec<Supplier>> {
    Ok(sqlx::query_as!(
        Supplier,
        "SELECT id, organization_id, supplier_number, ap_account_id, payment_terms_code, currency_code,
             tax_code, is_active
         FROM suppliers ORDER BY id"
    )
    .fetch_all(pool)
    .await?)
}

pub async fn get_supplier(pool: &PgPool, id: i64) -> Result<Supplier> {
    sqlx::query_as!(
        Supplier,
        "SELECT id, organization_id, supplier_number, ap_account_id, payment_terms_code, currency_code,
             tax_code, is_active
         FROM suppliers WHERE id = $1::int8",
        id
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(Error::not_found)
}

pub async fn create_supplier(pool: &PgPool, s: SupplierInput) -> Result<i32> {
    require_organization(s.organization_id)?;
    Ok(sqlx::query_scalar!(
        "INSERT INTO suppliers (organization_id, supplier_number, ap_account_id, payment_terms_code,
             currency_code, tax_code)
         VALUES ($1::int8, $2, $3::int8, $4, $5, $6) RETURNING id",
        s.organization_id,
        s.supplier_number,
        s.ap_account_id,
        s.payment_terms_code,
        s.currency_code,
        s.tax_code
    )
    .fetch_one(pool)
    .await?)
}

pub async fn update_supplier(pool: &PgPool, id: i64, s: SupplierInput) -> Result<()> {
    require_organization(s.organization_id)?;
    found(
        sqlx::query!(
            "UPDATE suppliers SET organization_id = $2::int8, supplier_number = $3, ap_account_id = $4::int8,
                 payment_terms_code = $5, currency_code = $6, tax_code = $7, is_active = $8
             WHERE id = $1::int8",
            id,
            s.organization_id,
            s.supplier_number,
            s.ap_account_id,
            s.payment_terms_code,
            s.currency_code,
            s.tax_code,
            s.is_active
        )
        .execute(pool)
        .await?
        .rows_affected(),
    )
}

// ---------------------------------------------------------------------------
// Products
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct Product {
    pub id: i32,
    pub sku: String,
    pub name: String,
    pub description: Option<String>,
    pub unit_price: String,
    pub currency_code: Option<String>,
    pub revenue_account_id: Option<i32>,
    pub tax_code: Option<String>,
    pub track_inventory: bool,
    pub inventory_account_id: Option<i32>,
    pub cogs_account_id: Option<i32>,
    pub is_active: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ProductInput {
    pub sku: String,
    pub name: String,
    pub description: Option<String>,
    /// Decimal; absent or empty means 0.
    pub unit_price: Option<String>,
    pub currency_code: Option<String>,
    pub revenue_account_id: Option<i64>,
    pub tax_code: Option<String>,
    pub track_inventory: bool,
    pub inventory_account_id: Option<i64>,
    pub cogs_account_id: Option<i64>,
    pub is_active: bool,
}

/// A decimal that defaults to zero when absent or empty.
fn or_zero(value: Option<String>) -> String {
    value.filter(|v| !v.is_empty()).unwrap_or_else(|| "0".to_string())
}

pub async fn list_products(pool: &PgPool) -> Result<Vec<Product>> {
    Ok(sqlx::query_as!(
        Product,
        r#"SELECT id, sku, name, description, unit_price::text AS "unit_price!", currency_code,
               revenue_account_id, tax_code, track_inventory, inventory_account_id, cogs_account_id, is_active
           FROM products ORDER BY sku"#
    )
    .fetch_all(pool)
    .await?)
}

pub async fn get_product(pool: &PgPool, id: i64) -> Result<Product> {
    sqlx::query_as!(
        Product,
        r#"SELECT id, sku, name, description, unit_price::text AS "unit_price!", currency_code,
               revenue_account_id, tax_code, track_inventory, inventory_account_id, cogs_account_id, is_active
           FROM products WHERE id = $1::int8"#,
        id
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(Error::not_found)
}

pub async fn create_product(pool: &PgPool, p: ProductInput) -> Result<i32> {
    require(&p.sku, "sku")?;
    require(&p.name, "name")?;
    Ok(sqlx::query_scalar!(
        "INSERT INTO products (sku, name, description, unit_price, currency_code, revenue_account_id,
             tax_code, track_inventory, inventory_account_id, cogs_account_id)
         VALUES ($1, $2, $3, $4::text::numeric, $5, $6::int8, $7, $8, $9::int8, $10::int8) RETURNING id",
        p.sku,
        p.name,
        p.description,
        or_zero(p.unit_price),
        p.currency_code,
        p.revenue_account_id,
        p.tax_code,
        p.track_inventory,
        p.inventory_account_id,
        p.cogs_account_id
    )
    .fetch_one(pool)
    .await?)
}

pub async fn update_product(pool: &PgPool, id: i64, p: ProductInput) -> Result<()> {
    require(&p.sku, "sku")?;
    require(&p.name, "name")?;
    found(
        sqlx::query!(
            "UPDATE products SET sku = $2, name = $3, description = $4, unit_price = $5::text::numeric,
                 currency_code = $6, revenue_account_id = $7::int8, tax_code = $8, track_inventory = $9,
                 inventory_account_id = $10::int8, cogs_account_id = $11::int8, is_active = $12
             WHERE id = $1::int8",
            id,
            p.sku,
            p.name,
            p.description,
            or_zero(p.unit_price),
            p.currency_code,
            p.revenue_account_id,
            p.tax_code,
            p.track_inventory,
            p.inventory_account_id,
            p.cogs_account_id,
            p.is_active
        )
        .execute(pool)
        .await?
        .rows_affected(),
    )
}

// ---------------------------------------------------------------------------
// Chart of accounts
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct Account {
    pub id: i32,
    pub code: String,
    pub name: String,
    pub account_type: String,
    pub parent_id: Option<i32>,
    pub currency_code: Option<String>,
    pub is_postable: bool,
    pub is_active: bool,
    pub is_cash: bool,
    pub cash_flow_activity: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct AccountInput {
    pub code: String,
    pub name: String,
    pub account_type: String,
    pub parent_id: Option<i64>,
    pub currency_code: Option<String>,
    /// Defaults to false: a summary account.
    pub is_postable: bool,
    pub is_active: bool,
    pub is_cash: bool,
    /// Defaults to `operating`.
    pub cash_flow_activity: Option<String>,
}

impl AccountInput {
    fn check(&self) -> Result<()> {
        require(&self.code, "code")?;
        require(&self.name, "name")?;
        require(&self.account_type, "account_type")
    }

    fn activity(&self) -> String {
        self.cash_flow_activity.clone().filter(|a| !a.is_empty()).unwrap_or_else(|| "operating".to_string())
    }
}

pub async fn list_accounts(pool: &PgPool) -> Result<Vec<Account>> {
    Ok(sqlx::query_as!(
        Account,
        "SELECT id, code, name, account_type, parent_id, currency_code, is_postable, is_active, is_cash,
             cash_flow_activity
         FROM accounts ORDER BY code"
    )
    .fetch_all(pool)
    .await?)
}

pub async fn get_account(pool: &PgPool, id: i64) -> Result<Account> {
    sqlx::query_as!(
        Account,
        "SELECT id, code, name, account_type, parent_id, currency_code, is_postable, is_active, is_cash,
             cash_flow_activity
         FROM accounts WHERE id = $1::int8",
        id
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(Error::not_found)
}

pub async fn create_account(pool: &PgPool, a: AccountInput) -> Result<i32> {
    a.check()?;
    Ok(sqlx::query_scalar!(
        "INSERT INTO accounts (code, name, account_type, parent_id, currency_code, is_postable, is_cash,
             cash_flow_activity)
         VALUES ($1, $2, $3, $4::int8, $5, $6, $7, $8) RETURNING id",
        a.code,
        a.name,
        a.account_type,
        a.parent_id,
        a.currency_code,
        a.is_postable,
        a.is_cash,
        a.activity()
    )
    .fetch_one(pool)
    .await?)
}

pub async fn update_account(pool: &PgPool, id: i64, a: AccountInput) -> Result<()> {
    a.check()?;
    found(
        sqlx::query!(
            "UPDATE accounts SET code = $2, name = $3, account_type = $4, parent_id = $5::int8,
                 currency_code = $6, is_postable = $7, is_active = $8, is_cash = $9, cash_flow_activity = $10
             WHERE id = $1::int8",
            id,
            a.code,
            a.name,
            a.account_type,
            a.parent_id,
            a.currency_code,
            a.is_postable,
            a.is_active,
            a.is_cash,
            a.activity()
        )
        .execute(pool)
        .await?
        .rows_affected(),
    )
}

// ---------------------------------------------------------------------------
// Tax codes and payment terms, keyed by code
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct TaxCode {
    pub code: String,
    pub name: String,
    pub rate: String,
    pub tax_account_id: Option<i32>,
    pub is_active: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct TaxCodeInput {
    /// Ignored on update: the path's code wins.
    pub code: String,
    pub name: String,
    /// Percent; absent or empty means 0.
    pub rate: Option<String>,
    pub tax_account_id: Option<i64>,
    pub is_active: bool,
}

pub async fn list_tax_codes(pool: &PgPool) -> Result<Vec<TaxCode>> {
    Ok(sqlx::query_as!(
        TaxCode,
        r#"SELECT code, name, rate::text AS "rate!", tax_account_id, is_active FROM tax_codes ORDER BY code"#
    )
    .fetch_all(pool)
    .await?)
}

pub async fn get_tax_code(pool: &PgPool, code: &str) -> Result<TaxCode> {
    sqlx::query_as!(
        TaxCode,
        r#"SELECT code, name, rate::text AS "rate!", tax_account_id, is_active FROM tax_codes WHERE code = $1"#,
        code
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(Error::not_found)
}

pub async fn create_tax_code(pool: &PgPool, t: TaxCodeInput) -> Result<String> {
    require(&t.code, "code")?;
    require(&t.name, "name")?;
    Ok(sqlx::query_scalar!(
        "INSERT INTO tax_codes (code, name, rate, tax_account_id) VALUES ($1, $2, $3::text::numeric, $4::int8)
         RETURNING code",
        t.code,
        t.name,
        or_zero(t.rate),
        t.tax_account_id
    )
    .fetch_one(pool)
    .await?)
}

pub async fn update_tax_code(pool: &PgPool, code: &str, t: TaxCodeInput) -> Result<()> {
    require(&t.name, "name")?;
    found(
        sqlx::query!(
            "UPDATE tax_codes SET name = $2, rate = $3::text::numeric, tax_account_id = $4::int8, is_active = $5
             WHERE code = $1",
            code,
            t.name,
            or_zero(t.rate),
            t.tax_account_id,
            t.is_active
        )
        .execute(pool)
        .await?
        .rows_affected(),
    )
}

#[derive(Debug, Serialize)]
pub struct PaymentTerm {
    pub code: String,
    pub name: String,
    pub due_days: i32,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PaymentTermInput {
    /// Ignored on update: the path's code wins.
    pub code: String,
    pub name: String,
    pub due_days: i32,
}

pub async fn list_payment_terms(pool: &PgPool) -> Result<Vec<PaymentTerm>> {
    Ok(sqlx::query_as!(PaymentTerm, "SELECT code, name, due_days FROM payment_terms ORDER BY due_days, code")
        .fetch_all(pool)
        .await?)
}

pub async fn get_payment_term(pool: &PgPool, code: &str) -> Result<PaymentTerm> {
    sqlx::query_as!(PaymentTerm, "SELECT code, name, due_days FROM payment_terms WHERE code = $1", code)
        .fetch_optional(pool)
        .await?
        .ok_or_else(Error::not_found)
}

pub async fn create_payment_term(pool: &PgPool, p: PaymentTermInput) -> Result<String> {
    require(&p.code, "code")?;
    require(&p.name, "name")?;
    Ok(sqlx::query_scalar!(
        "INSERT INTO payment_terms (code, name, due_days) VALUES ($1, $2, $3) RETURNING code",
        p.code,
        p.name,
        p.due_days
    )
    .fetch_one(pool)
    .await?)
}

pub async fn update_payment_term(pool: &PgPool, code: &str, p: PaymentTermInput) -> Result<()> {
    require(&p.name, "name")?;
    found(
        sqlx::query!("UPDATE payment_terms SET name = $2, due_days = $3 WHERE code = $1", code, p.name, p.due_days)
            .execute(pool)
            .await?
            .rows_affected(),
    )
}

// ---------------------------------------------------------------------------
// Warehouses
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct Warehouse {
    pub id: i32,
    pub code: String,
    pub name: String,
    pub address_id: Option<i32>,
    pub is_active: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct WarehouseInput {
    pub code: String,
    pub name: String,
    pub address_id: Option<i64>,
    pub is_active: bool,
}

pub async fn list_warehouses(pool: &PgPool) -> Result<Vec<Warehouse>> {
    Ok(sqlx::query_as!(Warehouse, "SELECT id, code, name, address_id, is_active FROM warehouses ORDER BY code")
        .fetch_all(pool)
        .await?)
}

pub async fn get_warehouse(pool: &PgPool, id: i64) -> Result<Warehouse> {
    sqlx::query_as!(Warehouse, "SELECT id, code, name, address_id, is_active FROM warehouses WHERE id = $1::int8", id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(Error::not_found)
}

pub async fn create_warehouse(pool: &PgPool, w: WarehouseInput) -> Result<i32> {
    require(&w.code, "code")?;
    require(&w.name, "name")?;
    Ok(sqlx::query_scalar!(
        "INSERT INTO warehouses (code, name, address_id) VALUES ($1, $2, $3::int8) RETURNING id",
        w.code,
        w.name,
        w.address_id
    )
    .fetch_one(pool)
    .await?)
}

pub async fn update_warehouse(pool: &PgPool, id: i64, w: WarehouseInput) -> Result<()> {
    require(&w.code, "code")?;
    require(&w.name, "name")?;
    found(
        sqlx::query!(
            "UPDATE warehouses SET code = $2, name = $3, address_id = $4::int8, is_active = $5 WHERE id = $1::int8",
            id,
            w.code,
            w.name,
            w.address_id,
            w.is_active
        )
        .execute(pool)
        .await?
        .rows_affected(),
    )
}
