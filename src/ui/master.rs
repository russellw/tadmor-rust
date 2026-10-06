//! Master data screens (spec/domain.md §13.3, M1 to M8): a list and a form
//! for each kind of record, the users screens, and the settings.
//!
//! Each kind of record is a `Resource`: its columns, its form, and the
//! service calls behind them. The handlers are generic over it.

use std::future::Future;

use axum::Router;
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::get;
use serde_json::{Value, json};
use sqlx::PgPool;

use super::forms::{Kind, field_value, money, plain, show};
use super::lookups::{self, Accounts, with_blank};
use super::{Action, Cell, Field, FieldKind, FormData, FormView, Section, Session, Table, Values, failure, forbidden, page, see_other};
use crate::error::{Error, Result};
use crate::http::AppState;
use crate::{currency, master, users};

/// A kind of master data record, as its screens show it.
pub trait Resource: Send + Sync + 'static {
    /// The list's path; records are at `{PATH}/{key}`.
    const PATH: &'static str;
    const TITLE: &'static str;
    const NOUN: &'static str;
    const COLUMNS: &'static [&'static str];
    /// The posted fields, and how each becomes JSON for the service.
    const SPEC: &'static [(&'static str, Kind)];

    /// Each record's key and its row, in the API's order.
    fn rows(pool: &PgPool) -> impl Future<Output = Result<Vec<(String, Vec<Cell>)>>> + Send;
    fn record(pool: &PgPool, key: &str) -> impl Future<Output = Result<Value>> + Send;
    /// The form's fields, filled from `record` (a record, or what was posted).
    fn fields(pool: &PgPool, record: &Value, key: Option<&str>) -> impl Future<Output = Result<Vec<Field>>> + Send;
    fn create(pool: &PgPool, values: Values) -> impl Future<Output = Result<()>> + Send;
    fn update(pool: &PgPool, key: &str, values: Values) -> impl Future<Output = Result<()>> + Send;
}

fn id(key: &str) -> Result<i64> {
    key.parse().map_err(|_| Error::not_found())
}

fn value(record: &Value, name: &str) -> String {
    field_value(&record[name])
}

/// A field filled from the record.
pub fn field(label: &str, name: &str, kind: FieldKind, record: &Value) -> Field {
    Field { value: value(record, name), ..Field::new(label, name, kind) }
}

pub fn required(mut f: Field) -> Field {
    f.required = true;
    f
}

pub fn readonly(mut f: Field) -> Field {
    f.readonly = true;
    f
}

/// A select over `options`, with a blank choice labelled `blank`.
pub fn select(label: &str, name: &str, record: &Value, blank: &str, options: lookups::Options) -> Field {
    let mut f = field(label, name, FieldKind::Select, record);
    f.options = with_blank(blank, options, &f.value);
    f
}

/// The active-status cell lists show (M6).
fn active(record: &Value) -> Cell {
    Cell::text(if record["is_active"] == true { "active" } else { "inactive" })
}

async fn list<R: Resource>(State(st): State<AppState>, session: Session) -> Response {
    match R::rows(&st.pool).await {
        Ok(rows) => {
            let mut table = Table::new(R::COLUMNS);
            table.rows = rows
                .into_iter()
                .map(|(key, mut cells)| {
                    cells[0].href = Some(format!("{}/{key}", R::PATH));
                    cells
                })
                .collect();
            let new = Action::link(&format!("New {}", R::NOUN), format!("{}/new", R::PATH));
            page(&session, R::TITLE, vec![Section::Actions(vec![new], None), Section::Table(table)])
        }
        Err(err) => failure(&session, err),
    }
}

async fn form_page<R: Resource>(pool: &PgPool, session: &Session, record: &Value, key: Option<&str>, error: Option<String>) -> Response {
    let fields = match R::fields(pool, record, key).await {
        Ok(fields) => fields,
        Err(err) => return failure(session, err),
    };
    let (title, action, submit) = match key {
        Some(key) => (format!("Edit {}", R::NOUN), format!("{}/{key}", R::PATH), "Save"),
        None => (format!("New {}", R::NOUN), R::PATH.to_string(), "Create"),
    };
    let form = FormView { action, fields, submit: submit.into(), error, cancel: Some(R::PATH.into()) };
    page(session, &title, vec![Section::Form(form)])
}

async fn new<R: Resource>(State(st): State<AppState>, session: Session) -> Response {
    form_page::<R>(&st.pool, &session, &json!({"is_active": true}), None, None).await
}

async fn create<R: Resource>(State(st): State<AppState>, session: Session, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    let result = match Values::from_form(&form, R::SPEC) {
        Ok(values) => R::create(&st.pool, values).await,
        Err(err) => Err(err),
    };
    match result {
        Ok(()) => see_other(R::PATH),
        Err(err) => form_page::<R>(&st.pool, &session, &echo(&form, R::SPEC), None, Some(err.message)).await,
    }
}

/// What was posted, to fill the form again after a refusal.
pub fn echo(form: &FormData, spec: &[(&str, Kind)]) -> Value {
    let pairs = spec.iter().map(|(name, kind)| {
        let v = if matches!(kind, Kind::Bool) { Value::Bool(form.flag(name)) } else { Value::String(form.text(name)) };
        (name.to_string(), v)
    });
    Value::Object(pairs.collect())
}

async fn edit<R: Resource>(State(st): State<AppState>, session: Session, Path(key): Path<String>) -> Response {
    match R::record(&st.pool, &key).await {
        Ok(record) => form_page::<R>(&st.pool, &session, &record, Some(&key), None).await,
        Err(err) => failure(&session, err),
    }
}

async fn update<R: Resource>(State(st): State<AppState>, session: Session, Path(key): Path<String>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    let result = match Values::from_form(&form, R::SPEC) {
        Ok(values) => R::update(&st.pool, &key, values).await,
        Err(err) => Err(err),
    };
    match result {
        Ok(()) => see_other(R::PATH),
        Err(err) => form_page::<R>(&st.pool, &session, &echo(&form, R::SPEC), Some(&key), Some(err.message)).await,
    }
}

fn resource<R: Resource>() -> Router<AppState> {
    Router::new()
        .route(R::PATH, get(list::<R>).post(create::<R>))
        .route(&format!("{}/new", R::PATH), get(new::<R>))
        .route(&format!("{}/{{key}}", R::PATH), get(edit::<R>).post(update::<R>))
}

pub fn routes() -> Router<AppState> {
    resource::<Organizations>()
        .merge(resource::<Customers>())
        .merge(resource::<Suppliers>())
        .merge(resource::<Products>())
        .merge(resource::<AccountsResource>())
        .merge(resource::<TaxCodes>())
        .merge(resource::<PaymentTerms>())
        .merge(resource::<Warehouses>())
        .route("/users", get(users_list).post(users_create))
        .route("/users/new", get(users_new))
        .route("/users/{id}", get(users_edit).post(users_update))
        .route("/users/{id}/password", get(password_page).post(password_set))
        .route("/settings", get(settings_page).post(settings_save))
}

fn to_value<T: serde::Serialize>(t: T) -> Value {
    serde_json::to_value(t).unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------
// M1 organizations
// ---------------------------------------------------------------------------

struct Organizations;

impl Resource for Organizations {
    const PATH: &'static str = "/organizations";
    const TITLE: &'static str = "Organizations";
    const NOUN: &'static str = "organization";
    const COLUMNS: &'static [&'static str] = &["Name", "Legal name", "Tax id", "Country", "Currency", "Email", "Our company"];
    const SPEC: &'static [(&'static str, Kind)] = &[
        ("name", Kind::Str),
        ("legal_name", Kind::OptStr),
        ("tax_id", Kind::OptStr),
        ("country_code", Kind::OptStr),
        ("default_currency", Kind::OptStr),
        ("email", Kind::OptStr),
        ("is_self", Kind::Bool),
    ];

    async fn rows(pool: &PgPool) -> Result<Vec<(String, Vec<Cell>)>> {
        Ok(master::list_organizations(pool)
            .await?
            .into_iter()
            .map(|o| {
                let cells = vec![
                    Cell::text(&o.name),
                    Cell::text(o.legal_name.unwrap_or_default()),
                    Cell::text(o.tax_id.unwrap_or_default()),
                    Cell::text(o.country_code.unwrap_or_default()),
                    Cell::text(o.default_currency.unwrap_or_default()),
                    Cell::text(o.email.unwrap_or_default()),
                    Cell::text(if o.is_self { "yes" } else { "" }),
                ];
                (o.id.to_string(), cells)
            })
            .collect())
    }

    async fn record(pool: &PgPool, key: &str) -> Result<Value> {
        Ok(to_value(master::get_organization(pool, id(key)?).await?))
    }

    async fn fields(pool: &PgPool, r: &Value, _key: Option<&str>) -> Result<Vec<Field>> {
        Ok(vec![
            required(field("Name", "name", FieldKind::Text, r)),
            field("Legal name", "legal_name", FieldKind::Text, r),
            field("Tax id", "tax_id", FieldKind::Text, r),
            select("Country", "country_code", r, "(none)", lookups::countries(pool).await?),
            select("Default currency", "default_currency", r, "(none)", lookups::currencies(pool).await?),
            field("Email", "email", FieldKind::Email, r),
            field("This is our company (shown on printed documents)", "is_self", FieldKind::Checkbox, r),
        ])
    }

    async fn create(pool: &PgPool, values: Values) -> Result<()> {
        master::create_organization(pool, values.into()?).await.map(|_| ())
    }

    async fn update(pool: &PgPool, key: &str, values: Values) -> Result<()> {
        master::update_organization(pool, id(key)?, values.into()?).await
    }
}

// ---------------------------------------------------------------------------
// M2 customers and suppliers
// ---------------------------------------------------------------------------

async fn organization_names(pool: &PgPool) -> Result<std::collections::HashMap<String, String>> {
    Ok(lookups::organizations(pool).await?.into_iter().collect())
}

/// The fields customers and suppliers share; the organization is chosen on
/// create and read-only after.
async fn party_fields(pool: &PgPool, r: &Value, key: Option<&str>, customer: bool) -> Result<Vec<Field>> {
    let (number, control, control_label) =
        if customer { ("customer_number", "ar_account_id", "A/R account") } else { ("supplier_number", "ap_account_id", "A/P account") };
    let mut org = required(select("Organization", "organization_id", r, "(choose)", lookups::organizations(pool).await?));
    org.readonly = key.is_some();
    let mut fields = vec![
        org,
        field(if customer { "Customer number" } else { "Supplier number" }, number, FieldKind::Text, r),
        select(control_label, control, r, "(none)", lookups::accounts(pool, Accounts::Postable).await?),
        select("Payment terms", "payment_terms_code", r, "(none)", lookups::payment_terms(pool).await?),
        select("Currency", "currency_code", r, "(none)", lookups::currencies(pool).await?),
        select("Tax code", "tax_code", r, "(none)", lookups::tax_code_options(pool).await?),
    ];
    if customer {
        fields.push(field("Credit limit", "credit_limit", FieldKind::Decimal, r));
    }
    if key.is_some() {
        fields.push(field("Active", "is_active", FieldKind::Checkbox, r));
    }
    Ok(fields)
}

struct Customers;

impl Resource for Customers {
    const PATH: &'static str = "/customers";
    const TITLE: &'static str = "Customers";
    const NOUN: &'static str = "customer";
    const COLUMNS: &'static [&'static str] = &["Organization", "Number", "Currency", "Tax code", "Terms", "Credit limit#", "Status"];
    const SPEC: &'static [(&'static str, Kind)] = &[
        ("organization_id", Kind::Int),
        ("customer_number", Kind::OptStr),
        ("ar_account_id", Kind::OptInt),
        ("payment_terms_code", Kind::OptStr),
        ("currency_code", Kind::OptStr),
        ("tax_code", Kind::OptStr),
        ("credit_limit", Kind::OptStr),
        ("is_active", Kind::Bool),
    ];

    async fn rows(pool: &PgPool) -> Result<Vec<(String, Vec<Cell>)>> {
        let names = organization_names(pool).await?;
        Ok(master::list_customers(pool)
            .await?
            .into_iter()
            .map(|c| {
                let v = to_value(&c);
                let cells = vec![
                    Cell::text(names.get(&c.organization_id.to_string()).cloned().unwrap_or_default()),
                    Cell::text(c.customer_number.unwrap_or_default()),
                    Cell::text(c.currency_code.unwrap_or_default()),
                    Cell::text(c.tax_code.unwrap_or_default()),
                    Cell::text(c.payment_terms_code.unwrap_or_default()),
                    Cell::number(c.credit_limit.as_deref().map(money).unwrap_or_default()),
                    active(&v),
                ];
                (c.id.to_string(), cells)
            })
            .collect())
    }

    async fn record(pool: &PgPool, key: &str) -> Result<Value> {
        Ok(to_value(master::get_customer(pool, id(key)?).await?))
    }

    async fn fields(pool: &PgPool, r: &Value, key: Option<&str>) -> Result<Vec<Field>> {
        party_fields(pool, r, key, true).await
    }

    async fn create(pool: &PgPool, values: Values) -> Result<()> {
        master::create_customer(pool, values.into()?).await.map(|_| ())
    }

    async fn update(pool: &PgPool, key: &str, values: Values) -> Result<()> {
        master::update_customer(pool, id(key)?, values.into()?).await
    }
}

struct Suppliers;

impl Resource for Suppliers {
    const PATH: &'static str = "/suppliers";
    const TITLE: &'static str = "Suppliers";
    const NOUN: &'static str = "supplier";
    const COLUMNS: &'static [&'static str] = &["Organization", "Number", "Currency", "Tax code", "Terms", "Status"];
    const SPEC: &'static [(&'static str, Kind)] = &[
        ("organization_id", Kind::Int),
        ("supplier_number", Kind::OptStr),
        ("ap_account_id", Kind::OptInt),
        ("payment_terms_code", Kind::OptStr),
        ("currency_code", Kind::OptStr),
        ("tax_code", Kind::OptStr),
        ("is_active", Kind::Bool),
    ];

    async fn rows(pool: &PgPool) -> Result<Vec<(String, Vec<Cell>)>> {
        let names = organization_names(pool).await?;
        Ok(master::list_suppliers(pool)
            .await?
            .into_iter()
            .map(|s| {
                let v = to_value(&s);
                let cells = vec![
                    Cell::text(names.get(&s.organization_id.to_string()).cloned().unwrap_or_default()),
                    Cell::text(s.supplier_number.unwrap_or_default()),
                    Cell::text(s.currency_code.unwrap_or_default()),
                    Cell::text(s.tax_code.unwrap_or_default()),
                    Cell::text(s.payment_terms_code.unwrap_or_default()),
                    active(&v),
                ];
                (s.id.to_string(), cells)
            })
            .collect())
    }

    async fn record(pool: &PgPool, key: &str) -> Result<Value> {
        Ok(to_value(master::get_supplier(pool, id(key)?).await?))
    }

    async fn fields(pool: &PgPool, r: &Value, key: Option<&str>) -> Result<Vec<Field>> {
        party_fields(pool, r, key, false).await
    }

    async fn create(pool: &PgPool, values: Values) -> Result<()> {
        master::create_supplier(pool, values.into()?).await.map(|_| ())
    }

    async fn update(pool: &PgPool, key: &str, values: Values) -> Result<()> {
        master::update_supplier(pool, id(key)?, values.into()?).await
    }
}

// ---------------------------------------------------------------------------
// M3 products
// ---------------------------------------------------------------------------

struct Products;

impl Resource for Products {
    const PATH: &'static str = "/products";
    const TITLE: &'static str = "Products";
    const NOUN: &'static str = "product";
    const COLUMNS: &'static [&'static str] = &["SKU", "Name", "Unit price#", "Currency", "Tax code", "Stocked", "Status"];
    const SPEC: &'static [(&'static str, Kind)] = &[
        ("sku", Kind::Str),
        ("name", Kind::Str),
        ("description", Kind::OptStr),
        ("unit_price", Kind::OptStr),
        ("currency_code", Kind::OptStr),
        ("revenue_account_id", Kind::OptInt),
        ("tax_code", Kind::OptStr),
        ("track_inventory", Kind::Bool),
        ("inventory_account_id", Kind::OptInt),
        ("cogs_account_id", Kind::OptInt),
        ("is_active", Kind::Bool),
    ];

    async fn rows(pool: &PgPool) -> Result<Vec<(String, Vec<Cell>)>> {
        Ok(master::list_products(pool)
            .await?
            .into_iter()
            .map(|p| {
                let v = to_value(&p);
                let cells = vec![
                    Cell::text(&p.sku),
                    Cell::text(&p.name),
                    Cell::number(money(&p.unit_price)),
                    Cell::text(p.currency_code.unwrap_or_default()),
                    Cell::text(p.tax_code.unwrap_or_default()),
                    Cell::text(if p.track_inventory { "yes" } else { "" }),
                    active(&v),
                ];
                (p.id.to_string(), cells)
            })
            .collect())
    }

    async fn record(pool: &PgPool, key: &str) -> Result<Value> {
        Ok(to_value(master::get_product(pool, id(key)?).await?))
    }

    async fn fields(pool: &PgPool, r: &Value, key: Option<&str>) -> Result<Vec<Field>> {
        let accounts = lookups::accounts(pool, Accounts::Postable).await?;
        let mut fields = vec![
            required(field("SKU", "sku", FieldKind::Text, r)),
            required(field("Name", "name", FieldKind::Text, r)),
            field("Description", "description", FieldKind::TextArea, r),
            field("Unit price", "unit_price", FieldKind::Decimal, r),
            select("Currency", "currency_code", r, "(none)", lookups::currencies(pool).await?),
            select("Revenue account", "revenue_account_id", r, "(none)", accounts.clone()),
            select("Tax code", "tax_code", r, "(none)", lookups::tax_code_options(pool).await?),
            field("Track inventory", "track_inventory", FieldKind::Checkbox, r),
            select("Inventory account", "inventory_account_id", r, "(none)", accounts.clone()),
            select("COGS account", "cogs_account_id", r, "(none)", accounts),
        ];
        if key.is_some() {
            fields.push(field("Active", "is_active", FieldKind::Checkbox, r));
        }
        Ok(fields)
    }

    async fn create(pool: &PgPool, values: Values) -> Result<()> {
        master::create_product(pool, values.into()?).await.map(|_| ())
    }

    async fn update(pool: &PgPool, key: &str, values: Values) -> Result<()> {
        master::update_product(pool, id(key)?, values.into()?).await
    }
}

// ---------------------------------------------------------------------------
// M4 chart of accounts
// ---------------------------------------------------------------------------

struct AccountsResource;

impl Resource for AccountsResource {
    const PATH: &'static str = "/accounts";
    const TITLE: &'static str = "Chart of accounts";
    const NOUN: &'static str = "account";
    const COLUMNS: &'static [&'static str] = &["Code", "Name", "Type", "Currency", "Postable", "Cash", "Status", "Ledger"];
    const SPEC: &'static [(&'static str, Kind)] = &[
        ("code", Kind::Str),
        ("name", Kind::Str),
        ("account_type", Kind::Str),
        ("parent_id", Kind::OptInt),
        ("currency_code", Kind::OptStr),
        ("is_postable", Kind::Bool),
        ("is_active", Kind::Bool),
        ("is_cash", Kind::Bool),
        ("cash_flow_activity", Kind::OptStr),
    ];

    async fn rows(pool: &PgPool) -> Result<Vec<(String, Vec<Cell>)>> {
        Ok(master::list_accounts(pool)
            .await?
            .into_iter()
            .map(|a| {
                let v = to_value(&a);
                let cells = vec![
                    Cell::text(&a.code),
                    Cell::text(&a.name),
                    Cell::text(&a.account_type),
                    Cell::text(a.currency_code.unwrap_or_default()),
                    Cell::text(if a.is_postable { "yes" } else { "summary" }),
                    Cell::text(if a.is_cash { "yes" } else { "" }),
                    active(&v),
                    Cell::link("ledger", format!("/reports/ledger/{}", a.id)),
                ];
                (a.id.to_string(), cells)
            })
            .collect())
    }

    async fn record(pool: &PgPool, key: &str) -> Result<Value> {
        Ok(to_value(master::get_account(pool, id(key)?).await?))
    }

    async fn fields(pool: &PgPool, r: &Value, key: Option<&str>) -> Result<Vec<Field>> {
        let types = ["asset", "liability", "equity", "revenue", "expense"].map(|t| (t.to_string(), t.to_string())).to_vec();
        let activities = ["operating", "investing", "financing"].map(|t| (t.to_string(), t.to_string())).to_vec();
        // A parent picker never offers the account itself.
        let parents: lookups::Options =
            lookups::accounts(pool, Accounts::Active).await?.into_iter().filter(|(v, _)| Some(v.as_str()) != key).collect();
        let mut kind = required(select("Type", "account_type", r, "(choose)", types));
        kind.options.retain(|(v, _)| !v.is_empty() || kind.value.is_empty());
        let mut fields = vec![
            required(field("Code", "code", FieldKind::Text, r)),
            required(field("Name", "name", FieldKind::Text, r)),
            kind,
            select("Parent", "parent_id", r, "(top level)", parents),
            select("Currency", "currency_code", r, "(any)", lookups::currencies(pool).await?),
            field("Postable (carries journal lines; otherwise a summary account)", "is_postable", FieldKind::Checkbox, r),
            field("Cash or bank account (assets only)", "is_cash", FieldKind::Checkbox, r),
            select("Cash flow activity", "cash_flow_activity", r, "operating", activities),
        ];
        if key.is_some() {
            fields.push(field("Active", "is_active", FieldKind::Checkbox, r));
        }
        Ok(fields)
    }

    async fn create(pool: &PgPool, values: Values) -> Result<()> {
        master::create_account(pool, values.into()?).await.map(|_| ())
    }

    async fn update(pool: &PgPool, key: &str, values: Values) -> Result<()> {
        master::update_account(pool, id(key)?, values.into()?).await
    }
}

// ---------------------------------------------------------------------------
// M5 tax codes, payment terms, warehouses
// ---------------------------------------------------------------------------

struct TaxCodes;

impl Resource for TaxCodes {
    const PATH: &'static str = "/tax-codes";
    const TITLE: &'static str = "Tax codes";
    const NOUN: &'static str = "tax code";
    const COLUMNS: &'static [&'static str] = &["Code", "Name", "Rate %#", "Status"];
    const SPEC: &'static [(&'static str, Kind)] =
        &[("code", Kind::Str), ("name", Kind::Str), ("rate", Kind::OptStr), ("tax_account_id", Kind::OptInt), ("is_active", Kind::Bool)];

    async fn rows(pool: &PgPool) -> Result<Vec<(String, Vec<Cell>)>> {
        Ok(master::list_tax_codes(pool)
            .await?
            .into_iter()
            .map(|t| {
                let v = to_value(&t);
                let cells = vec![Cell::text(&t.code), Cell::text(&t.name), Cell::number(plain(&t.rate)), active(&v)];
                (t.code, cells)
            })
            .collect())
    }

    async fn record(pool: &PgPool, key: &str) -> Result<Value> {
        let mut v = to_value(master::get_tax_code(pool, key).await?);
        v["rate"] = Value::String(plain(&show(&v["rate"])));
        Ok(v)
    }

    async fn fields(pool: &PgPool, r: &Value, key: Option<&str>) -> Result<Vec<Field>> {
        let mut code = required(field("Code", "code", FieldKind::Text, r));
        code.readonly = key.is_some();
        let mut fields = vec![
            code,
            required(field("Name", "name", FieldKind::Text, r)),
            field("Rate (percent)", "rate", FieldKind::Decimal, r),
            select("Tax account", "tax_account_id", r, "(none)", lookups::accounts(pool, Accounts::Postable).await?),
        ];
        if key.is_some() {
            fields.push(field("Active", "is_active", FieldKind::Checkbox, r));
        }
        Ok(fields)
    }

    async fn create(pool: &PgPool, values: Values) -> Result<()> {
        master::create_tax_code(pool, values.into()?).await.map(|_| ())
    }

    async fn update(pool: &PgPool, key: &str, values: Values) -> Result<()> {
        master::update_tax_code(pool, key, values.into()?).await
    }
}

struct PaymentTerms;

impl Resource for PaymentTerms {
    const PATH: &'static str = "/payment-terms";
    const TITLE: &'static str = "Payment terms";
    const NOUN: &'static str = "payment term";
    const COLUMNS: &'static [&'static str] = &["Code", "Name", "Due days#"];
    const SPEC: &'static [(&'static str, Kind)] = &[("code", Kind::Str), ("name", Kind::Str), ("due_days", Kind::Int)];

    async fn rows(pool: &PgPool) -> Result<Vec<(String, Vec<Cell>)>> {
        Ok(master::list_payment_terms(pool)
            .await?
            .into_iter()
            .map(|t| {
                let cells = vec![Cell::text(&t.code), Cell::text(&t.name), Cell::number(t.due_days.to_string())];
                (t.code, cells)
            })
            .collect())
    }

    async fn record(pool: &PgPool, key: &str) -> Result<Value> {
        Ok(to_value(master::get_payment_term(pool, key).await?))
    }

    async fn fields(_pool: &PgPool, r: &Value, key: Option<&str>) -> Result<Vec<Field>> {
        let mut code = required(field("Code", "code", FieldKind::Text, r));
        code.readonly = key.is_some();
        Ok(vec![code, required(field("Name", "name", FieldKind::Text, r)), field("Due days", "due_days", FieldKind::Integer, r)])
    }

    async fn create(pool: &PgPool, values: Values) -> Result<()> {
        master::create_payment_term(pool, values.into()?).await.map(|_| ())
    }

    async fn update(pool: &PgPool, key: &str, values: Values) -> Result<()> {
        master::update_payment_term(pool, key, values.into()?).await
    }
}

struct Warehouses;

impl Resource for Warehouses {
    const PATH: &'static str = "/warehouses";
    const TITLE: &'static str = "Warehouses";
    const NOUN: &'static str = "warehouse";
    const COLUMNS: &'static [&'static str] = &["Code", "Name", "Status"];
    const SPEC: &'static [(&'static str, Kind)] =
        &[("code", Kind::Str), ("name", Kind::Str), ("address_id", Kind::OptInt), ("is_active", Kind::Bool)];

    async fn rows(pool: &PgPool) -> Result<Vec<(String, Vec<Cell>)>> {
        Ok(master::list_warehouses(pool)
            .await?
            .into_iter()
            .map(|w| {
                let v = to_value(&w);
                (w.id.to_string(), vec![Cell::text(&w.code), Cell::text(&w.name), active(&v)])
            })
            .collect())
    }

    async fn record(pool: &PgPool, key: &str) -> Result<Value> {
        Ok(to_value(master::get_warehouse(pool, id(key)?).await?))
    }

    async fn fields(_pool: &PgPool, r: &Value, key: Option<&str>) -> Result<Vec<Field>> {
        // Addresses have no screens (spec/domain.md §14); a warehouse keeps its own.
        let mut fields = vec![
            required(field("Code", "code", FieldKind::Text, r)),
            required(field("Name", "name", FieldKind::Text, r)),
            field("", "address_id", FieldKind::Hidden, r),
        ];
        if key.is_some() {
            fields.push(field("Active", "is_active", FieldKind::Checkbox, r));
        }
        Ok(fields)
    }

    async fn create(pool: &PgPool, values: Values) -> Result<()> {
        master::create_warehouse(pool, values.into()?).await.map(|_| ())
    }

    async fn update(pool: &PgPool, key: &str, values: Values) -> Result<()> {
        master::update_warehouse(pool, id(key)?, values.into()?).await
    }
}

// ---------------------------------------------------------------------------
// M7 users (administrators only)
// ---------------------------------------------------------------------------

fn user_fields(r: &Value, creating: bool) -> Vec<Field> {
    let mut fields = vec![required(field("Email", "email", FieldKind::Email, r)), required(field("Full name", "full_name", FieldKind::Text, r))];
    if creating {
        fields.push(required(field("Password (at least 8 characters)", "password", FieldKind::Password, r)));
    } else {
        fields.push(field("Active", "is_active", FieldKind::Checkbox, r));
    }
    fields.push(field("Administrator", "is_admin", FieldKind::Checkbox, r));
    fields
}

fn user_form(session: &Session, title: &str, action: String, fields: Vec<Field>, error: Option<String>, extra: Vec<Section>) -> Response {
    let form = FormView { action, fields, submit: "Save".into(), error, cancel: Some("/users".into()) };
    let mut sections = vec![Section::Form(form)];
    sections.extend(extra);
    page(session, title, sections)
}

async fn users_list(State(st): State<AppState>, session: Session) -> Response {
    if !session.user.is_admin {
        return forbidden(&session);
    }
    match users::list(&st.pool).await {
        Ok(list) => {
            let mut table = Table::new(&["Email", "Name", "Role", "Status"]);
            table.rows = list
                .iter()
                .map(|u| {
                    vec![
                        Cell::link(&u.email, format!("/users/{}", u.id)),
                        Cell::text(&u.full_name),
                        Cell::text(if u.is_admin { "administrator" } else { "user" }),
                        Cell::text(if u.is_active { "active" } else { "inactive" }),
                    ]
                })
                .collect();
            page(&session, "Users", vec![Section::Actions(vec![Action::link("New user", "/users/new")], None), Section::Table(table)])
        }
        Err(err) => failure(&session, err),
    }
}

async fn users_new(session: Session) -> Response {
    if !session.user.is_admin {
        return forbidden(&session);
    }
    user_form(&session, "New user", "/users".into(), user_fields(&json!({}), true), None, vec![])
}

async fn users_create(State(st): State<AppState>, session: Session, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    if !session.user.is_admin {
        return forbidden(&session);
    }
    let new = users::NewUser { email: form.text("email"), full_name: form.text("full_name"), password: form.text("password"), is_admin: form.flag("is_admin") };
    match users::create(&st.pool, new).await {
        Ok(_) => see_other("/users"),
        Err(err) => {
            let echo = json!({"email": form.text("email"), "full_name": form.text("full_name"), "is_admin": form.flag("is_admin")});
            user_form(&session, "New user", "/users".into(), user_fields(&echo, true), Some(err.message), vec![])
        }
    }
}

fn password_link(user: i64) -> Vec<Section> {
    vec![Section::Actions(vec![Action::link("Reset password", format!("/users/{user}/password"))], None)]
}

async fn users_edit(State(st): State<AppState>, session: Session, Path(user): Path<i64>) -> Response {
    if !session.user.is_admin {
        return forbidden(&session);
    }
    match users::get(&st.pool, user).await {
        Ok(u) => user_form(&session, "Edit user", format!("/users/{user}"), user_fields(&to_value(u), false), None, password_link(user)),
        Err(err) => failure(&session, err),
    }
}

/// Refuses, visibly, an administrator deactivating or demoting themselves.
async fn users_update(State(st): State<AppState>, session: Session, Path(user): Path<i64>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    if !session.user.is_admin {
        return forbidden(&session);
    }
    let update = users::UserUpdate {
        email: form.text("email"),
        full_name: form.text("full_name"),
        is_active: form.flag("is_active"),
        is_admin: form.flag("is_admin"),
    };
    match users::update(&st.pool, &session.user, user, update).await {
        Ok(()) => see_other("/users"),
        Err(err) => {
            let echo = json!({"email": form.text("email"), "full_name": form.text("full_name"),
                              "is_active": form.flag("is_active"), "is_admin": form.flag("is_admin")});
            user_form(&session, "Edit user", format!("/users/{user}"), user_fields(&echo, false), Some(err.message), password_link(user))
        }
    }
}

fn password_form(session: &Session, user: i64, error: Option<String>) -> Response {
    let fields = vec![required(Field::new("New password (at least 8 characters)", "password", FieldKind::Password))];
    let form = FormView { action: format!("/users/{user}/password"), fields, submit: "Set password".into(), error, cancel: Some(format!("/users/{user}")) };
    page(session, "Reset password", vec![Section::Text("Setting a new password signs the user out everywhere.".into()), Section::Form(form)])
}

async fn password_page(session: Session, Path(user): Path<i64>) -> Response {
    if !session.user.is_admin {
        return forbidden(&session);
    }
    password_form(&session, user, None)
}

async fn password_set(State(st): State<AppState>, session: Session, Path(user): Path<i64>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    if !session.user.is_admin {
        return forbidden(&session);
    }
    match users::set_password(&st.pool, user, form.text("password")).await {
        Ok(()) => see_other("/users"),
        Err(err) => password_form(&session, user, Some(err.message)),
    }
}

// ---------------------------------------------------------------------------
// M8 settings
// ---------------------------------------------------------------------------

async fn settings_view(pool: &PgPool, session: &Session, record: &Value, error: Option<String>) -> Response {
    let currencies = match lookups::currencies(pool).await {
        Ok(c) => c,
        Err(err) => return failure(session, err),
    };
    let accounts = match lookups::accounts(pool, Accounts::Postable).await {
        Ok(a) => a,
        Err(err) => return failure(session, err),
    };
    let mut base = required(select("Base currency", "base_currency", record, "(choose)", currencies));
    let mut fx = select("FX gain/loss account", "fx_gain_loss_account_id", record, "(none)", accounts);
    let mut sections = vec![Section::Text(
        "The base currency cannot change once any journal entry exists. Settling a document at a rate other than \
         its own needs an FX gain/loss account."
            .into(),
    )];
    if session.user.is_admin {
        let form = FormView { action: "/settings".into(), fields: vec![base, fx], submit: "Save".into(), error, cancel: None };
        sections.push(Section::Form(form));
    } else {
        base.readonly = true;
        fx.readonly = true;
        let label = |f: &Field| f.options.iter().find(|(v, _)| *v == f.value).map(|(_, l)| l.clone()).unwrap_or_default();
        sections.push(Section::Facts(vec![("Base currency".into(), Cell::text(label(&base))), ("FX gain/loss account".into(), Cell::text(label(&fx)))]));
        sections.push(Section::Text("Only an administrator can change the settings.".into()));
    }
    page(session, "Settings", sections)
}

async fn settings_page(State(st): State<AppState>, session: Session) -> Response {
    match currency::get_settings(&st.pool).await {
        Ok(s) => settings_view(&st.pool, &session, &to_value(s), None).await,
        Err(err) => failure(&session, err),
    }
}

async fn settings_save(State(st): State<AppState>, session: Session, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    if !session.user.is_admin {
        return forbidden(&session);
    }
    let result = match Values::from_form(&form, &[("base_currency", Kind::Str), ("fx_gain_loss_account_id", Kind::OptInt)]) {
        Ok(values) => match values.into() {
            Ok(settings) => currency::update_settings(&st.pool, settings).await,
            Err(err) => Err(err),
        },
        Err(err) => Err(err),
    };
    match result {
        Ok(()) => see_other("/settings"),
        Err(err) => {
            let echo = json!({"base_currency": form.text("base_currency"), "fx_gain_loss_account_id": form.text("fx_gain_loss_account_id")});
            settings_view(&st.pool, &session, &echo, Some(err.message)).await
        }
    }
}
