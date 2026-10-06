//! Accounting screens (spec/domain.md §13.9, A1 to A5): the fiscal
//! calendar, year-end, exchange rates, and bank statements.

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;

use super::documents::confirm;
use super::forms::{Kind, money, plain};
use super::lookups::{self, Accounts};
use super::master::{echo, field, required, select};
use super::{Action, Cell, Field, FieldKind, FormData, FormView, Section, Session, Table, Values, failure, forbidden, page, see_other};
use crate::banking::{self, LineInput, StatementInput};
use crate::calendar::{self, AccountingPeriodInput, FiscalYearInput};
use crate::currency::{self, ExchangeRateInput};
use crate::error::{Error, Result};
use crate::http::AppState;
use crate::yearend;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/periods", get(periods))
        .route("/fiscal-years/new", get(year_new))
        .route("/fiscal-years", get(|| async { see_other("/periods") }).post(year_create))
        .route("/fiscal-years/{id}", get(year_edit).post(year_update))
        .route("/fiscal-years/{id}/close", get(close_page).post(close_year))
        .route("/fiscal-years/{id}/reopen", post(reopen_year))
        .route("/accounting-periods/new", get(period_new))
        .route("/accounting-periods", get(|| async { see_other("/periods") }).post(period_create))
        .route("/accounting-periods/{id}", get(period_edit).post(period_update))
        .route("/accounting-periods/{id}/toggle", post(period_toggle))
        .route("/exchange-rates", get(rates).post(rate_create))
        .route("/exchange-rates/new", get(rate_new))
        .route("/exchange-rates/{currency}/{date}", get(rate_edit).post(rate_update))
        .route("/exchange-rates/{currency}/{date}/delete", get(rate_confirm_delete).post(rate_delete))
        .route("/bank-statements", get(statements).post(statement_create))
        .route("/bank-statements/new", get(statement_new))
        .route("/bank-statements/{id}", get(statement_page).post(statement_update))
        .route("/bank-statements/{id}/edit", get(statement_edit))
        .route("/bank-statements/{id}/delete", get(statement_confirm_delete).post(statement_delete))
        .route("/bank-statements/{id}/{action}", post(statement_act))
        .route("/bank-statement-lines/{id}/{action}", post(line_act))
}

fn to_value<T: serde::Serialize>(t: T) -> Value {
    serde_json::to_value(t).unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------
// A1 periods, A2 year-end
// ---------------------------------------------------------------------------

/// A1: each fiscal year with its periods; each period closes or reopens in
/// one step. A2: closing a year, and reopening the latest closed one.
async fn periods(State(st): State<AppState>, session: Session) -> Response {
    let result: Result<Vec<Section>> = async {
        let years = calendar::list_fiscal_years(&st.pool).await?;
        let periods = calendar::list_accounting_periods(&st.pool).await?;
        let latest_closed = years.iter().filter(|y| y.status == "closed").max_by(|a, b| a.start_date.cmp(&b.start_date)).map(|y| y.id);
        let mut sections = vec![Section::Actions(vec![Action::link("New fiscal year", "/fiscal-years/new"), Action::link("New period", "/accounting-periods/new")], None)];
        if years.is_empty() {
            sections.push(Section::Text("No fiscal years yet.".into()));
        }
        for y in years.iter().rev() {
            let mut table = Table::new(&["Period", "Start", "End", "Status", ""]).titled(format!("{} ({} to {}), {}", y.name, y.start_date, y.end_date, y.status));
            table.rows = periods
                .iter()
                .filter(|p| p.fiscal_year_id == y.id)
                .map(|p| {
                    vec![
                        Cell::link(&p.name, format!("/accounting-periods/{}", p.id)),
                        Cell::text(&p.start_date),
                        Cell::text(&p.end_date),
                        Cell::text(&p.status),
                        Cell::text(""),
                    ]
                })
                .collect();
            table.empty = "No periods yet; posting creates each month's as it is needed.".into();
            sections.push(Section::Table(table));
            let mut actions = vec![Action::link("Edit year", format!("/fiscal-years/{}", y.id))];
            for p in periods.iter().filter(|p| p.fiscal_year_id == y.id) {
                let verb = if p.status == "open" { "Close" } else { "Reopen" };
                actions.push(Action::post(&format!("{verb} {}", p.name), format!("/accounting-periods/{}/toggle", p.id)));
            }
            if session.user.is_admin {
                if y.status == "open" {
                    actions.push(Action::link("Year-end close", format!("/fiscal-years/{}/close", y.id)));
                } else if Some(y.id) == latest_closed {
                    actions.push(Action::post("Reopen year", format!("/fiscal-years/{}/reopen", y.id)).danger());
                }
            }
            sections.push(Section::Actions(actions, None));
        }
        Ok(sections)
    }
    .await;
    match result {
        Ok(sections) => page(&session, "Periods", sections),
        Err(err) => failure(&session, err),
    }
}

const YEAR_FIELDS: [(&str, Kind); 3] = [("name", Kind::Str), ("start_date", Kind::Str), ("end_date", Kind::Str)];

fn year_form(session: &Session, record: &Value, id: Option<i64>, error: Option<String>) -> Response {
    let fields = vec![
        required(field("Name", "name", FieldKind::Text, record)),
        required(field("Start", "start_date", FieldKind::Date, record)),
        required(field("End", "end_date", FieldKind::Date, record)),
    ];
    let action = id.map_or("/fiscal-years".to_string(), |id| format!("/fiscal-years/{id}"));
    let title = if id.is_some() { "Edit fiscal year" } else { "New fiscal year" };
    page(session, title, vec![Section::Form(FormView { get: false, action, fields, submit: "Save".into(), error, cancel: Some("/periods".into()) })])
}

async fn year_new(session: Session) -> Response {
    year_form(&session, &json!({}), None, None)
}

async fn year_create(State(st): State<AppState>, session: Session, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    let result = match Values::from_form(&form, &YEAR_FIELDS).and_then(Values::into::<FiscalYearInput>) {
        Ok(y) => calendar::create_fiscal_year(&st.pool, y).await.map(|_| ()),
        Err(err) => Err(err),
    };
    match result {
        Ok(()) => see_other("/periods"),
        Err(err) => year_form(&session, &echo(&form, &YEAR_FIELDS), None, Some(err.message)),
    }
}

async fn year_edit(State(st): State<AppState>, session: Session, Path(id): Path<i64>) -> Response {
    match calendar::get_fiscal_year(&st.pool, id).await {
        Ok(y) => year_form(&session, &to_value(y), Some(id), None),
        Err(err) => failure(&session, err),
    }
}

async fn year_update(State(st): State<AppState>, session: Session, Path(id): Path<i64>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    let result = match Values::from_form(&form, &YEAR_FIELDS).and_then(Values::into::<FiscalYearInput>) {
        Ok(y) => calendar::update_fiscal_year(&st.pool, id, y).await,
        Err(err) => Err(err),
    };
    match result {
        Ok(()) => see_other("/periods"),
        Err(err) => year_form(&session, &echo(&form, &YEAR_FIELDS), Some(id), Some(err.message)),
    }
}

/// A2: asks for the retained earnings account, proposing the seeded one,
/// and says what closing will do.
async fn close_page(State(st): State<AppState>, session: Session, Path(id): Path<i64>) -> Response {
    close_form(&st.pool, &session, id, None).await
}

async fn close_form(pool: &PgPool, session: &Session, id: i64, error: Option<String>) -> Response {
    if !session.user.is_admin {
        return forbidden(session);
    }
    let result: Result<(String, Field)> = async {
        let y = calendar::get_fiscal_year(pool, id).await?;
        let seeded = sqlx::query_scalar!("SELECT id FROM accounts WHERE code = '3000' AND account_type = 'equity' AND is_active AND is_postable")
            .fetch_optional(pool)
            .await?;
        let account = select("Retained earnings account", "retained_earnings_account_id", &json!({"retained_earnings_account_id": seeded.map(|s| s.to_string())}), "(choose)", lookups::accounts(pool, Accounts::Equity).await?);
        Ok((format!("{} ({} to {})", y.name, y.start_date, y.end_date), required(account)))
    }
    .await;
    match result {
        Ok((year, account)) => page(
            session,
            &format!("Close {year}"),
            vec![
                Section::Text("Closing the year will:".into()),
                Section::Text("1. post a closing entry on its last day that moves every revenue and expense balance into the retained earnings account (none if nothing has a balance);".into()),
                Section::Text("2. close every period of the year, and the year itself, so nothing more can be posted into it;".into()),
                Section::Text("3. create the next fiscal year, if none covers the following day.".into()),
                Section::Text("It can be undone by reopening the year, newest first.".into()),
                Section::Form(FormView { get: false, action: format!("/fiscal-years/{id}/close"), fields: vec![account], submit: "Close the year".into(), error, cancel: Some("/periods".into()) }),
            ],
        ),
        Err(err) => failure(session, err),
    }
}

async fn close_year(State(st): State<AppState>, session: Session, Path(id): Path<i64>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    if !session.user.is_admin {
        return forbidden(&session);
    }
    let account = form.text("retained_earnings_account_id").parse().unwrap_or(0);
    let result = if account <= 0 { Err(Error::bad_request("Choose the retained earnings account.")) } else { yearend::close(&st.pool, id, account).await };
    match result {
        Ok(closed) => {
            let mut facts = vec![("Closing entry".to_string(), match closed.closing_entry_id {
                Some(e) => Cell::link(format!("#{e}"), format!("/journal-entries/{e}")),
                None => Cell::text("none: nothing to sweep"),
            })];
            facts.push(("Next fiscal year".to_string(), Cell::text(if closed.next_fiscal_year_id.is_some() { "created" } else { "none created" })));
            page(&session, "Year closed", vec![Section::Facts(facts), Section::Actions(vec![Action::link("Back to periods", "/periods")], None)])
        }
        Err(err) => close_form(&st.pool, &session, id, Some(err.message)).await,
    }
}

async fn reopen_year(State(st): State<AppState>, session: Session, Path(id): Path<i64>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    if !session.user.is_admin {
        return forbidden(&session);
    }
    match yearend::reopen(&st.pool, id).await {
        Ok(_) => see_other("/periods"),
        Err(err) => failure(&session, err),
    }
}

const PERIOD_FIELDS: [(&str, Kind); 5] = [("fiscal_year_id", Kind::Int), ("name", Kind::Str), ("start_date", Kind::Str), ("end_date", Kind::Str), ("status", Kind::OptStr)];

async fn period_form(pool: &PgPool, session: &Session, record: &Value, id: Option<i64>, error: Option<String>) -> Response {
    let years = match calendar::list_fiscal_years(pool).await {
        Ok(y) => y.into_iter().map(|y| (y.id.to_string(), y.name)).collect(),
        Err(err) => return failure(session, err),
    };
    let mut fields = vec![
        required(select("Fiscal year", "fiscal_year_id", record, "(choose)", years)),
        required(field("Name", "name", FieldKind::Text, record)),
        required(field("Start", "start_date", FieldKind::Date, record)),
        required(field("End", "end_date", FieldKind::Date, record)),
    ];
    if id.is_some() {
        fields.push(select("Status", "status", record, "open", vec![("closed".into(), "closed".into())]));
    }
    let action = id.map_or("/accounting-periods".to_string(), |id| format!("/accounting-periods/{id}"));
    let title = if id.is_some() { "Edit period" } else { "New period" };
    page(session, title, vec![Section::Form(FormView { get: false, action, fields, submit: "Save".into(), error, cancel: Some("/periods".into()) })])
}

/// A1: the new period proposes the month after the latest one.
async fn period_new(State(st): State<AppState>, session: Session) -> Response {
    let proposal = sqlx::query!(
        r#"SELECT p.fiscal_year_id,
               to_char(p.end_date + 1, 'YYYY-MM') AS "name!",
               (p.end_date + 1)::text AS "start!",
               (date_trunc('month', p.end_date + 1) + interval '1 month - 1 day')::date::text AS "end!"
           FROM accounting_periods p ORDER BY p.end_date DESC LIMIT 1"#
    )
    .fetch_optional(&st.pool)
    .await;
    let record = match proposal {
        Ok(Some(p)) => json!({"fiscal_year_id": p.fiscal_year_id.to_string(), "name": p.name, "start_date": p.start, "end_date": p.end}),
        Ok(None) => json!({}),
        Err(err) => return failure(&session, err.into()),
    };
    period_form(&st.pool, &session, &record, None, None).await
}

async fn period_create(State(st): State<AppState>, session: Session, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    let result = match Values::from_form(&form, &PERIOD_FIELDS).and_then(Values::into::<AccountingPeriodInput>) {
        Ok(p) => calendar::create_accounting_period(&st.pool, p).await.map(|_| ()),
        Err(err) => Err(err),
    };
    match result {
        Ok(()) => see_other("/periods"),
        Err(err) => period_form(&st.pool, &session, &echo(&form, &PERIOD_FIELDS), None, Some(err.message)).await,
    }
}

async fn period_edit(State(st): State<AppState>, session: Session, Path(id): Path<i64>) -> Response {
    match calendar::get_accounting_period(&st.pool, id).await {
        Ok(p) => period_form(&st.pool, &session, &to_value(p), Some(id), None).await,
        Err(err) => failure(&session, err),
    }
}

async fn period_update(State(st): State<AppState>, session: Session, Path(id): Path<i64>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    let result = match Values::from_form(&form, &PERIOD_FIELDS).and_then(Values::into::<AccountingPeriodInput>) {
        Ok(p) => calendar::update_accounting_period(&st.pool, id, p).await,
        Err(err) => Err(err),
    };
    match result {
        Ok(()) => see_other("/periods"),
        Err(err) => period_form(&st.pool, &session, &echo(&form, &PERIOD_FIELDS), Some(id), Some(err.message)).await,
    }
}

/// A1: closes an open period or reopens a closed one, in one step.
async fn period_toggle(State(st): State<AppState>, session: Session, Path(id): Path<i64>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    let result = async {
        let p = calendar::get_accounting_period(&st.pool, id).await?;
        let status = if p.status == "open" { "closed" } else { "open" };
        let input = AccountingPeriodInput { fiscal_year_id: i64::from(p.fiscal_year_id), name: p.name, start_date: p.start_date, end_date: p.end_date, status: Some(status.into()) };
        calendar::update_accounting_period(&st.pool, id, input).await
    }
    .await;
    match result {
        Ok(()) => see_other("/periods"),
        Err(err) => failure(&session, err),
    }
}

// ---------------------------------------------------------------------------
// A3 exchange rates
// ---------------------------------------------------------------------------

async fn rates(State(st): State<AppState>, session: Session) -> Response {
    match currency::list_exchange_rates(&st.pool).await {
        Ok(list) => {
            let mut table = Table::new(&["Currency", "Date", "Rate#", ""]);
            table.rows = list
                .iter()
                .map(|r| {
                    vec![
                        Cell::link(&r.currency_code, format!("/exchange-rates/{}/{}", r.currency_code, r.rate_date)),
                        Cell::text(&r.rate_date),
                        Cell::number(&r.rate),
                        Cell::link("delete", format!("/exchange-rates/{}/{}/delete", r.currency_code, r.rate_date)),
                    ]
                })
                .collect();
            table.empty = "No exchange rates yet.".into();
            let note = "A rate is how many base-currency units one unit of the currency buys. Postings use the latest rate on or before their date.";
            page(&session, "Exchange rates", vec![Section::Text(note.into()), Section::Actions(vec![Action::link("New rate", "/exchange-rates/new")], None), Section::Table(table)])
        }
        Err(err) => failure(&session, err),
    }
}

const RATE_FIELDS: [(&str, Kind); 3] = [("currency_code", Kind::Str), ("rate_date", Kind::Str), ("rate", Kind::Str)];

async fn rate_form(pool: &PgPool, session: &Session, record: &Value, key: Option<(&str, &str)>, error: Option<String>) -> Response {
    let currencies = match lookups::currencies(pool).await {
        Ok(c) => c,
        Err(err) => return failure(session, err),
    };
    let mut currency = required(select("Currency", "currency_code", record, "(choose)", currencies));
    let mut date = required(field("Date", "rate_date", FieldKind::Date, record));
    currency.readonly = key.is_some();
    date.readonly = key.is_some();
    let fields = vec![currency, date, required(field("Rate", "rate", FieldKind::Decimal, record))];
    let action = key.map_or("/exchange-rates".to_string(), |(c, d)| format!("/exchange-rates/{c}/{d}"));
    let title = if key.is_some() { "Change exchange rate" } else { "New exchange rate" };
    page(session, title, vec![Section::Form(FormView { get: false, action, fields, submit: "Save".into(), error, cancel: Some("/exchange-rates".into()) })])
}

async fn rate_new(State(st): State<AppState>, session: Session) -> Response {
    rate_form(&st.pool, &session, &json!({}), None, None).await
}

async fn rate_create(State(st): State<AppState>, session: Session, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    let result = match Values::from_form(&form, &RATE_FIELDS).and_then(Values::into::<ExchangeRateInput>) {
        Ok(r) => currency::create_exchange_rate(&st.pool, r).await.map(|_| ()),
        Err(err) => Err(err),
    };
    match result {
        Ok(()) => see_other("/exchange-rates"),
        Err(err) => rate_form(&st.pool, &session, &echo(&form, &RATE_FIELDS), None, Some(err.message)).await,
    }
}

async fn rate_edit(State(st): State<AppState>, session: Session, Path((code, date)): Path<(String, String)>) -> Response {
    match currency::list_exchange_rates(&st.pool).await {
        Ok(list) => match list.into_iter().find(|r| r.currency_code == code && r.rate_date == date) {
            Some(r) => rate_form(&st.pool, &session, &to_value(&r), Some((&code, &date)), None).await,
            None => super::missing(&session),
        },
        Err(err) => failure(&session, err),
    }
}

async fn rate_update(State(st): State<AppState>, session: Session, Path((code, date)): Path<(String, String)>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    match currency::update_exchange_rate(&st.pool, &code, &date, &form.text("rate")).await {
        Ok(()) => see_other("/exchange-rates"),
        Err(err) => rate_form(&st.pool, &session, &echo(&form, &RATE_FIELDS), Some((&code, &date)), Some(err.message)).await,
    }
}

async fn rate_confirm_delete(session: Session, Path((code, date)): Path<(String, String)>) -> Response {
    confirm(&session, &format!("the {code} rate of {date}"), format!("/exchange-rates/{code}/{date}/delete"), "/exchange-rates".into())
}

async fn rate_delete(State(st): State<AppState>, session: Session, Path((code, date)): Path<(String, String)>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    match currency::delete_exchange_rate(&st.pool, &code, &date).await {
        Ok(()) => see_other("/exchange-rates"),
        Err(err) => failure(&session, err),
    }
}

// ---------------------------------------------------------------------------
// A4, A5 bank statements
// ---------------------------------------------------------------------------

async fn statements(State(st): State<AppState>, session: Session) -> Response {
    match banking::list(&st.pool).await {
        Ok(list) => {
            let mut table = Table::new(&["Account", "Date", "Reference", "Closing#", "Matched", "Difference#", "Status"]);
            table.rows = list
                .iter()
                .map(|s| {
                    vec![
                        Cell::link(format!("{} {}", s.account_code, s.account_name), format!("/bank-statements/{}", s.id)),
                        Cell::text(&s.statement_date),
                        Cell::text(s.reference.clone().unwrap_or_default()),
                        Cell::number(money(&s.closing_balance)),
                        Cell::text(format!("{} of {}", s.matched_count, s.line_count)),
                        Cell::number(money(&s.difference)),
                        Cell::text(&s.status),
                    ]
                })
                .collect();
            page(&session, "Bank statements", vec![Section::Actions(vec![Action::link("New statement", "/bank-statements/new")], None), Section::Table(table)])
        }
        Err(err) => failure(&session, err),
    }
}

const STATEMENT_FIELDS: [(&str, Kind); 5] =
    [("account_id", Kind::Int), ("statement_date", Kind::Str), ("opening_balance", Kind::Str), ("closing_balance", Kind::Str), ("reference", Kind::OptStr)];

/// A4: the form offers only cash accounts.
async fn statement_form(pool: &PgPool, session: &Session, record: &Value, id: Option<i64>, error: Option<String>) -> Response {
    let accounts = match lookups::accounts(pool, Accounts::Cash).await {
        Ok(a) => a,
        Err(err) => return failure(session, err),
    };
    let fields = vec![
        required(select("Cash account", "account_id", record, "(choose)", accounts)),
        required(field("Statement date", "statement_date", FieldKind::Date, record)),
        required(field("Opening balance", "opening_balance", FieldKind::Decimal, record)),
        required(field("Closing balance", "closing_balance", FieldKind::Decimal, record)),
        field("Reference", "reference", FieldKind::Text, record),
    ];
    let (title, action, cancel) = match id {
        Some(id) => ("Edit statement", format!("/bank-statements/{id}"), format!("/bank-statements/{id}")),
        None => ("New statement", "/bank-statements".to_string(), "/bank-statements".to_string()),
    };
    page(session, title, vec![Section::Form(FormView { get: false, action, fields, submit: "Save".into(), error, cancel: Some(cancel) })])
}

async fn statement_new(State(st): State<AppState>, session: Session) -> Response {
    statement_form(&st.pool, &session, &json!({"opening_balance": "0", "closing_balance": "0"}), None, None).await
}

async fn statement_create(State(st): State<AppState>, session: Session, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    let result = match Values::from_form(&form, &STATEMENT_FIELDS).and_then(Values::into::<StatementInput>) {
        Ok(s) => banking::create(&st.pool, s).await,
        Err(err) => Err(err),
    };
    match result {
        Ok(id) => see_other(&format!("/bank-statements/{id}")),
        Err(err) => statement_form(&st.pool, &session, &echo(&form, &STATEMENT_FIELDS), None, Some(err.message)).await,
    }
}

async fn statement_edit(State(st): State<AppState>, session: Session, Path(id): Path<i64>) -> Response {
    match banking::get(&st.pool, id).await {
        Ok(s) => {
            let mut record = to_value(&s);
            record["opening_balance"] = Value::String(plain(&s.opening_balance));
            record["closing_balance"] = Value::String(plain(&s.closing_balance));
            statement_form(&st.pool, &session, &record, Some(id), None).await
        }
        Err(err) => failure(&session, err),
    }
}

async fn statement_update(State(st): State<AppState>, session: Session, Path(id): Path<i64>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    let result = match Values::from_form(&form, &STATEMENT_FIELDS).and_then(Values::into::<StatementInput>) {
        Ok(s) => banking::update(&st.pool, id, s).await,
        Err(err) => Err(err),
    };
    match result {
        Ok(()) => see_other(&format!("/bank-statements/{id}")),
        Err(err) => statement_form(&st.pool, &session, &echo(&form, &STATEMENT_FIELDS), Some(id), Some(err.message)).await,
    }
}

async fn statement_confirm_delete(State(st): State<AppState>, session: Session, Path(id): Path<i64>) -> Response {
    match banking::get(&st.pool, id).await {
        Ok(s) => confirm(&session, &format!("the {} statement of {} and its lines", s.account_code, s.statement_date), format!("/bank-statements/{id}/delete"), format!("/bank-statements/{id}")),
        Err(err) => failure(&session, err),
    }
}

async fn statement_delete(State(st): State<AppState>, session: Session, Path(id): Path<i64>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    match banking::delete(&st.pool, id).await {
        Ok(()) => see_other("/bank-statements"),
        Err(err) => statement(&st.pool, &session, id, false, Some(err.message), None).await,
    }
}

/// A5: add a line, import CSV, auto-match, reconcile, reopen.
async fn statement_act(State(st): State<AppState>, session: Session, Path((id, action)): Path<(i64, String)>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    let outcome: Result<Option<String>> = match action.as_str() {
        "lines" => {
            let line = LineInput { txn_date: form.text("txn_date"), description: form.text("description"), reference: Some(form.text("reference")).filter(|r| !r.is_empty()), amount: form.text("amount") };
            banking::add_line(&st.pool, id, line).await.map(|_| None)
        }
        "import" => banking::import(&st.pool, id, &form.text("csv")).await.map(|n| Some(format!("Imported {n} line(s)."))),
        "auto-match" => banking::auto_match(&st.pool, id).await.map(|n| Some(format!("Matched {n} line(s)."))),
        "reconcile" => banking::reconcile(&st.pool, id).await.map(|_| None),
        "reopen" if session.user.is_admin => banking::reopen(&st.pool, id).await.map(|_| None),
        "reopen" => Err(Error::forbidden("Only an administrator can reopen a statement.")),
        _ => return super::not_found(session).await,
    };
    match outcome {
        Ok(None) => see_other(&format!("/bank-statements/{id}")),
        Ok(Some(notice)) => statement(&st.pool, &session, id, false, None, Some(notice)).await,
        Err(err) => statement(&st.pool, &session, id, false, Some(err.message), None).await,
    }
}

/// A5: match a line to a journal line, unmatch it, or delete it.
async fn line_act(State(st): State<AppState>, session: Session, Path((line, action)): Path<(i64, String)>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    let statement_id = match sqlx::query_scalar!("SELECT statement_id FROM bank_statement_lines WHERE id = $1::int8", line).fetch_optional(&st.pool).await {
        Ok(Some(s)) => i64::from(s),
        Ok(None) => return super::missing(&session),
        Err(err) => return failure(&session, err.into()),
    };
    let outcome = match action.as_str() {
        "match" => match form.text("journal_line_id").parse::<i64>() {
            Ok(journal_line) => banking::match_line(&st.pool, line, journal_line).await,
            Err(_) => Err(Error::bad_request("Choose a journal line to match.")),
        },
        "unmatch" => banking::unmatch_line(&st.pool, line).await,
        "delete" => banking::delete_line(&st.pool, line).await,
        _ => return super::not_found(session).await,
    };
    match outcome {
        Ok(()) => see_other(&format!("/bank-statements/{statement_id}")),
        Err(err) => statement(&st.pool, &session, statement_id, false, Some(err.message), None).await,
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct StatementQuery {
    all: String,
}

async fn statement_page(State(st): State<AppState>, session: Session, Path(id): Path<i64>, Query(q): Query<StatementQuery>) -> Response {
    statement(&st.pool, &session, id, !q.all.is_empty(), None, None).await
}

/// A5: the statement, its lines and matches, and what can be done to it.
async fn statement(pool: &PgPool, session: &Session, id: i64, all: bool, error: Option<String>, notice: Option<String>) -> Response {
    let result: Result<Vec<Section>> = async {
        let s = banking::get(pool, id).await?;
        let lines = banking::lines(pool, id).await?;
        let candidates = banking::candidates(pool, id).await?;
        let open = s.status == "open";
        let path = format!("/bank-statements/{id}");
        let facts = vec![
            ("Account".to_string(), Cell::text(format!("{} {}", s.account_code, s.account_name))),
            ("Statement date".to_string(), Cell::text(&s.statement_date)),
            ("Reference".to_string(), Cell::text(s.reference.clone().unwrap_or_default())),
            ("Opening".to_string(), Cell::number(money(&s.opening_balance))),
            ("Lines".to_string(), Cell::number(money(&s.lines_total))),
            ("Closing".to_string(), Cell::number(money(&s.closing_balance))),
            ("Difference".to_string(), Cell::number(money(&s.difference))),
            ("Matched".to_string(), Cell::text(format!("{} of {}", s.matched_count, s.line_count))),
            ("Status".to_string(), Cell::text(&s.status)),
        ];
        let mut actions = Vec::new();
        if open {
            actions.push(Action::post("Auto-match", format!("{path}/auto-match")));
            actions.push(Action::post("Reconcile", format!("{path}/reconcile")));
            actions.push(Action::link("Edit", format!("{path}/edit")));
            actions.push(Action::link("Delete", format!("{path}/delete")).danger());
        } else if session.user.is_admin {
            actions.push(Action::post("Reopen", format!("{path}/reopen")).danger());
        }
        let mut sections = Vec::new();
        if let Some(n) = notice {
            sections.push(Section::Text(n));
        }
        sections.push(Section::Facts(facts));
        sections.push(Section::Actions(actions, error));

        let mut table = Table::new(&["#", "Date", "Description", "Reference", "Amount#", "Matched to"]);
        table.rows = lines
            .iter()
            .map(|l| {
                vec![
                    Cell::text(l.line_no.to_string()),
                    Cell::text(&l.txn_date),
                    Cell::text(&l.description),
                    Cell::text(l.reference.clone().unwrap_or_default()),
                    Cell::number(money(&l.amount)),
                    match l.journal_entry_id {
                        Some(e) => Cell::link(format!("entry #{e} of {}", l.entry_date.clone().unwrap_or_default()), format!("/journal-entries/{e}")),
                        None => Cell::text("unmatched"),
                    },
                ]
            })
            .collect();
        table.empty = "No lines yet.".into();
        sections.push(Section::Table(table));

        if open {
            // Each line's match: candidates of its amount, or all of them.
            for l in &lines {
                let label = format!("Line {} ({}, {})", l.line_no, money(&l.amount), l.description);
                let line_path = format!("/bank-statement-lines/{}", l.id);
                if l.journal_line_id.is_some() {
                    sections.push(Section::Actions(vec![Action::post(&format!("Unmatch line {}", l.line_no), format!("{line_path}/unmatch")), Action::post(&format!("Delete line {}", l.line_no), format!("{line_path}/delete")).danger()], None));
                    continue;
                }
                let offered: Vec<(String, String)> = candidates
                    .iter()
                    .filter(|c| all || super::reports::units(&c.amount) == super::reports::units(&l.amount))
                    .map(|c| (c.journal_line_id.to_string(), format!("{} entry #{} {} {}", c.entry_date, c.journal_entry_id, money(&c.amount), c.memo.clone().unwrap_or_default())))
                    .collect();
                if offered.is_empty() {
                    sections.push(Section::Text(format!("{label}: no unclaimed journal line has this amount.")));
                } else {
                    let mut pick = Field::new(&label, "journal_line_id", FieldKind::Select);
                    pick.options = offered;
                    sections.push(Section::Form(FormView { get: false, action: format!("{line_path}/match"), fields: vec![pick], submit: "Match".into(), error: None, cancel: None }));
                }
                sections.push(Section::Actions(vec![Action::post(&format!("Delete line {}", l.line_no), format!("{line_path}/delete")).danger()], None));
            }
            sections.push(Section::Actions(
                vec![if all { Action::link("Show candidates of each line's amount", path.clone()) } else { Action::link("Show all candidates", format!("{path}?all=1")) }],
                None,
            ));
            sections.push(Section::Heading("Add a line".into()));
            sections.push(Section::Form(FormView {
                get: false,
                action: format!("{path}/lines"),
                fields: vec![
                    required(Field::new("Date", "txn_date", FieldKind::Date)),
                    required(Field::new("Description", "description", FieldKind::Text)),
                    Field::new("Reference", "reference", FieldKind::Text),
                    Field { help: Some("Signed from the books' side: a deposit is positive.".into()), ..required(Field::new("Amount", "amount", FieldKind::Decimal)) },
                ],
                submit: "Add line".into(),
                error: None,
                cancel: None,
            }));
            sections.push(Section::Heading("Import CSV".into()));
            sections.push(Section::Form(FormView {
                get: false,
                action: format!("{path}/import"),
                fields: vec![Field { help: Some("Columns date,description,amount[,reference]; a header row is skipped. Any bad row rejects the whole import.".into()), ..Field::new("Paste CSV", "csv", FieldKind::TextArea) }],
                submit: "Import".into(),
                error: None,
                cancel: None,
            }));
        }
        if all {
            let mut table = Table::new(&["Date", "Entry", "Reference", "Memo", "Amount#"]).titled("All candidates");
            table.rows = candidates
                .iter()
                .map(|c| vec![Cell::text(&c.entry_date), Cell::link(format!("#{}", c.journal_entry_id), format!("/journal-entries/{}", c.journal_entry_id)), Cell::text(c.reference.clone().unwrap_or_default()), Cell::text(c.memo.clone().unwrap_or_default()), Cell::number(money(&c.amount))])
                .collect();
            table.empty = "No unclaimed journal lines on this account.".into();
            sections.push(Section::Table(table));
        }
        Ok(sections)
    }
    .await;
    match result {
        Ok(sections) => page(session, "Bank statement", sections),
        Err(err) => failure(session, err),
    }
}

