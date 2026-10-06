//! Invoices, bills and credit notes (spec/domain.md §13.4, D1 to D7) and
//! payments (§13.5, P1 to P4). The line-editing form is shared with orders.

use std::collections::HashMap;

use askama::Template;
use axum::Router;
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{get, post};
use serde_json::{Value, json};
use sqlx::PgPool;

use super::forms::{Kind, money, plain, show};
use super::lookups::{self, Accounts, with_blank};
use super::master::{echo, field, required, select};
use super::view::Chrome;
use super::{Action, Cell, Field, FieldKind, FormData, FormView, Section, Session, Table, Values, failure, page, render, see_other};
use crate::documents::{self, DocumentInput, Kind as DocKind, LineInput};
use crate::error::{Error, Result};
use crate::http::AppState;
use crate::payments::{self, PaymentInput, PaymentKind};
use crate::printing::{self, Printable};
use crate::settlement::{self, Settler};

/// A document kind as its screens show it.
pub struct Screen {
    pub kind: &'static DocKind,
    pub title: &'static str,
    pub noun: &'static str,
    /// The sales side (customers) rather than purchasing (suppliers).
    pub sales: bool,
    pub printable: &'static Printable,
    /// Credit notes settle invoices or bills.
    pub settler: Option<&'static Settler>,
    /// The label of the second date, when the kind has one.
    pub due_label: &'static str,
}

pub static SCREENS: [Screen; 4] = [
    Screen { kind: &documents::SALES_INVOICES, title: "Invoices", noun: "invoice", sales: true, printable: &printing::PRINTABLES[0], settler: None, due_label: "Due date" },
    Screen { kind: &documents::PURCHASE_BILLS, title: "Bills", noun: "bill", sales: false, printable: &printing::PRINTABLES[1], settler: None, due_label: "Due date" },
    Screen {
        kind: &documents::SALES_CREDIT_NOTES,
        title: "Credit notes",
        noun: "credit note",
        sales: true,
        printable: &printing::PRINTABLES[2],
        settler: Some(&settlement::SALES_CREDIT_NOTE),
        due_label: "",
    },
    Screen {
        kind: &documents::PURCHASE_CREDIT_NOTES,
        title: "Supplier credits",
        noun: "supplier credit",
        sales: false,
        printable: &printing::PRINTABLES[3],
        settler: Some(&settlement::PURCHASE_CREDIT_NOTE),
        due_label: "",
    },
];

fn screen_routes(s: &'static Screen) -> Router<AppState> {
    let base = format!("/{}", s.kind.path);
    Router::new()
        .route(
            &base,
            get(move |st: State<AppState>, se: Session| list(st, se, s)).post(move |st: State<AppState>, se: Session, f: FormData| create(st, se, s, f)),
        )
        .route(&format!("{base}/new"), get(move |st: State<AppState>, se: Session| new(st, se, s)))
        .route(
            &format!("{base}/{{id}}"),
            get(move |st: State<AppState>, se: Session, id: Path<i64>| detail_page(st, se, s, id))
                .post(move |st: State<AppState>, se: Session, id: Path<i64>, f: FormData| update(st, se, s, id, f)),
        )
        .route(&format!("{base}/{{id}}/edit"), get(move |st: State<AppState>, se: Session, id: Path<i64>| edit(st, se, s, id)))
        .route(
            &format!("{base}/{{id}}/delete"),
            get(move |st: State<AppState>, se: Session, id: Path<i64>| confirm_delete(st, se, s, id))
                .post(move |st: State<AppState>, se: Session, id: Path<i64>, f: FormData| delete(st, se, s, id, f)),
        )
        .route(
            &format!("{base}/{{id}}/{{action}}"),
            post(move |st: State<AppState>, se: Session, path: Path<(i64, String)>, f: FormData| act(st, se, s, path, f)),
        )
}

fn payment_routes(s: &'static PaymentScreen) -> Router<AppState> {
    let base = format!("/{}", s.kind.path);
    Router::new()
        .route(
            &base,
            get(move |st: State<AppState>, se: Session| payment_list(st, se, s))
                .post(move |st: State<AppState>, se: Session, f: FormData| payment_create(st, se, s, f)),
        )
        .route(&format!("{base}/new"), get(move |st: State<AppState>, se: Session| payment_new(st, se, s)))
        .route(
            &format!("{base}/{{id}}"),
            get(move |st: State<AppState>, se: Session, id: Path<i64>| payment_detail_page(st, se, s, id))
                .post(move |st: State<AppState>, se: Session, id: Path<i64>, f: FormData| payment_update(st, se, s, id, f)),
        )
        .route(&format!("{base}/{{id}}/edit"), get(move |st: State<AppState>, se: Session, id: Path<i64>| payment_edit(st, se, s, id)))
        .route(
            &format!("{base}/{{id}}/delete"),
            get(move |st: State<AppState>, se: Session, id: Path<i64>| payment_confirm_delete(st, se, s, id))
                .post(move |st: State<AppState>, se: Session, id: Path<i64>, f: FormData| payment_delete(st, se, s, id, f)),
        )
        .route(
            &format!("{base}/{{id}}/{{action}}"),
            post(move |st: State<AppState>, se: Session, path: Path<(i64, String)>, f: FormData| payment_act(st, se, s, path, f)),
        )
}

pub fn routes() -> Router<AppState> {
    let documents = SCREENS.iter().fold(Router::new(), |r, s| r.merge(screen_routes(s)));
    PAYMENT_SCREENS.iter().fold(documents, |r, s| r.merge(payment_routes(s)))
}

/// Whether a decimal is above zero.
pub fn positive(value: &str) -> bool {
    !value.starts_with('-') && value.bytes().any(|b| (b'1'..=b'9').contains(&b))
}

/// `options` with each of `current` added if missing, then a blank first.
fn keeping(mut options: lookups::Options, current: impl IntoIterator<Item = String>) -> lookups::Options {
    for value in current {
        if !value.is_empty() && !options.iter().any(|(v, _)| *v == value) {
            options.push((value.clone(), value));
        }
    }
    with_blank("", options, "")
}

/// Shows an amount with its currency when that is not the base (G7).
pub fn amount(currency: &str, base: &str, value: &str) -> String {
    if currency == base { money(value) } else { format!("{currency} {}", money(value)) }
}

// ---------------------------------------------------------------------------
// The line-editing form, shared with orders (D2, O2)
// ---------------------------------------------------------------------------

/// One line as the form shows it.
#[derive(Clone, Debug, Default)]
pub struct LineRow {
    pub product_id: String,
    pub description: String,
    pub quantity: String,
    pub price: String,
    pub account_id: String,
    pub tax_code: String,
    pub tax_rate: String,
}

#[derive(Template)]
#[template(path = "docform.html")]
struct DocFormTemplate<'a> {
    chrome: &'a Chrome,
    title: &'a str,
    action: &'a str,
    submit: &'a str,
    cancel: &'a str,
    fields: &'a [Field],
    lines: &'a [LineRow],
    blank: LineRow,
    products: &'a [(String, String)],
    accounts: &'a [(String, String)],
    taxes: &'a [(String, String)],
    price_label: &'a str,
    account_label: &'a str,
    client_data: &'a str,
    error: Option<&'a str>,
}

/// Everything about a document form but its values.
pub struct FormSetup {
    pub kind: &'static DocKind,
    pub sales: bool,
    pub due_label: &'static str,
    pub title: String,
    pub action: String,
    pub cancel: String,
}

/// The line editor form for any kind with lines: invoices, bills, credit
/// notes and orders.
pub async fn document_form(pool: &PgPool, session: &Session, setup: &FormSetup, header: &Value, lines: Vec<LineRow>, error: Option<String>) -> Response {
    match document_form_inner(pool, session, setup, header, lines, error).await {
        Ok(response) => response,
        Err(err) => failure(session, err),
    }
}

async fn document_form_inner(pool: &PgPool, session: &Session, setup: &FormSetup, header: &Value, mut lines: Vec<LineRow>, error: Option<String>) -> Result<Response> {
    let k = setup.kind;
    let party_label = if setup.sales { "Customer" } else { "Supplier" };
    let mut fields = vec![
        required(field(&format!("{} number", capitalize(k.noun.rsplit(' ').next().unwrap_or(k.noun))), k.number, FieldKind::Text, header)),
        required(select(party_label, k.party, header, "(choose)", lookups::parties(pool, setup.sales).await?)),
        required(field("Date", k.date, FieldKind::Date, header)),
    ];
    if let Some(due) = k.due {
        fields.push(field(setup.due_label, due, FieldKind::Date, header));
    }
    let mut currency = required(select("Currency", "currency_code", header, "(choose)", lookups::currencies(pool).await?));
    if currency.value.is_empty() {
        currency.value = lookups::base_currency(pool).await?;
    }
    fields.push(currency);
    fields.push(field("Reference", "reference", FieldKind::Text, header));
    fields.push(field("Memo", "memo", FieldKind::TextArea, header));

    let product_choices = lookups::products(pool).await?;
    let accounts = lookups::accounts(pool, Accounts::Postable).await?;
    let taxes = lookups::tax_codes(pool).await?;
    let products = keeping(product_choices.iter().map(|p| (p.id.to_string(), p.label.clone())).collect(), lines.iter().map(|l| l.product_id.clone()));
    let account_options = keeping(accounts, lines.iter().map(|l| l.account_id.clone()));
    let tax_options = keeping(taxes.iter().map(|(c, n, _)| (c.clone(), format!("{c} {n}"))).collect(), lines.iter().map(|l| l.tax_code.clone()));

    let party_currency: HashMap<String, String> = party_currencies(pool, setup.sales).await?;
    let client = json!({
        "products": product_choices.iter().map(|p| (p.id.to_string(), json!({
            "description": p.description,
            "tax_code": p.tax_code,
            "price": if setup.sales { Value::String(plain(&p.unit_price)) } else { Value::Null },
            "account": if setup.sales { p.revenue_account_id.map(|a| Value::String(a.to_string())).unwrap_or(Value::Null) } else { Value::Null },
        }))).collect::<serde_json::Map<_, _>>(),
        "taxes": taxes.iter().map(|(c, _, r)| (c.clone(), Value::String(r.clone()))).collect::<serde_json::Map<_, _>>(),
        "partyCurrency": party_currency,
    });
    // Inside <script>, "<" could end the element; JSON allows it escaped.
    let client_data = client.to_string().replace('<', "\\u003c");

    if lines.is_empty() {
        lines.push(LineRow { quantity: "1".into(), tax_rate: "0".into(), ..LineRow::default() });
    }
    let chrome = super::chrome(session);
    let (price_label, account_label) = if setup.sales { ("Unit price", "Revenue account") } else { ("Unit cost", "Expense account") };
    Ok(render(DocFormTemplate {
        chrome: &chrome,
        title: &setup.title,
        action: &setup.action,
        submit: "Save",
        cancel: &setup.cancel,
        fields: &fields,
        lines: &lines,
        blank: LineRow { quantity: "1".into(), tax_rate: "0".into(), ..LineRow::default() },
        products: &products,
        accounts: &account_options,
        taxes: &tax_options,
        price_label,
        account_label,
        client_data: &client_data,
        error: error.as_deref(),
    }))
}

pub fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

async fn party_currencies(pool: &PgPool, sales: bool) -> Result<HashMap<String, String>> {
    let rows: Vec<(i32, Option<String>)> = if sales {
        sqlx::query!(r#"SELECT id, currency_code::text AS currency FROM customers"#).fetch_all(pool).await?.into_iter().map(|r| (r.id, r.currency)).collect()
    } else {
        sqlx::query!(r#"SELECT id, currency_code::text AS currency FROM suppliers"#).fetch_all(pool).await?.into_iter().map(|r| (r.id, r.currency)).collect()
    };
    Ok(rows.into_iter().filter_map(|(id, c)| c.map(|c| (id.to_string(), c))).collect())
}

/// The posted header, as the form shows it again after a refusal.
pub fn posted_header(kind: &DocKind, form: &FormData) -> Value {
    let mut spec = vec![(kind.number, Kind::Str), (kind.party, Kind::Str), (kind.date, Kind::Str), ("currency_code", Kind::Str), ("reference", Kind::Str), ("memo", Kind::Str)];
    if let Some(due) = kind.due {
        spec.push((due, Kind::Str));
    }
    echo(form, &spec)
}

/// The posted lines: one of each field per line, in order; a line left
/// entirely blank is dropped.
pub fn posted_lines(form: &FormData) -> Vec<LineRow> {
    let col = |name: &str| form.all(name);
    let (products, descriptions, quantities, prices, accounts, taxes, rates) =
        (col("line_product_id"), col("line_description"), col("line_quantity"), col("line_price"), col("line_account_id"), col("line_tax_code"), col("line_tax_rate"));
    let at = |v: &Vec<String>, i: usize| v.get(i).cloned().unwrap_or_default();
    (0..descriptions.len())
        .map(|i| LineRow {
            product_id: at(&products, i),
            description: at(&descriptions, i),
            quantity: at(&quantities, i),
            price: at(&prices, i),
            account_id: at(&accounts, i),
            tax_code: at(&taxes, i),
            tax_rate: at(&rates, i),
        })
        .filter(|l| !(l.product_id.is_empty() && l.description.trim().is_empty() && l.price.trim().is_empty() && l.account_id.is_empty() && l.tax_code.is_empty()))
        .collect()
}

fn optional(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

fn optional_id(s: &str) -> Result<Option<i64>> {
    match optional(s) {
        None => Ok(None),
        Some(s) => s.parse().map(Some).map_err(|_| Error::bad_request(format!("{s:?} is not a valid choice"))),
    }
}

/// The document a posted form describes.
pub fn posted_document(kind: &DocKind, form: &FormData) -> Result<DocumentInput> {
    let lines = posted_lines(form)
        .into_iter()
        .map(|l| {
            Ok(LineInput {
                product_id: optional_id(&l.product_id)?,
                description: l.description,
                quantity: optional(&l.quantity),
                price: optional(&l.price),
                account_id: optional_id(&l.account_id)?,
                tax_code: optional(&l.tax_code),
                tax_rate: optional(&l.tax_rate),
            })
        })
        .collect::<Result<_>>()?;
    Ok(DocumentInput {
        number: form.text(kind.number),
        party_id: form.text(kind.party).trim().parse().unwrap_or(0),
        date: form.text(kind.date),
        due_date: kind.due.and_then(|d| optional(&form.text(d))),
        currency_code: form.text("currency_code"),
        reference: optional(&form.text("reference")),
        memo: optional(&form.text("memo")),
        lines,
    })
}

/// A saved document's lines, as the form shows them.
pub fn saved_lines(kind: &DocKind, lines: &[Value]) -> Vec<LineRow> {
    lines
        .iter()
        .map(|l| LineRow {
            product_id: show(&l["product_id"]),
            description: show(&l["description"]),
            quantity: plain(&show(&l["quantity"])),
            price: plain(&show(&l[kind.price])),
            account_id: show(&l[kind.account]),
            tax_code: show(&l["tax_code"]),
            tax_rate: plain(&show(&l["tax_rate"])),
        })
        .collect()
}

fn setup(s: &Screen, title: String, action: String, cancel: String) -> FormSetup {
    FormSetup { kind: s.kind, sales: s.sales, due_label: s.due_label, title, action, cancel }
}

// ---------------------------------------------------------------------------
// D1 to D7
// ---------------------------------------------------------------------------

async fn list(State(st): State<AppState>, session: Session, s: &'static Screen) -> Response {
    let result: Result<Table> = async {
        let docs = documents::list(&st.pool, s.kind).await?;
        let names = lookups::party_names(&st.pool, s.sales).await?;
        let base = lookups::base_currency(&st.pool).await?;
        let mut columns = vec!["Number", if s.sales { "Customer" } else { "Supplier" }, "Date"];
        if s.kind.due.is_some() {
            columns.push("Due");
        }
        columns.extend(["Total#", if s.settler.is_some() { "Unapplied#" } else { "Balance#" }, "Status"]);
        let mut table = Table::new(&columns);
        table.rows = docs
            .iter()
            .map(|d| {
                let currency = show(&d["currency_code"]);
                let id = &d["id"];
                let mut row = vec![
                    Cell::link(show(&d[s.kind.number]), format!("/{}/{id}", s.kind.path)),
                    Cell::text(names.get(&d[s.kind.party].as_i64().unwrap_or(0)).cloned().unwrap_or_default()),
                    Cell::text(show(&d[s.kind.date])),
                ];
                if let Some(due) = s.kind.due {
                    row.push(Cell::text(show(&d[due])));
                }
                row.push(Cell::number(amount(&currency, &base, &show(&d["total"]))));
                row.push(Cell::number(amount(&currency, &base, &show(&d["balance"]))));
                row.push(Cell::text(format!("{}, {}", show(&d["status"]), show(&d[s.kind.settlement]))));
                row
            })
            .collect();
        Ok(table)
    }
    .await;
    match result {
        Ok(table) => page(&session, s.title, vec![Section::Actions(vec![Action::link(&format!("New {}", s.noun), format!("/{}/new", s.kind.path))], None), Section::Table(table)]),
        Err(err) => failure(&session, err),
    }
}

async fn new(State(st): State<AppState>, session: Session, s: &'static Screen) -> Response {
    let setup = setup(s, format!("New {}", s.noun), format!("/{}", s.kind.path), format!("/{}", s.kind.path));
    document_form(&st.pool, &session, &setup, &json!({}), vec![], None).await
}

async fn create(State(st): State<AppState>, session: Session, s: &'static Screen, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    let result = match posted_document(s.kind, &form) {
        Ok(doc) => documents::create(&st.pool, s.kind, doc).await,
        Err(err) => Err(err),
    };
    match result {
        Ok(id) => see_other(&format!("/{}/{id}", s.kind.path)),
        Err(err) => {
            let setup = setup(s, format!("New {}", s.noun), format!("/{}", s.kind.path), format!("/{}", s.kind.path));
            document_form(&st.pool, &session, &setup, &posted_header(s.kind, &form), posted_lines(&form), Some(err.message)).await
        }
    }
}

async fn edit(State(st): State<AppState>, session: Session, s: &'static Screen, Path(id): Path<i64>) -> Response {
    let loaded = async { Ok::<_, Error>((documents::get(&st.pool, s.kind, id).await?, documents::lines(&st.pool, s.kind, id).await?)) }.await;
    match loaded {
        Ok((doc, lines)) => {
            let setup = setup(s, format!("Edit {} {}", s.noun, show(&doc[s.kind.number])), format!("/{}/{id}", s.kind.path), format!("/{}/{id}", s.kind.path));
            document_form(&st.pool, &session, &setup, &doc, saved_lines(s.kind, &lines), None).await
        }
        Err(err) => failure(&session, err),
    }
}

async fn update(State(st): State<AppState>, session: Session, s: &'static Screen, Path(id): Path<i64>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    let result = match posted_document(s.kind, &form) {
        Ok(doc) => documents::update(&st.pool, s.kind, id, doc).await,
        Err(err) => Err(err),
    };
    match result {
        Ok(()) => see_other(&format!("/{}/{id}", s.kind.path)),
        Err(err) => {
            let setup = setup(s, format!("Edit {}", s.noun), format!("/{}/{id}", s.kind.path), format!("/{}/{id}", s.kind.path));
            document_form(&st.pool, &session, &setup, &posted_header(s.kind, &form), posted_lines(&form), Some(err.message)).await
        }
    }
}

/// G6: a delete asks first.
pub fn confirm(session: &Session, what: &str, action: String, back: String) -> Response {
    page(
        session,
        &format!("Delete {what}?"),
        vec![
            Section::Text(format!("This deletes {what} for good.")),
            Section::Actions(vec![Action::post("Delete", action).danger(), Action::link("Keep it", back)], None),
        ],
    )
}

async fn confirm_delete(State(st): State<AppState>, session: Session, s: &'static Screen, Path(id): Path<i64>) -> Response {
    match documents::get(&st.pool, s.kind, id).await {
        Ok(doc) => confirm(&session, &format!("{} {}", s.noun, show(&doc[s.kind.number])), format!("/{}/{id}/delete", s.kind.path), format!("/{}/{id}", s.kind.path)),
        Err(err) => failure(&session, err),
    }
}

async fn delete(State(st): State<AppState>, session: Session, s: &'static Screen, Path(id): Path<i64>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    match documents::delete(&st.pool, s.kind, id).await {
        Ok(()) => see_other(&format!("/{}", s.kind.path)),
        Err(err) => detail(&st.pool, &session, s, id, Some(err.message), None).await,
    }
}

/// D4, D7: post, unpost, apply, and email, each showing its outcome on the
/// detail screen.
async fn act(State(st): State<AppState>, session: Session, s: &'static Screen, Path((id, action)): Path<(i64, String)>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    let outcome: Result<Option<String>> = match action.as_str() {
        "post" => documents::post(&st.pool, s.kind, id).await.map(|_| None),
        "unpost" if session.user.is_admin => documents::unpost(&st.pool, s.kind, id).await.map(|_| None),
        "unpost" => Err(Error::forbidden("Only an administrator can unpost.")),
        "apply" => match s.settler {
            Some(settler) => settlement::apply(&st.pool, settler, id).await.map(|made| Some(format!("Applied to {} document(s).", made.len()))),
            None => Err(Error::not_found()),
        },
        "email" => email(&st, s.printable, id, &form.text("to")).await.map(Some),
        _ => return super::not_found(session).await,
    };
    match outcome {
        Ok(None) => see_other(&format!("/{}/{id}", s.kind.path)),
        Ok(Some(notice)) => detail(&st.pool, &session, s, id, None, Some(notice)).await,
        Err(err) => detail(&st.pool, &session, s, id, Some(err.message), None).await,
    }
}

/// D7: emails the PDF to the given addresses (comma separated), or the
/// counterparty's email on file, and says where it went. With email off,
/// says so (the API's 501).
pub async fn email(st: &AppState, p: &Printable, id: i64, to: &str) -> Result<String> {
    let mut recipients: Vec<String> = to.split([',', ';']).map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect();
    if recipients.is_empty() {
        let on_file = printing::recipient(&st.pool, p, id).await?.filter(|e| !e.is_empty());
        recipients.push(on_file.ok_or_else(|| Error::unprocessable("No recipient given, and the counterparty has no email on file."))?);
    } else {
        printing::recipient(&st.pool, p, id).await?;
    }
    let Some(mailer) = &st.mailer else {
        return Err(Error::new(
            axum::http::StatusCode::NOT_IMPLEMENTED,
            format!("Email sending is not configured on this server, so nothing was sent to {}.", recipients.join(", ")),
        ));
    };
    let printed = printing::render(&st.pool, p, id).await?;
    let subject = format!("{} {}", p.label, printed.number);
    let note = format!("Please find attached {} {}.", p.label.to_lowercase(), printed.number);
    mailer.send_pdf(&recipients, &subject, &note, &printed.filename(p), printed.pdf).await?;
    Ok(format!("Sent to {}.", recipients.join(", ")))
}

async fn detail_page(State(st): State<AppState>, session: Session, s: &'static Screen, Path(id): Path<i64>) -> Response {
    detail(&st.pool, &session, s, id, None, None).await
}

/// D3: the document, its lines and totals, and the actions its state allows.
async fn detail(pool: &PgPool, session: &Session, s: &'static Screen, id: i64, error: Option<String>, notice: Option<String>) -> Response {
    match detail_sections(pool, session, s, id, error, notice).await {
        Ok((title, sections)) => page(session, &title, sections),
        Err(err) => failure(session, err),
    }
}

async fn detail_sections(pool: &PgPool, session: &Session, s: &'static Screen, id: i64, error: Option<String>, notice: Option<String>) -> Result<(String, Vec<Section>)> {
    let k = s.kind;
    let doc = documents::get(pool, k, id).await?;
    let lines = documents::lines(pool, k, id).await?;
    let names = lookups::party_names(pool, s.sales).await?;
    let products = lookups::product_names(pool).await?;
    let base = lookups::base_currency(pool).await?;
    let currency = show(&doc["currency_code"]);
    let status = show(&doc["status"]);
    let party_id = doc[k.party].as_i64().unwrap_or(0);
    let amt = |field: &str| amount(&currency, &base, &show(&doc[field]));

    let mut facts = vec![
        (if s.sales { "Customer" } else { "Supplier" }.to_string(), Cell::link(names.get(&party_id).cloned().unwrap_or_default(), format!("/{}/{party_id}", if s.sales { "customers" } else { "suppliers" }))),
        ("Date".to_string(), Cell::text(show(&doc[k.date]))),
    ];
    if let Some(due) = k.due {
        facts.push((s.due_label.to_string(), Cell::text(show(&doc[due]))));
    }
    facts.extend([
        ("Currency".to_string(), Cell::text(&currency)),
        ("Status".to_string(), Cell::text(format!("{status}, {}", show(&doc[k.settlement])))),
        ("Total".to_string(), Cell::number(amt("total"))),
        ("Applied".to_string(), Cell::number(amt("amount_applied"))),
        (if s.settler.is_some() { "Unapplied" } else { "Balance" }.to_string(), Cell::number(amt("balance"))),
        ("Reference".to_string(), Cell::text(show(&doc["reference"]))),
        ("Memo".to_string(), Cell::text(show(&doc["memo"]))),
    ]);
    if let Some(entry) = doc["journal_entry_id"].as_i64() {
        facts.push(("Journal entry".to_string(), Cell::link(format!("#{entry}"), format!("/journal-entries/{entry}"))));
    }

    let mut table = Table::new(&["#", "Product", "Description", "Quantity#", if s.sales { "Unit price#" } else { "Unit cost#" }, "Tax code", "Tax %#", "Subtotal#", "Tax#", "Total#"]);
    table.rows = lines
        .iter()
        .map(|l| {
            vec![
                Cell::text(show(&l["line_no"])),
                Cell::text(l["product_id"].as_i64().and_then(|p| products.get(&p).cloned()).unwrap_or_default()),
                Cell::text(show(&l["description"])),
                Cell::number(plain(&show(&l["quantity"]))),
                Cell::number(money(&show(&l[k.price]))),
                Cell::text(show(&l["tax_code"])),
                Cell::number(plain(&show(&l["tax_rate"]))),
                Cell::number(money(&show(&l["line_subtotal"]))),
                Cell::number(money(&show(&l["tax_amount"]))),
                Cell::number(money(&show(&l["line_total"]))),
            ]
        })
        .collect();
    let mut footer = vec![Cell::default(); 9];
    footer[2] = Cell::text("Total");
    footer.push(Cell::number(amt("total")));
    table.footer = Some(footer);
    table.empty = "No lines.".into();

    // D4: the actions this state allows.
    let base_path = format!("/{}/{id}", k.path);
    let mut actions = Vec::new();
    let order_linked = lines.iter().any(|l| !l["order_line_id"].is_null());
    if status == "draft" {
        actions.push(Action::post("Post", format!("{base_path}/post")));
        if !order_linked {
            actions.push(Action::link("Edit", format!("{base_path}/edit")));
        }
        actions.push(Action::link("Delete", format!("{base_path}/delete")).danger());
    }
    if status == "posted" {
        if s.settler.is_some() && positive(&show(&doc["balance"])) {
            actions.push(Action::post("Apply to open documents", format!("{base_path}/apply")));
        }
        if session.user.is_admin {
            actions.push(Action::post("Unpost", format!("{base_path}/unpost")).danger());
        }
    }
    actions.push(Action::link("PDF", format!("/api/{}/{id}/pdf", k.path)));

    let mut sections = Vec::new();
    if let Some(n) = notice {
        sections.push(Section::Text(n));
    }
    sections.push(Section::Facts(facts));
    sections.push(Section::Actions(actions, error));
    if order_linked && status == "draft" {
        sections.push(Section::Text("This document was produced from an order, so it cannot be edited; deleting it returns its quantities to the order.".into()));
    }
    sections.push(Section::Table(table));

    // D5: what a credit note is applied to.
    if let Some(settler) = s.settler {
        sections.push(applications_table(pool, settler, id, s.sales).await?);
    }
    // D7: email.
    sections.push(email_form(&base_path));
    Ok((format!("{} {}", capitalize(s.noun), show(&doc[k.number])), sections))
}

/// D5, P4: the documents a settler is applied to, with links.
pub async fn applications_table(pool: &PgPool, settler: &Settler, id: i64, sales: bool) -> Result<Section> {
    let apps = settlement::applications(pool, settler, id).await?;
    let target = if sales { "sales-invoices" } else { "purchase-bills" };
    let mut table = Table::new(&[if sales { "Invoice" } else { "Bill" }, "Applied#"]).titled("Applied to");
    table.empty = "Not applied to anything yet.".into();
    table.rows = apps
        .iter()
        .map(|a| vec![Cell::link(show(&a["document_number"]), format!("/{target}/{}", a["document_id"])), Cell::number(money(&show(&a["amount_applied"])))])
        .collect();
    Ok(Section::Table(table))
}

/// D7, O7: optional recipients; blank means the counterparty's email.
pub fn email_form(base_path: &str) -> Section {
    let mut to = Field::new("Email the PDF to (blank: the counterparty's email on file)", "to", FieldKind::Text);
    to.help = Some("Separate several addresses with commas.".into());
    Section::Form(FormView { get: false, action: format!("{base_path}/email"), fields: vec![to], submit: "Send".into(), error: None, cancel: None })
}

// ---------------------------------------------------------------------------
// P1 to P4
// ---------------------------------------------------------------------------

pub struct PaymentScreen {
    pub kind: &'static PaymentKind,
    pub title: &'static str,
    pub noun: &'static str,
    pub sales: bool,
    pub settler: &'static Settler,
    pub account_label: &'static str,
}

pub static PAYMENT_SCREENS: [PaymentScreen; 2] = [
    PaymentScreen {
        kind: &payments::CUSTOMER_PAYMENTS,
        title: "Customer payments",
        noun: "customer payment",
        sales: true,
        settler: &settlement::CUSTOMER_PAYMENT,
        account_label: "Deposit account",
    },
    PaymentScreen {
        kind: &payments::SUPPLIER_PAYMENTS,
        title: "Supplier payments",
        noun: "supplier payment",
        sales: false,
        settler: &settlement::SUPPLIER_PAYMENT,
        account_label: "Payment account",
    },
];

fn payment_spec(k: &PaymentKind) -> Vec<(&'static str, Kind)> {
    vec![
        (k.party, Kind::Str),
        ("payment_date", Kind::Str),
        ("currency_code", Kind::Str),
        ("amount", Kind::Str),
        ("method", Kind::Str),
        ("reference", Kind::Str),
        (k.account, Kind::Str),
    ]
}

async fn payment_list(State(st): State<AppState>, session: Session, s: &'static PaymentScreen) -> Response {
    let result: Result<Table> = async {
        let list = payments::list(&st.pool, s.kind).await?;
        let names = lookups::party_names(&st.pool, s.sales).await?;
        let base = lookups::base_currency(&st.pool).await?;
        let mut table = Table::new(&[if s.sales { "Customer" } else { "Supplier" }, "Date", "Method", "Amount#", "Applied#", "Unapplied#", "Status"]);
        table.rows = list
            .iter()
            .map(|p| {
                let currency = show(&p["currency_code"]);
                let name = names.get(&p[s.kind.party].as_i64().unwrap_or(0)).cloned().unwrap_or_default();
                vec![
                    Cell::link(if name.is_empty() { "(payment)".into() } else { name }, format!("/{}/{}", s.kind.path, p["id"])),
                    Cell::text(show(&p["payment_date"])),
                    Cell::text(show(&p["method"])),
                    Cell::number(amount(&currency, &base, &show(&p["amount"]))),
                    Cell::number(amount(&currency, &base, &show(&p["amount_applied"]))),
                    Cell::number(amount(&currency, &base, &show(&p["unapplied"]))),
                    Cell::text(show(&p["status"])),
                ]
            })
            .collect();
        Ok(table)
    }
    .await;
    match result {
        Ok(table) => page(&session, s.title, vec![Section::Actions(vec![Action::link(&format!("New {}", s.noun), format!("/{}/new", s.kind.path))], None), Section::Table(table)]),
        Err(err) => failure(&session, err),
    }
}

async fn payment_form(pool: &PgPool, session: &Session, s: &PaymentScreen, record: &Value, id: Option<i64>, error: Option<String>) -> Response {
    let fields: Result<Vec<Field>> = async {
        let methods = ["cash", "check", "card", "transfer", "other"].map(|m| (m.to_string(), m.to_string())).to_vec();
        let mut currency = required(select("Currency", "currency_code", record, "(choose)", lookups::currencies(pool).await?));
        if currency.value.is_empty() {
            currency.value = lookups::base_currency(pool).await?;
        }
        Ok(vec![
            required(select(if s.sales { "Customer" } else { "Supplier" }, s.kind.party, record, "(choose)", lookups::parties(pool, s.sales).await?)),
            required(field("Date", "payment_date", FieldKind::Date, record)),
            currency,
            required(field("Amount", "amount", FieldKind::Decimal, record)),
            select("Method", "method", record, "(none)", methods),
            field("Reference", "reference", FieldKind::Text, record),
            select(s.account_label, s.kind.account, record, "(choose)", lookups::accounts(pool, Accounts::Postable).await?),
        ])
    }
    .await;
    let fields = match fields {
        Ok(f) => f,
        Err(err) => return failure(session, err),
    };
    let (title, action, cancel) = match id {
        Some(id) => (format!("Edit {}", s.noun), format!("/{}/{id}", s.kind.path), format!("/{}/{id}", s.kind.path)),
        None => (format!("New {}", s.noun), format!("/{}", s.kind.path), format!("/{}", s.kind.path)),
    };
    page(session, &title, vec![Section::Form(FormView { get: false, action, fields, submit: "Save".into(), error, cancel: Some(cancel) })])
}

fn posted_payment(k: &PaymentKind, form: &FormData) -> Result<PaymentInput> {
    let v = Values::from_form(form, &[(k.party, Kind::Int), ("payment_date", Kind::Str), ("currency_code", Kind::Str), ("amount", Kind::Str), ("method", Kind::OptStr), ("reference", Kind::OptStr), (k.account, Kind::OptInt)])?;
    PaymentInput::from_json(k, v.json())
}

async fn payment_new(State(st): State<AppState>, session: Session, s: &'static PaymentScreen) -> Response {
    payment_form(&st.pool, &session, s, &json!({}), None, None).await
}

async fn payment_create(State(st): State<AppState>, session: Session, s: &'static PaymentScreen, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    let result = match posted_payment(s.kind, &form) {
        Ok(p) => payments::create(&st.pool, s.kind, p).await,
        Err(err) => Err(err),
    };
    match result {
        Ok(id) => see_other(&format!("/{}/{id}", s.kind.path)),
        Err(err) => payment_form(&st.pool, &session, s, &echo(&form, &payment_spec(s.kind)), None, Some(err.message)).await,
    }
}

async fn payment_edit(State(st): State<AppState>, session: Session, s: &'static PaymentScreen, Path(id): Path<i64>) -> Response {
    match payments::get(&st.pool, s.kind, id).await {
        Ok(mut p) => {
            p["amount"] = Value::String(plain(&show(&p["amount"])));
            payment_form(&st.pool, &session, s, &p, Some(id), None).await
        }
        Err(err) => failure(&session, err),
    }
}

async fn payment_update(State(st): State<AppState>, session: Session, s: &'static PaymentScreen, Path(id): Path<i64>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    let result = match posted_payment(s.kind, &form) {
        Ok(p) => payments::update(&st.pool, s.kind, id, p).await,
        Err(err) => Err(err),
    };
    match result {
        Ok(()) => see_other(&format!("/{}/{id}", s.kind.path)),
        Err(err) => payment_form(&st.pool, &session, s, &echo(&form, &payment_spec(s.kind)), Some(id), Some(err.message)).await,
    }
}

async fn payment_confirm_delete(State(st): State<AppState>, session: Session, s: &'static PaymentScreen, Path(id): Path<i64>) -> Response {
    match payments::get(&st.pool, s.kind, id).await {
        Ok(p) => confirm(&session, &format!("this {} of {}", s.noun, money(&show(&p["amount"]))), format!("/{}/{id}/delete", s.kind.path), format!("/{}/{id}", s.kind.path)),
        Err(err) => failure(&session, err),
    }
}

async fn payment_delete(State(st): State<AppState>, session: Session, s: &'static PaymentScreen, Path(id): Path<i64>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    match payments::delete(&st.pool, s.kind, id).await {
        Ok(()) => see_other(&format!("/{}", s.kind.path)),
        Err(err) => payment_detail(&st.pool, &session, s, id, Some(err.message), None).await,
    }
}

async fn payment_act(State(st): State<AppState>, session: Session, s: &'static PaymentScreen, Path((id, action)): Path<(i64, String)>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    let outcome: Result<Option<String>> = match action.as_str() {
        "post" => payments::post(&st.pool, s.kind, id).await.map(|_| None),
        "unpost" if session.user.is_admin => payments::unpost(&st.pool, s.kind, id).await.map(|_| None),
        "unpost" => Err(Error::forbidden("Only an administrator can unpost.")),
        "apply" => settlement::apply(&st.pool, s.settler, id).await.map(|made| Some(format!("Applied to {} document(s).", made.len()))),
        _ => return super::not_found(session).await,
    };
    match outcome {
        Ok(None) => see_other(&format!("/{}/{id}", s.kind.path)),
        Ok(Some(notice)) => payment_detail(&st.pool, &session, s, id, None, Some(notice)).await,
        Err(err) => payment_detail(&st.pool, &session, s, id, Some(err.message), None).await,
    }
}

async fn payment_detail_page(State(st): State<AppState>, session: Session, s: &'static PaymentScreen, Path(id): Path<i64>) -> Response {
    payment_detail(&st.pool, &session, s, id, None, None).await
}

/// P3, P4: the payment, its actions, and what it is applied to.
async fn payment_detail(pool: &PgPool, session: &Session, s: &'static PaymentScreen, id: i64, error: Option<String>, notice: Option<String>) -> Response {
    let result: Result<Vec<Section>> = async {
        let p = payments::get(pool, s.kind, id).await?;
        let names = lookups::party_names(pool, s.sales).await?;
        let accounts = lookups::account_names(pool).await?;
        let base = lookups::base_currency(pool).await?;
        let currency = show(&p["currency_code"]);
        let status = show(&p["status"]);
        let party_id = p[s.kind.party].as_i64().unwrap_or(0);
        let amt = |f: &str| amount(&currency, &base, &show(&p[f]));
        let mut facts = vec![
            (if s.sales { "Customer" } else { "Supplier" }.to_string(), Cell::text(names.get(&party_id).cloned().unwrap_or_default())),
            ("Date".to_string(), Cell::text(show(&p["payment_date"]))),
            ("Currency".to_string(), Cell::text(&currency)),
            ("Amount".to_string(), Cell::number(amt("amount"))),
            ("Applied".to_string(), Cell::number(amt("amount_applied"))),
            ("Unapplied".to_string(), Cell::number(amt("unapplied"))),
            ("Method".to_string(), Cell::text(show(&p["method"]))),
            ("Reference".to_string(), Cell::text(show(&p["reference"]))),
            (s.account_label.to_string(), Cell::text(p[s.kind.account].as_i64().and_then(|a| accounts.get(&a).cloned()).unwrap_or_default())),
            ("Status".to_string(), Cell::text(&status)),
        ];
        if let Some(entry) = p["journal_entry_id"].as_i64() {
            facts.push(("Journal entry".to_string(), Cell::link(format!("#{entry}"), format!("/journal-entries/{entry}"))));
        }
        let base_path = format!("/{}/{id}", s.kind.path);
        let mut actions = Vec::new();
        if status == "draft" {
            actions.push(Action::post("Post", format!("{base_path}/post")));
            actions.push(Action::link("Edit", format!("{base_path}/edit")));
            actions.push(Action::link("Delete", format!("{base_path}/delete")).danger());
        }
        if status == "posted" {
            if positive(&show(&p["unapplied"])) {
                actions.push(Action::post("Apply to open documents", format!("{base_path}/apply")));
            }
            if session.user.is_admin {
                actions.push(Action::post("Unpost", format!("{base_path}/unpost")).danger());
            }
        }
        let mut sections = Vec::new();
        if let Some(n) = notice {
            sections.push(Section::Text(n));
        }
        sections.push(Section::Facts(facts));
        sections.push(Section::Actions(actions, error));
        sections.push(applications_table(pool, s.settler, id, s.sales).await?);
        Ok(sections)
    }
    .await;
    match result {
        Ok(sections) => page(session, &capitalize(s.noun), sections),
        Err(err) => failure(session, err),
    }
}
