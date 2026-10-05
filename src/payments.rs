//! Customer and supplier payments (spec/api.md §5.9, domain §4): drafts,
//! posting, and unposting. Like the documents with lines, the two kinds
//! differ only in names, so each is described once and its SQL built from
//! that (see src/documents.rs). Applying them lives in src/settlement.rs.

use serde_json::{Map, Value};
use sqlx::PgPool;

use crate::documents::{conflict, optional_int, optional_string, parse, string};
use crate::error::{Error, Result};
use crate::posting;

pub struct PaymentKind {
    pub path: &'static str,
    pub noun: &'static str,
    pub table: &'static str,
    pub party: &'static str,
    pub party_table: &'static str,
    pub control: &'static str,
    /// The cash or bank account: debited by a receipt, credited by a payment.
    pub account: &'static str,
    /// Whether the account is debited (receipts) and the control credited.
    pub account_is_debit: bool,
    pub applications: &'static str,
}

pub static CUSTOMER_PAYMENTS: PaymentKind = PaymentKind {
    path: "customer-payments",
    noun: "Customer payment",
    table: "customer_payments",
    party: "customer_id",
    party_table: "customers",
    control: "ar_account_id",
    account: "deposit_account_id",
    account_is_debit: true,
    applications: "payment_applications",
};

pub static SUPPLIER_PAYMENTS: PaymentKind = PaymentKind {
    path: "supplier-payments",
    noun: "Supplier payment",
    table: "supplier_payments",
    party: "supplier_id",
    party_table: "suppliers",
    control: "ap_account_id",
    account: "payment_account_id",
    account_is_debit: false,
    applications: "bill_applications",
};

pub static PAYMENT_KINDS: [&PaymentKind; 2] = [&CUSTOMER_PAYMENTS, &SUPPLIER_PAYMENTS];

#[derive(Debug, Default)]
pub struct PaymentInput {
    pub party_id: i64,
    pub payment_date: String,
    pub currency_code: String,
    /// Decimal, required and positive.
    pub amount: String,
    /// cash, check, card, transfer, other, or none.
    pub method: Option<String>,
    pub reference: Option<String>,
    pub account_id: Option<i64>,
}

impl PaymentInput {
    pub fn from_json(kind: &PaymentKind, body: Value) -> Result<PaymentInput> {
        let Value::Object(m): Value = body else { return Err(Error::bad_request("invalid JSON body: expected an object")) };
        let m: &Map<String, Value> = &m;
        Ok(PaymentInput {
            party_id: optional_int(m, kind.party)?.unwrap_or(0),
            payment_date: string(m, "payment_date")?,
            currency_code: string(m, "currency_code")?,
            amount: string(m, "amount")?,
            method: optional_string(m, "method")?,
            reference: optional_string(m, "reference")?,
            account_id: optional_int(m, kind.account)?,
        })
    }

    fn check(&self, kind: &PaymentKind) -> Result<()> {
        let missing = |field: &str| Err(Error::bad_request(format!("{field} is required")));
        if self.party_id <= 0 {
            return missing(kind.party);
        }
        if self.payment_date.is_empty() {
            return missing("payment_date");
        }
        if self.currency_code.is_empty() {
            return missing("currency_code");
        }
        if self.amount.is_empty() {
            return missing("amount");
        }
        Ok(())
    }
}

/// Creates the payment as a draft and returns its id. The schema refuses an
/// amount that is not positive and an unknown method (422).
pub async fn create(pool: &PgPool, kind: &PaymentKind, p: PaymentInput) -> Result<i64> {
    p.check(kind)?;
    let sql = format!(
        "INSERT INTO {table} ({party}, payment_date, currency_code, amount, method, reference, {account})
         VALUES ($1::int8, $2::text::date, $3, $4::text::numeric, $5, $6, $7::int8) RETURNING id",
        table = kind.table,
        party = kind.party,
        account = kind.account,
    );
    let id = sqlx::query_scalar::<_, i32>(&sql)
        .bind(p.party_id)
        .bind(&p.payment_date)
        .bind(&p.currency_code)
        .bind(&p.amount)
        .bind(&p.method)
        .bind(&p.reference)
        .bind(p.account_id)
        .fetch_one(pool)
        .await?;
    Ok(i64::from(id))
}

async fn missing_or_not_draft(pool: &PgPool, kind: &PaymentKind, id: i64) -> Error {
    let sql = format!("SELECT status FROM {} WHERE id = $1::int8", kind.table);
    match sqlx::query_scalar::<_, String>(&sql).bind(id).fetch_optional(pool).await {
        Ok(None) => Error::not_found(),
        Ok(Some(status)) => conflict(format!("the payment is {status}, not a draft")),
        Err(err) => err.into(),
    }
}

/// Replaces a draft payment.
pub async fn update(pool: &PgPool, kind: &PaymentKind, id: i64, p: PaymentInput) -> Result<()> {
    p.check(kind)?;
    let sql = format!(
        "UPDATE {table} SET {party} = $2::int8, payment_date = $3::text::date, currency_code = $4,
             amount = $5::text::numeric, method = $6, reference = $7, {account} = $8::int8
         WHERE id = $1::int8 AND status = 'draft'",
        table = kind.table,
        party = kind.party,
        account = kind.account,
    );
    let done = sqlx::query(&sql)
        .bind(id)
        .bind(p.party_id)
        .bind(&p.payment_date)
        .bind(&p.currency_code)
        .bind(&p.amount)
        .bind(&p.method)
        .bind(&p.reference)
        .bind(p.account_id)
        .execute(pool)
        .await?;
    if done.rows_affected() == 0 {
        return Err(missing_or_not_draft(pool, kind, id).await);
    }
    Ok(())
}

pub async fn delete(pool: &PgPool, kind: &PaymentKind, id: i64) -> Result<()> {
    let sql = format!("DELETE FROM {} WHERE id = $1::int8 AND status = 'draft'", kind.table);
    if sqlx::query(&sql).bind(id).execute(pool).await?.rows_affected() == 0 {
        return Err(missing_or_not_draft(pool, kind, id).await);
    }
    Ok(())
}

fn read_sql(kind: &PaymentKind) -> String {
    format!(
        "SELECT json_build_object(
             'id', p.id, '{party}', p.{party}, 'payment_date', p.payment_date::text, '{account}', p.{account},
             'currency_code', p.currency_code, 'amount', p.amount::text, 'method', p.method,
             'reference', p.reference, 'status', p.status,
             'amount_applied', a.applied::numeric(19,4)::text, 'unapplied', (p.amount - a.applied)::numeric(19,4)::text,
             'journal_entry_id', p.journal_entry_id)::text
         FROM {table} p
         CROSS JOIN LATERAL (SELECT COALESCE(sum(amount_applied), 0) AS applied FROM {apps} WHERE payment_id = p.id) a",
        party = kind.party,
        account = kind.account,
        table = kind.table,
        apps = kind.applications,
    )
}

/// Newest first, then by id descending.
pub async fn list(pool: &PgPool, kind: &PaymentKind) -> Result<Vec<Value>> {
    let sql = format!("{} ORDER BY p.payment_date DESC, p.id DESC", read_sql(kind));
    sqlx::query_scalar::<_, String>(&sql).fetch_all(pool).await?.into_iter().map(parse).collect()
}

pub async fn get(pool: &PgPool, kind: &PaymentKind, id: i64) -> Result<Value> {
    let sql = format!("{} WHERE p.id = $1::int8", read_sql(kind));
    parse(sqlx::query_scalar::<_, String>(&sql).bind(id).fetch_optional(pool).await?.ok_or_else(Error::not_found)?)
}

/// Posts a draft payment (domain §4.2, §4.3): the cash or bank account and
/// the party's control account, each at round(amount × rate, 4).
pub async fn post(pool: &PgPool, kind: &PaymentKind, id: i64) -> Result<i32> {
    let mut tx = pool.begin().await?;
    let sql = format!(
        "SELECT p.status, p.currency_code::text, p.payment_date::text, p.amount::text, p.amount > 0, p.{account}, c.{control}
         FROM {table} p JOIN {parties} c ON c.id = p.{party}
         WHERE p.id = $1::int8",
        account = kind.account,
        control = kind.control,
        table = kind.table,
        parties = kind.party_table,
        party = kind.party,
    );
    let (status, currency, date, amount, has_amount, account, control) =
        sqlx::query_as::<_, (String, String, String, String, bool, Option<i32>, Option<i32>)>(&sql)
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(Error::not_found)?;
    if status != "draft" {
        return Err(conflict(format!("the payment is {status}, not a draft")));
    }
    if !has_amount {
        return Err(Error::unprocessable("the payment's amount is not positive"));
    }
    let (Some(account), Some(control)) = (account, control) else {
        return Err(Error::unprocessable(format!("the payment needs a {} and its party a {}", kind.account, kind.control)));
    };
    let period = posting::period_for_date(&mut tx, &date).await?;
    let entry = posting::create_entry(&mut tx, &date, period, &currency, kind.noun, None).await?;
    // Both lines carry the same amount, so the same rounded conversion
    // keeps the entry balanced in base.
    let (debit, credit) = if kind.account_is_debit { (account, control) } else { (control, account) };
    sqlx::query!(
        "INSERT INTO journal_lines (journal_entry_id, line_no, account_id, debit, credit, base_debit, base_credit)
         SELECT id, 1, $2, $3::text::numeric, 0, round($3::text::numeric * exchange_rate, 4), 0
         FROM journal_entries WHERE id = $1",
        entry,
        debit,
        amount
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "INSERT INTO journal_lines (journal_entry_id, line_no, account_id, debit, credit, base_debit, base_credit)
         SELECT id, 2, $2, 0, $3::text::numeric, 0, round($3::text::numeric * exchange_rate, 4)
         FROM journal_entries WHERE id = $1",
        entry,
        credit,
        amount
    )
    .execute(&mut *tx)
    .await?;
    let sql = format!("UPDATE {} SET status = 'posted', journal_entry_id = $1, period_id = $2 WHERE id = $3::int8", kind.table);
    sqlx::query(&sql).bind(entry).bind(period).bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(entry)
}

/// Reverses a posted payment's entry, reverses the realized-FX entries of
/// its applications and deletes them, and returns it to draft (domain §4.4).
pub async fn unpost(pool: &PgPool, kind: &PaymentKind, id: i64) -> Result<i32> {
    let mut tx = pool.begin().await?;
    let sql = format!("SELECT status, journal_entry_id FROM {} WHERE id = $1::int8", kind.table);
    let (status, entry) = sqlx::query_as::<_, (String, Option<i32>)>(&sql)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(Error::not_found)?;
    let Some(entry) = entry.filter(|_| status == "posted") else {
        return Err(conflict(format!("the payment is {status}, not posted")));
    };
    let reversal = posting::reverse_entry(&mut tx, entry).await?;
    let sql = format!(
        "SELECT fx_journal_entry_id FROM {} WHERE payment_id = $1::int8 AND fx_journal_entry_id IS NOT NULL ORDER BY id",
        kind.applications
    );
    for fx in sqlx::query_scalar::<_, i32>(&sql).bind(id).fetch_all(&mut *tx).await? {
        posting::reverse_entry(&mut tx, fx).await?;
    }
    let sql = format!("DELETE FROM {} WHERE payment_id = $1::int8", kind.applications);
    sqlx::query(&sql).bind(id).execute(&mut *tx).await?;
    let sql = format!("UPDATE {} SET status = 'draft', journal_entry_id = NULL, period_id = NULL WHERE id = $1::int8", kind.table);
    sqlx::query(&sql).bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(reversal)
}
