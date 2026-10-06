//! Bank reconciliation (spec/api.md §5.13, domain §8): statements for a
//! cash account, their lines, entered by hand or imported from CSV, matched
//! to posted journal lines, and reconciled.
//!
//! The schema keeps the rules: a statement's account is a postable, active
//! cash account; a reconciled statement and its lines are frozen; a line
//! matches a posted journal line on the statement's account with the same
//! signed amount, and a journal line backs at most one statement line (a
//! unique index, 409); and a statement reconciles only when every line is
//! matched and it balances.

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use sqlx::{PgExecutor, PgPool};

use crate::error::{Error, Result};
use crate::master::require;

#[derive(Debug, Serialize)]
pub struct Statement {
    pub id: i32,
    pub account_id: i32,
    pub account_code: String,
    pub account_name: String,
    pub statement_date: String,
    pub opening_balance: String,
    pub closing_balance: String,
    pub reference: Option<String>,
    pub status: String,
    pub line_count: i64,
    pub matched_count: i64,
    pub lines_total: String,
    /// opening + lines_total − closing.
    pub difference: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct StatementInput {
    pub account_id: i64,
    pub statement_date: String,
    pub opening_balance: String,
    pub closing_balance: String,
    pub reference: Option<String>,
}

impl StatementInput {
    fn check(&self) -> Result<()> {
        if self.account_id <= 0 {
            return Err(Error::bad_request("account_id is required"));
        }
        require(&self.statement_date, "statement_date")?;
        require(&self.opening_balance, "opening_balance")?;
        require(&self.closing_balance, "closing_balance")
    }
}

#[derive(Debug, Serialize)]
pub struct StatementLine {
    pub id: i32,
    pub line_no: i32,
    pub txn_date: String,
    pub description: String,
    pub reference: Option<String>,
    pub amount: String,
    pub journal_line_id: Option<i32>,
    pub journal_entry_id: Option<i32>,
    pub entry_date: Option<String>,
    pub entry_memo: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct LineInput {
    pub txn_date: String,
    pub description: String,
    pub reference: Option<String>,
    /// Signed from the books' side: a deposit is positive.
    pub amount: String,
}

impl LineInput {
    fn check(&self) -> Result<()> {
        require(&self.txn_date, "txn_date")?;
        require(&self.description, "description")?;
        require(&self.amount, "amount")
    }
}

#[derive(Debug, Serialize)]
pub struct Candidate {
    pub journal_line_id: i32,
    pub journal_entry_id: i32,
    pub entry_date: String,
    pub reference: Option<String>,
    pub memo: Option<String>,
    pub amount: String,
}

fn conflict(message: impl Into<String>) -> Error {
    Error::new(StatusCode::CONFLICT, message)
}

async fn status<'e, E: PgExecutor<'e>>(executor: E, id: i64) -> Result<String> {
    sqlx::query_scalar!("SELECT status FROM bank_statements WHERE id = $1::int8", id)
        .fetch_optional(executor)
        .await?
        .ok_or_else(Error::not_found)
}

async fn require_open<'e, E: PgExecutor<'e>>(executor: E, id: i64) -> Result<()> {
    let status = status(executor, id).await?;
    if status != "open" {
        return Err(conflict(format!("the statement is {status}, not open")));
    }
    Ok(())
}

/// Newest first, then by id descending.
pub async fn list(pool: &PgPool) -> Result<Vec<Statement>> {
    Ok(sqlx::query_as!(
        Statement,
        r#"SELECT s.id, s.account_id, a.code AS account_code, a.name AS account_name, s.statement_date::text AS "statement_date!",
               s.opening_balance::text AS "opening_balance!", s.closing_balance::text AS "closing_balance!", s.reference, s.status,
               count(l.id) AS "line_count!", count(l.journal_line_id) AS "matched_count!",
               COALESCE(sum(l.amount), 0)::numeric(19,4)::text AS "lines_total!",
               (s.opening_balance + COALESCE(sum(l.amount), 0) - s.closing_balance)::numeric(19,4)::text AS "difference!"
           FROM bank_statements s
           JOIN accounts a ON a.id = s.account_id
           LEFT JOIN bank_statement_lines l ON l.statement_id = s.id
           GROUP BY s.id, a.id ORDER BY s.statement_date DESC, s.id DESC"#
    )
    .fetch_all(pool)
    .await?)
}

pub async fn get(pool: &PgPool, id: i64) -> Result<Statement> {
    sqlx::query_as!(
        Statement,
        r#"SELECT s.id, s.account_id, a.code AS account_code, a.name AS account_name, s.statement_date::text AS "statement_date!",
               s.opening_balance::text AS "opening_balance!", s.closing_balance::text AS "closing_balance!", s.reference, s.status,
               count(l.id) AS "line_count!", count(l.journal_line_id) AS "matched_count!",
               COALESCE(sum(l.amount), 0)::numeric(19,4)::text AS "lines_total!",
               (s.opening_balance + COALESCE(sum(l.amount), 0) - s.closing_balance)::numeric(19,4)::text AS "difference!"
           FROM bank_statements s
           JOIN accounts a ON a.id = s.account_id
           LEFT JOIN bank_statement_lines l ON l.statement_id = s.id
           WHERE s.id = $1::int8
           GROUP BY s.id, a.id"#,
        id
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(Error::not_found)
}

pub async fn create(pool: &PgPool, s: StatementInput) -> Result<i32> {
    s.check()?;
    Ok(sqlx::query_scalar!(
        "INSERT INTO bank_statements (account_id, statement_date, opening_balance, closing_balance, reference)
         VALUES ($1::int8, $2::text::date, $3::text::numeric, $4::text::numeric, $5) RETURNING id",
        s.account_id,
        s.statement_date,
        s.opening_balance,
        s.closing_balance,
        s.reference
    )
    .fetch_one(pool)
    .await?)
}

/// Replaces an open statement's header. Changing the account while lines
/// are matched is refused by the schema (422).
pub async fn update(pool: &PgPool, id: i64, s: StatementInput) -> Result<()> {
    s.check()?;
    let mut tx = pool.begin().await?;
    require_open(&mut *tx, id).await?;
    sqlx::query!(
        "UPDATE bank_statements SET account_id = $2::int8, statement_date = $3::text::date,
             opening_balance = $4::text::numeric, closing_balance = $5::text::numeric, reference = $6
         WHERE id = $1::int8",
        id,
        s.account_id,
        s.statement_date,
        s.opening_balance,
        s.closing_balance,
        s.reference
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Deletes an open statement and its lines.
pub async fn delete(pool: &PgPool, id: i64) -> Result<()> {
    let mut tx = pool.begin().await?;
    require_open(&mut *tx, id).await?;
    sqlx::query!("DELETE FROM bank_statements WHERE id = $1::int8", id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

/// The statement's lines in order, each with what it is matched to.
pub async fn lines(pool: &PgPool, id: i64) -> Result<Vec<StatementLine>> {
    status(pool, id).await?;
    Ok(sqlx::query_as!(
        StatementLine,
        r#"SELECT l.id, l.line_no, l.txn_date::text AS "txn_date!", l.description, l.reference, l.amount::text AS "amount!",
               l.journal_line_id, je.id AS "journal_entry_id?", je.entry_date::text AS entry_date, je.memo AS "entry_memo?"
           FROM bank_statement_lines l
           LEFT JOIN journal_lines jl ON jl.id = l.journal_line_id
           LEFT JOIN journal_entries je ON je.id = jl.journal_entry_id
           WHERE l.statement_id = $1::int8 ORDER BY l.line_no"#,
        id
    )
    .fetch_all(pool)
    .await?)
}

async fn insert_line(tx: &mut sqlx::PgConnection, id: i64, l: &LineInput) -> Result<i32> {
    Ok(sqlx::query_scalar!(
        "INSERT INTO bank_statement_lines (statement_id, line_no, txn_date, description, reference, amount)
         VALUES ($1::int8, COALESCE((SELECT max(line_no) FROM bank_statement_lines WHERE statement_id = $1::int8), 0) + 1,
                 $2::text::date, $3, $4, $5::text::numeric)
         RETURNING id",
        id,
        l.txn_date,
        l.description,
        l.reference,
        l.amount
    )
    .fetch_one(tx)
    .await?)
}

/// Appends a line to an open statement; a zero amount is a 422.
pub async fn add_line(pool: &PgPool, id: i64, l: LineInput) -> Result<i32> {
    l.check()?;
    let mut tx = pool.begin().await?;
    require_open(&mut *tx, id).await?;
    let line = insert_line(&mut tx, id, &l).await?;
    tx.commit().await?;
    Ok(line)
}

/// Appends every record of a CSV text, or none if any is bad (domain §8.2).
/// Returns how many were imported.
pub async fn import(pool: &PgPool, id: i64, csv: &str) -> Result<usize> {
    if csv.is_empty() {
        return Err(Error::bad_request("csv is required"));
    }
    let mut tx = pool.begin().await?;
    require_open(&mut *tx, id).await?;
    let lines = parse_statement_csv(csv).map_err(Error::unprocessable)?;
    for l in &lines {
        insert_line(&mut tx, id, l).await?;
    }
    tx.commit().await?;
    Ok(lines.len())
}

/// The statement account's posted journal lines that no statement line has
/// claimed, by entry date, entry, then line.
pub async fn candidates(pool: &PgPool, id: i64) -> Result<Vec<Candidate>> {
    status(pool, id).await?;
    Ok(sqlx::query_as!(
        Candidate,
        r#"SELECT jl.id AS journal_line_id, je.id AS journal_entry_id, je.entry_date::text AS "entry_date!", je.reference,
               COALESCE(jl.memo, je.memo) AS memo, (jl.debit - jl.credit)::numeric(19,4)::text AS "amount!"
           FROM journal_lines jl
           JOIN journal_entries je ON je.id = jl.journal_entry_id
           WHERE je.status = 'posted'
             AND jl.account_id = (SELECT account_id FROM bank_statements WHERE id = $1::int8)
             AND NOT EXISTS (SELECT 1 FROM bank_statement_lines b WHERE b.journal_line_id = jl.id)
           ORDER BY je.entry_date, je.id, jl.line_no"#,
        id
    )
    .fetch_all(pool)
    .await?)
}

/// The statement of a line, which must be open.
async fn open_line(tx: &mut sqlx::PgConnection, line: i64) -> Result<()> {
    let status = sqlx::query_scalar!(
        "SELECT s.status FROM bank_statement_lines l JOIN bank_statements s ON s.id = l.statement_id WHERE l.id = $1::int8",
        line
    )
    .fetch_optional(tx)
    .await?
    .ok_or_else(Error::not_found)?;
    if status != "open" {
        return Err(conflict(format!("the statement is {status}, not open")));
    }
    Ok(())
}

/// Matches an unmatched line to a journal line; the schema checks it is
/// posted, on the account, and for the same amount (422), and not already
/// claimed (409).
pub async fn match_line(pool: &PgPool, line: i64, journal_line: i64) -> Result<()> {
    let mut tx = pool.begin().await?;
    open_line(&mut tx, line).await?;
    let done = sqlx::query!(
        "UPDATE bank_statement_lines SET journal_line_id = $2::int8 WHERE id = $1::int8 AND journal_line_id IS NULL",
        line,
        journal_line
    )
    .execute(&mut *tx)
    .await?;
    if done.rows_affected() == 0 {
        return Err(conflict("the line is already matched"));
    }
    tx.commit().await?;
    Ok(())
}

/// Releases a line's match; a no-op if it has none.
pub async fn unmatch_line(pool: &PgPool, line: i64) -> Result<()> {
    let mut tx = pool.begin().await?;
    open_line(&mut tx, line).await?;
    sqlx::query!("UPDATE bank_statement_lines SET journal_line_id = NULL WHERE id = $1::int8", line).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn delete_line(pool: &PgPool, line: i64) -> Result<()> {
    let mut tx = pool.begin().await?;
    open_line(&mut tx, line).await?;
    sqlx::query!("DELETE FROM bank_statement_lines WHERE id = $1::int8", line).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

/// Visits the unmatched lines in order, each taking the unclaimed candidate
/// with its amount and the nearest entry date, ties to the lowest journal
/// line. Returns how many were matched.
pub async fn auto_match(pool: &PgPool, id: i64) -> Result<usize> {
    let mut tx = pool.begin().await?;
    require_open(&mut *tx, id).await?;
    let unmatched = sqlx::query_scalar!(
        "SELECT id FROM bank_statement_lines WHERE statement_id = $1::int8 AND journal_line_id IS NULL ORDER BY line_no",
        id
    )
    .fetch_all(&mut *tx)
    .await?;
    let mut matched = 0;
    for line in unmatched {
        let done = sqlx::query!(
            "UPDATE bank_statement_lines l SET journal_line_id = c.id
             FROM (SELECT jl.id FROM bank_statement_lines b
                   JOIN bank_statements s ON s.id = b.statement_id
                   JOIN journal_lines jl ON jl.account_id = s.account_id AND jl.debit - jl.credit = b.amount
                   JOIN journal_entries je ON je.id = jl.journal_entry_id AND je.status = 'posted'
                   WHERE b.id = $1
                     AND NOT EXISTS (SELECT 1 FROM bank_statement_lines used WHERE used.journal_line_id = jl.id)
                   ORDER BY abs(je.entry_date - b.txn_date), jl.id
                   LIMIT 1) c
             WHERE l.id = $1",
            line
        )
        .execute(&mut *tx)
        .await?;
        matched += done.rows_affected() as usize;
    }
    tx.commit().await?;
    Ok(matched)
}

/// Reconciles an open statement: every line matched, and opening plus the
/// lines equal to closing (422 otherwise).
pub async fn reconcile(pool: &PgPool, id: i64) -> Result<()> {
    let mut tx = pool.begin().await?;
    require_open(&mut *tx, id).await?;
    let s = sqlx::query!(
        r#"SELECT count(*) FILTER (WHERE l.id IS NOT NULL AND l.journal_line_id IS NULL) AS "unmatched!",
               s.opening_balance + COALESCE(sum(l.amount), 0) = s.closing_balance AS "balanced!"
           FROM bank_statements s LEFT JOIN bank_statement_lines l ON l.statement_id = s.id
           WHERE s.id = $1::int8 GROUP BY s.id"#,
        id
    )
    .fetch_one(&mut *tx)
    .await?;
    if s.unmatched > 0 {
        return Err(Error::unprocessable(format!("{} line(s) are unmatched", s.unmatched)));
    }
    if !s.balanced {
        return Err(Error::unprocessable("opening balance plus the lines does not equal the closing balance"));
    }
    sqlx::query!("UPDATE bank_statements SET status = 'reconciled', reconciled_at = now() WHERE id = $1::int8", id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Reopens a reconciled statement (administrators only).
pub async fn reopen(pool: &PgPool, id: i64) -> Result<()> {
    let mut tx = pool.begin().await?;
    let status = status(&mut *tx, id).await?;
    if status != "reconciled" {
        return Err(conflict(format!("the statement is {status}, not reconciled")));
    }
    sqlx::query!("UPDATE bank_statements SET status = 'open', reconciled_at = NULL WHERE id = $1::int8", id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// The records of a CSV text (RFC 4180: quoted fields may hold commas,
/// quotes doubled, and line breaks). Leading spaces before a field are
/// skipped, and blank lines are not records.
fn csv_records(text: &str) -> std::result::Result<Vec<Vec<String>>, String> {
    let mut records = Vec::new();
    let mut chars = text.chars().peekable();
    while chars.peek().is_some() {
        let mut record = Vec::new();
        loop {
            while chars.next_if(|c| *c == ' ' || *c == '\t').is_some() {}
            let mut field = String::new();
            if chars.next_if_eq(&'"').is_some() {
                loop {
                    match chars.next() {
                        None => return Err(format!("record {}: an unterminated quoted field", records.len() + 1)),
                        Some('"') if chars.next_if_eq(&'"').is_some() => field.push('"'),
                        Some('"') => break,
                        Some(c) => field.push(c),
                    }
                }
                if !matches!(chars.peek(), None | Some(',' | '\n' | '\r')) {
                    return Err(format!("record {}: text after a closing quote", records.len() + 1));
                }
            } else {
                while let Some(c) = chars.next_if(|c| !matches!(c, ',' | '\n' | '\r')) {
                    if c == '"' {
                        return Err(format!("record {}: a quote inside an unquoted field", records.len() + 1));
                    }
                    field.push(c);
                }
            }
            record.push(field);
            if chars.next_if_eq(&',').is_none() {
                break;
            }
        }
        chars.next_if_eq(&'\r');
        chars.next_if_eq(&'\n');
        if !(record.len() == 1 && record[0].is_empty()) {
            records.push(record);
        }
    }
    Ok(records)
}

/// Whether `s` is a plain decimal, `-?(\d+(\.\d*)?|\.\d+)`.
fn is_plain_decimal(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    let (whole, fraction) = s.split_once('.').map_or((s, None), |(w, f)| (w, Some(f)));
    let digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
    digits(whole) && fraction.is_none_or(digits) && (!whole.is_empty() || fraction.is_some_and(|f| !f.is_empty()))
}

/// The lines of a statement CSV, `date,description,amount[,reference]`
/// (domain §8.2), or why it is refused. The first record is a header,
/// skipped, if its first field is not a date.
pub fn parse_statement_csv(text: &str) -> std::result::Result<Vec<LineInput>, String> {
    let mut lines = Vec::new();
    for (i, record) in csv_records(text)?.into_iter().enumerate() {
        let n = i + 1;
        let record: Vec<&str> = record.iter().map(|f| f.trim()).collect();
        if record.len() == 1 && record[0].is_empty() {
            continue;
        }
        if record.len() != 3 && record.len() != 4 {
            return Err(format!("record {n} has {} fields; want date,description,amount[,reference]", record.len()));
        }
        if !crate::http::is_date(record[0]) {
            if i == 0 {
                continue; // a header
            }
            return Err(format!("record {n}: {:?} is not a YYYY-MM-DD date", record[0]));
        }
        if record[1].is_empty() {
            return Err(format!("record {n}: the description is empty"));
        }
        if !is_plain_decimal(record[2]) {
            return Err(format!("record {n}: {:?} is not a decimal amount", record[2]));
        }
        if record[2].trim_matches(['-', '.', '0']).is_empty() {
            return Err(format!("record {n}: the amount is zero"));
        }
        lines.push(LineInput {
            txn_date: record[0].to_string(),
            description: record[1].to_string(),
            amount: record[2].to_string(),
            reference: record.get(3).filter(|r| !r.is_empty()).map(|r| r.to_string()),
        });
    }
    if lines.is_empty() {
        return Err("the CSV has no data rows".to_string());
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_quoting() {
        assert_eq!(csv_records("a,\"b,c\",\"say \"\"hi\"\"\"\r\n\nd\n").unwrap(), [vec!["a", "b,c", "say \"hi\""], vec!["d"]]);
        assert_eq!(csv_records("x,  \"y\"").unwrap(), [vec!["x", "y"]]);
        assert_eq!(csv_records("\"multi\nline\",1").unwrap(), [vec!["multi\nline", "1"]]);
        assert!(csv_records("\"open").is_err());
        assert!(csv_records("a\"b,c").is_err());
        assert!(csv_records("\"a\"b,c").is_err());
    }

    #[test]
    fn decimals() {
        for ok in ["1", "-40", "12.5", "12.", ".5", "-.5", "0.00"] {
            assert!(is_plain_decimal(ok), "{ok}");
        }
        for bad in ["", "-", ".", "1e3", "1.2.3", "+1", "1,000", " 1"] {
            assert!(!is_plain_decimal(bad), "{bad}");
        }
    }

    #[test]
    fn statement_csv() {
        let lines = parse_statement_csv("date,description,amount,reference\n2101-05-04, Cheque ,-40,CHQ-1\n\n2101-05-21,Deposit,25\n").unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!((lines[0].description.as_str(), lines[0].reference.as_deref()), ("Cheque", Some("CHQ-1")));
        assert_eq!(lines[1].reference, None);
        for bad in [
            "date,description,amount\n",
            "2101-05-04,Ok,1\nnot-a-date,Bad,1\n",
            "2101-05-04,Zero,0.00\n",
            "2101-05-04,Too few\n",
            "2101-05-04,Not a number,1e3\n",
            "2101-05-04,,1\n",
        ] {
            assert!(parse_statement_csv(bad).is_err(), "{bad:?}");
        }
    }
}
