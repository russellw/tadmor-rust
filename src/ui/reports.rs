//! Reports (spec/domain.md §13.8, R1 to R8), and the journal entry screen.
//!
//! Every amount here has at most four decimal places, so section totals and
//! the ledger's running balance are added exactly as integers of ten
//! thousandths, never as floats.

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::routing::get;
use serde::Deserialize;

use super::forms::{money, plain};
use super::{Cell, Field, FieldKind, FormView, Section, Session, Table, failure, page};
use crate::error::{Error, Result};
use crate::http::{AppState, is_date};
use crate::reporting::{self, ActivityRow, AgingRow};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/reports/profit-and-loss", get(profit_and_loss))
        .route("/reports/balance-sheet", get(balance_sheet))
        .route("/reports/cash-flow", get(cash_flow))
        .route("/reports/trial-balance", get(trial_balance))
        .route("/reports/ledger/{account}", get(ledger))
        .route("/reports/ar-aging", get(|st: State<AppState>, se: Session| aging(st, se, true)))
        .route("/reports/ap-aging", get(|st: State<AppState>, se: Session| aging(st, se, false)))
        .route("/reports/inventory-valuation", get(valuation))
        .route("/journal-entries/{id}", get(journal_entry))
}

/// A decimal as ten-thousandths. Report amounts have at most four places.
pub fn units(s: &str) -> i128 {
    let (sign, s) = s.strip_prefix('-').map_or((1, s), |rest| (-1, rest));
    let (whole, fraction) = s.split_once('.').unwrap_or((s, ""));
    let fraction = format!("{fraction:0<4}");
    sign * (whole.parse::<i128>().unwrap_or(0) * 10_000 + fraction[..4].parse::<i128>().unwrap_or(0))
}

/// Ten-thousandths as a decimal.
pub fn decimal(units: i128) -> String {
    let sign = if units < 0 { "-" } else { "" };
    let abs = units.unsigned_abs();
    format!("{sign}{}.{:04}", abs / 10_000, abs % 10_000)
}

fn total<'a>(values: impl IntoIterator<Item = &'a str>) -> String {
    decimal(values.into_iter().map(units).sum())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Range {
    from: String,
    to: String,
    as_of: String,
}

impl Range {
    /// The bounds, blank meaning unbounded; a malformed one is refused.
    fn bound(value: &str, name: &str) -> Result<Option<String>> {
        match value.trim() {
            "" => Ok(None),
            v if is_date(v) => Ok(Some(v.to_string())),
            _ => Err(Error::bad_request(format!("{name} must be a date, or blank"))),
        }
    }
}

fn date_filter(action: &str, fields: &[(&str, &str, &str)]) -> Section {
    let fields = fields.iter().map(|(label, name, value)| Field { value: value.to_string(), ..Field::new(label, name, FieldKind::Date) }).collect();
    Section::Form(FormView { get: true, action: action.into(), fields, submit: "Show".into(), error: None, cancel: None })
}

fn report(session: &Session, title: &str, filter: Section, result: Result<Vec<Section>>) -> Response {
    match result {
        Ok(mut sections) => {
            sections.insert(0, filter);
            page(session, title, sections)
        }
        Err(err) if err.status == axum::http::StatusCode::BAD_REQUEST => page(session, title, vec![filter, Section::Error(err.message)]),
        Err(err) => failure(session, err),
    }
}

/// One section of a statement: its accounts and their total.
fn statement_table(title: &str, rows: &[&ActivityRow]) -> Table {
    let mut table = Table::new(&["Code", "Account", "Amount#"]).titled(title);
    table.rows = rows
        .iter()
        .map(|r| vec![Cell::link(&r.code, format!("/reports/ledger/{}", r.account_id)), Cell::text(&r.name), Cell::number(money(&r.amount))])
        .collect();
    table.footer = Some(vec![Cell::text(""), Cell::text(format!("Total {}", title.to_lowercase())), Cell::number(money(&total(rows.iter().map(|r| r.amount.as_str()))))]);
    table.empty = format!("No {} accounts with activity.", title.to_lowercase());
    table
}

/// R1.
async fn profit_and_loss(State(st): State<AppState>, session: Session, Query(q): Query<Range>) -> Response {
    let filter = date_filter("/reports/profit-and-loss", &[("From", "from", &q.from), ("To", "to", &q.to)]);
    let result = async {
        let (from, to) = (Range::bound(&q.from, "from")?, Range::bound(&q.to, "to")?);
        let rows = reporting::profit_and_loss(&st.pool, from.as_deref(), to.as_deref()).await?;
        let revenue: Vec<_> = rows.iter().filter(|r| r.account_type == "revenue").collect();
        let expense: Vec<_> = rows.iter().filter(|r| r.account_type == "expense").collect();
        let net = units(&total(revenue.iter().map(|r| r.amount.as_str()))) - units(&total(expense.iter().map(|r| r.amount.as_str())));
        Ok(vec![
            Section::Table(statement_table("Revenue", &revenue)),
            Section::Table(statement_table("Expenses", &expense)),
            Section::Facts(vec![("Net income".into(), Cell::number(money(&decimal(net))))]),
        ])
    }
    .await;
    report(&session, "Profit and loss", filter, result)
}

/// R2: the three sections and current earnings, so that the identity
/// assets = liabilities + equity + current earnings is visible.
async fn balance_sheet(State(st): State<AppState>, session: Session, Query(q): Query<Range>) -> Response {
    let filter = date_filter("/reports/balance-sheet", &[("As of", "as_of", &q.as_of)]);
    let result = async {
        let as_of = Range::bound(&q.as_of, "as_of")?;
        let bs = reporting::balance_sheet(&st.pool, as_of.as_deref()).await?;
        let section = |t: &str| bs.rows.iter().filter(|r| r.account_type == t).collect::<Vec<_>>();
        let (assets, liabilities, equity) = (section("asset"), section("liability"), section("equity"));
        let sum = |rows: &[&ActivityRow]| units(&total(rows.iter().map(|r| r.amount.as_str())));
        let right = sum(&liabilities) + sum(&equity) + units(&bs.current_earnings);
        Ok(vec![
            Section::Table(statement_table("Assets", &assets)),
            Section::Table(statement_table("Liabilities", &liabilities)),
            Section::Table(statement_table("Equity", &equity)),
            Section::Facts(vec![
                ("Current earnings".into(), Cell::number(money(&bs.current_earnings))),
                ("Total assets".into(), Cell::number(money(&decimal(sum(&assets))))),
                ("Liabilities + equity + current earnings".into(), Cell::number(money(&decimal(right)))),
            ]),
        ])
    }
    .await;
    report(&session, "Balance sheet", filter, result)
}

/// R3: operating from net income, investing, financing, then cash.
async fn cash_flow(State(st): State<AppState>, session: Session, Query(q): Query<Range>) -> Response {
    let filter = date_filter("/reports/cash-flow", &[("From", "from", &q.from), ("To", "to", &q.to)]);
    let result = async {
        let (from, to) = (Range::bound(&q.from, "from")?, Range::bound(&q.to, "to")?);
        let cf = reporting::cash_flow(&st.pool, from.as_deref(), to.as_deref()).await?;
        let mut sections = Vec::new();
        for activity in ["operating", "investing", "financing"] {
            let rows: Vec<_> = cf.rows.iter().filter(|r| r.activity == activity).collect();
            let mut table = Table::new(&["Code", "Account", "Amount#"]).titled(format!("{}{} activities", activity[..1].to_uppercase(), &activity[1..]));
            let mut subtotal: i128 = rows.iter().map(|r| units(&r.amount)).sum();
            if activity == "operating" {
                table.rows.push(vec![Cell::text(""), Cell::text("Net income"), Cell::number(money(&cf.net_income))]);
                subtotal += units(&cf.net_income);
            }
            table.rows.extend(rows.iter().map(|r| vec![Cell::link(&r.code, format!("/reports/ledger/{}", r.account_id)), Cell::text(&r.name), Cell::number(money(&r.amount))]));
            table.footer = Some(vec![Cell::text(""), Cell::text(format!("Net cash from {activity} activities")), Cell::number(money(&decimal(subtotal)))]);
            table.empty = "No activity.".into();
            sections.push(Section::Table(table));
        }
        sections.push(Section::Facts(vec![
            ("Opening cash".into(), Cell::number(money(&cf.opening_cash))),
            ("Net cash flow".into(), Cell::number(money(&cf.net_cash_flow))),
            ("Closing cash".into(), Cell::number(money(&cf.closing_cash))),
        ]));
        Ok(sections)
    }
    .await;
    report(&session, "Cash flow", filter, result)
}

/// R4: every account, with totals; each links to its ledger.
async fn trial_balance(State(st): State<AppState>, session: Session) -> Response {
    match reporting::trial_balance(&st.pool).await {
        Ok(rows) => {
            let mut table = Table::new(&["Code", "Account", "Type", "Debit#", "Credit#", "Balance#"]);
            table.rows = rows
                .iter()
                .map(|r| {
                    vec![
                        Cell::link(&r.code, format!("/reports/ledger/{}", r.account_id)),
                        Cell::text(&r.name),
                        Cell::text(&r.account_type),
                        Cell::number(money(&r.total_debit)),
                        Cell::number(money(&r.total_credit)),
                        Cell::number(money(&r.balance)),
                    ]
                })
                .collect();
            table.footer = Some(vec![
                Cell::text(""),
                Cell::text("Total"),
                Cell::text(""),
                Cell::number(money(&total(rows.iter().map(|r| r.total_debit.as_str())))),
                Cell::number(money(&total(rows.iter().map(|r| r.total_credit.as_str())))),
                Cell::number(money(&total(rows.iter().map(|r| r.balance.as_str())))),
            ]);
            page(&session, "Trial balance", vec![Section::Text("Base currency, posted entries; balances are debit-positive.".into()), Section::Table(table)])
        }
        Err(err) => failure(&session, err),
    }
}

/// R5: the account's lines in range, with a running balance in base, and
/// the transaction currency where it differs.
async fn ledger(State(st): State<AppState>, session: Session, Path(account): Path<i64>, Query(q): Query<Range>) -> Response {
    let filter = date_filter(&format!("/reports/ledger/{account}"), &[("From", "from", &q.from), ("To", "to", &q.to)]);
    let title = match crate::master::get_account(&st.pool, account).await {
        Ok(a) => format!("Ledger: {} {}", a.code, a.name),
        Err(err) => return failure(&session, err),
    };
    let result = async {
        let (from, to) = (Range::bound(&q.from, "from")?, Range::bound(&q.to, "to")?);
        let rows = reporting::account_ledger(&st.pool, account, from.as_deref(), to.as_deref()).await?;
        let base = super::lookups::base_currency(&st.pool).await?;
        let foreign = rows.iter().any(|r| r.currency_code != base);
        let mut labels = vec!["Date", "Entry", "Reference", "Memo"];
        if foreign {
            labels.extend(["Currency", "Debit#", "Credit#"]);
        }
        labels.extend(["Base debit#", "Base credit#", "Balance#"]);
        let mut table = Table::new(&labels);
        let mut balance: i128 = 0;
        for r in &rows {
            balance += units(&r.base_debit) - units(&r.base_credit);
            let mut row = vec![
                Cell::text(&r.entry_date),
                Cell::link(format!("#{}", r.journal_entry_id), format!("/journal-entries/{}", r.journal_entry_id)),
                Cell::text(r.reference.clone().unwrap_or_default()),
                Cell::text(r.memo.clone().unwrap_or_default()),
            ];
            if foreign {
                row.extend([Cell::text(&r.currency_code), Cell::number(money(&r.debit)), Cell::number(money(&r.credit))]);
            }
            row.extend([Cell::number(money(&r.base_debit)), Cell::number(money(&r.base_credit)), Cell::number(money(&decimal(balance)))]);
            table.rows.push(row);
        }
        table.empty = "No posted lines in this range.".into();
        Ok(vec![Section::Table(table)])
    }
    .await;
    report(&session, &title, filter, result)
}

/// R6: the entry and every line, with totals.
async fn journal_entry(State(st): State<AppState>, session: Session, Path(id): Path<i64>) -> Response {
    match reporting::journal_entry(&st.pool, id).await {
        Ok(e) => {
            let facts = vec![
                ("Date".into(), Cell::text(&e.entry_date)),
                ("Currency".into(), Cell::text(&e.currency_code)),
                ("Exchange rate".into(), Cell::text(plain(&e.exchange_rate))),
                ("Reference".into(), Cell::text(e.reference.clone().unwrap_or_default())),
                ("Memo".into(), Cell::text(e.memo.clone().unwrap_or_default())),
                ("Status".into(), Cell::text(&e.status)),
            ];
            let mut table = Table::new(&["#", "Account", "Memo", "Debit#", "Credit#", "Base debit#", "Base credit#"]);
            table.rows = e
                .lines
                .iter()
                .map(|l| {
                    vec![
                        Cell::text(l.line_no.to_string()),
                        Cell::link(format!("{} {}", l.account_code, l.account_name), format!("/reports/ledger/{}", l.account_id)),
                        Cell::text(l.memo.clone().unwrap_or_default()),
                        Cell::number(money(&l.debit)),
                        Cell::number(money(&l.credit)),
                        Cell::number(money(&l.base_debit)),
                        Cell::number(money(&l.base_credit)),
                    ]
                })
                .collect();
            let sum = |f: fn(&reporting::JournalLine) -> &String| money(&total(e.lines.iter().map(|l| f(l).as_str())));
            table.footer = Some(vec![
                Cell::text(""),
                Cell::text("Total"),
                Cell::text(""),
                Cell::number(sum(|l| &l.debit)),
                Cell::number(sum(|l| &l.credit)),
                Cell::number(sum(|l| &l.base_debit)),
                Cell::number(sum(|l| &l.base_credit)),
            ]);
            page(&session, &format!("Journal entry #{id}"), vec![Section::Facts(facts), Section::Table(table)])
        }
        Err(err) => failure(&session, err),
    }
}

/// An aging row's five buckets and its total, in column order.
fn cells(r: &AgingRow) -> [&String; 6] {
    [&r.not_yet_due, &r.days_1_30, &r.days_31_60, &r.days_61_90, &r.days_over_90, &r.total_outstanding]
}

/// R7: one row per party, the five buckets and the total, and a total row.
async fn aging(State(st): State<AppState>, session: Session, receivable: bool) -> Response {
    let rows = if receivable { reporting::ar_aging(&st.pool).await } else { reporting::ap_aging(&st.pool).await };
    match rows {
        Ok(rows) => {
            let mut table = Table::new(&[if receivable { "Customer" } else { "Supplier" }, "Not yet due#", "1-30 days#", "31-60 days#", "61-90 days#", "Over 90 days#", "Total#"]);
            table.rows = rows
                .iter()
                .map(|r| {
                    let mut row = vec![Cell::link(&r.party_name, format!("/{}/{}", if receivable { "customers" } else { "suppliers" }, r.party_id))];
                    row.extend(cells(r).iter().map(|v| Cell::number(money(v))));
                    row
                })
                .collect();
            let mut footer = vec![Cell::text("Total")];
            for i in 0..6 {
                footer.push(Cell::number(money(&total(rows.iter().map(|r| cells(r)[i].as_str())))));
            }
            table.footer = Some(footer);
            table.empty = "Nothing outstanding.".into();
            let note = "Posted documents with a balance, by due date against today (UTC). Amounts are in each document's currency.";
            page(&session, if receivable { "A/R aging" } else { "A/P aging" }, vec![Section::Text(note.into()), Section::Table(table)])
        }
        Err(err) => failure(&session, err),
    }
}

/// R8: per product, with a total value.
async fn valuation(State(st): State<AppState>, session: Session) -> Response {
    match reporting::inventory_valuation(&st.pool).await {
        Ok(rows) => {
            let mut table = Table::new(&["SKU", "Product", "On hand#", "Average cost#", "Value#"]);
            table.rows = rows
                .iter()
                .map(|r| vec![Cell::text(&r.sku), Cell::text(&r.name), Cell::number(plain(&r.qty_on_hand)), Cell::number(money(&r.avg_unit_cost)), Cell::number(money(&r.value_on_hand))])
                .collect();
            table.footer = Some(vec![Cell::text(""), Cell::text("Total value"), Cell::text(""), Cell::text(""), Cell::number(money(&total(rows.iter().map(|r| r.value_on_hand.as_str()))))]);
            table.empty = "No stock movements yet.".into();
            page(&session, "Inventory valuation", vec![Section::Table(table)])
        }
        Err(err) => failure(&session, err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_sums() {
        assert_eq!(units("12.5"), 125_000);
        assert_eq!(units("-0.0001"), -1);
        assert_eq!(decimal(-1), "-0.0001");
        assert_eq!(total(["0.1000", "0.2000", "-0.0500"]), "0.2500");
        assert_eq!(total(["999999999999999.9999", "0.0001"]), "1000000000000000.0000");
    }
}

