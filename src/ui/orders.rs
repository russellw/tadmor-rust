//! Orders (spec/domain.md §13.6, O1 to O7) and stock movements (§13.7,
//! S1 to S3).

use axum::Router;
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{get, post};
use serde_json::{Value, json};
use sqlx::PgPool;

use super::documents::{FormSetup, amount, capitalize, confirm, document_form, email, email_form, posted_document, posted_header, posted_lines, saved_lines};
use super::forms::{money, plain, show};
use super::lookups::{self, Accounts};
use super::master::{echo, field, required, select};
use super::{Action, Cell, Field, FieldKind, FormData, FormView, Section, Session, Table, failure, page, see_other};
use crate::documents::{self, Kind as DocKind};
use crate::error::{Error, Result};
use crate::http::AppState;
use crate::inventory::{self, MovementInput};
use crate::orders::{self, DocumentRequest, LineQuantity, Side, StockRequest};
use crate::printing::{self, Printable};

/// An order side as its screens show it.
pub struct OrderScreen {
    side: &'static Side,
    title: &'static str,
    noun: &'static str,
    sales: bool,
    printable: &'static Printable,
    /// The document it is invoiced or billed into.
    target: &'static DocKind,
    document_verb: &'static str,
    stock_verb: &'static str,
    due_label: &'static str,
}

static ORDER_SCREENS: [OrderScreen; 2] = [
    OrderScreen {
        side: &orders::SALES,
        title: "Sales orders",
        noun: "sales order",
        sales: true,
        printable: &printing::PRINTABLES[4],
        target: &documents::SALES_INVOICES,
        document_verb: "invoice",
        stock_verb: "ship",
        due_label: "Expected ship",
    },
    OrderScreen {
        side: &orders::PURCHASE,
        title: "Purchase orders",
        noun: "purchase order",
        sales: false,
        printable: &printing::PRINTABLES[5],
        target: &documents::PURCHASE_BILLS,
        document_verb: "bill",
        stock_verb: "receive",
        due_label: "Expected receipt",
    },
];

fn order_routes(s: &'static OrderScreen) -> Router<AppState> {
    let base = format!("/{}", s.side.kind.path);
    Router::new()
        .route(&base, get(move |st: State<AppState>, se: Session| list(st, se, s)).post(move |st: State<AppState>, se: Session, f: FormData| create(st, se, s, f)))
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
            &format!("{base}/{{id}}/{}", s.document_verb),
            get(move |st: State<AppState>, se: Session, id: Path<i64>| document_page(st, se, s, id))
                .post(move |st: State<AppState>, se: Session, id: Path<i64>, f: FormData| make_document(st, se, s, id, f)),
        )
        .route(
            &format!("{base}/{{id}}/{}", s.stock_verb),
            get(move |st: State<AppState>, se: Session, id: Path<i64>| stock_page(st, se, s, id))
                .post(move |st: State<AppState>, se: Session, id: Path<i64>, f: FormData| move_stock(st, se, s, id, f)),
        )
        .route(
            &format!("{base}/{{id}}/{{action}}"),
            post(move |st: State<AppState>, se: Session, path: Path<(i64, String)>, f: FormData| act(st, se, s, path, f)),
        )
}

pub fn routes() -> Router<AppState> {
    ORDER_SCREENS
        .iter()
        .fold(Router::new(), |r, s| r.merge(order_routes(s)))
        .route("/stock-movements", get(movement_list).post(movement_create))
        .route("/stock-movements/new", get(movement_new))
        .route("/stock-movements/{id}", get(movement_detail_page).post(movement_update))
        .route("/stock-movements/{id}/edit", get(movement_edit))
        .route("/stock-movements/{id}/delete", get(movement_confirm_delete).post(movement_delete))
        .route("/stock-movements/{id}/{action}", post(movement_act))
}

fn setup(s: &OrderScreen, title: String, action: String, cancel: String) -> FormSetup {
    FormSetup { kind: s.side.kind, sales: s.sales, due_label: s.due_label, title, action, cancel }
}

/// O1.
async fn list(State(st): State<AppState>, session: Session, s: &'static OrderScreen) -> Response {
    let result: Result<Table> = async {
        let list = orders::list(&st.pool, s.side).await?;
        let names = lookups::party_names(&st.pool, s.sales).await?;
        let base = lookups::base_currency(&st.pool).await?;
        let k = s.side.kind;
        let mut table = Table::new(&["Number", if s.sales { "Customer" } else { "Supplier" }, "Date", "Total#", "Status", if s.sales { "Invoiced" } else { "Billed" }, if s.sales { "Shipped" } else { "Received" }]);
        table.rows = list
            .iter()
            .map(|o| {
                vec![
                    Cell::link(show(&o["order_number"]), format!("/{}/{}", k.path, o["id"])),
                    Cell::text(names.get(&o[k.party].as_i64().unwrap_or(0)).cloned().unwrap_or_default()),
                    Cell::text(show(&o["order_date"])),
                    Cell::number(amount(&show(&o["currency_code"]), &base, &show(&o["total"]))),
                    Cell::text(show(&o["status"])),
                    Cell::text(show(&o[s.side.document_status])),
                    Cell::text(show(&o[s.side.stock_status])),
                ]
            })
            .collect();
        Ok(table)
    }
    .await;
    match result {
        Ok(table) => page(&session, s.title, vec![Section::Actions(vec![Action::link(&format!("New {}", s.noun), format!("/{}/new", s.side.kind.path))], None), Section::Table(table)]),
        Err(err) => failure(&session, err),
    }
}

/// O2: the line editor, for drafts.
async fn new(State(st): State<AppState>, session: Session, s: &'static OrderScreen) -> Response {
    let path = format!("/{}", s.side.kind.path);
    document_form(&st.pool, &session, &setup(s, format!("New {}", s.noun), path.clone(), path), &json!({}), vec![], None).await
}

async fn create(State(st): State<AppState>, session: Session, s: &'static OrderScreen, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    let k = s.side.kind;
    let result = match posted_document(k, &form) {
        Ok(doc) => documents::create(&st.pool, k, doc).await,
        Err(err) => Err(err),
    };
    match result {
        Ok(id) => see_other(&format!("/{}/{id}", k.path)),
        Err(err) => {
            let path = format!("/{}", k.path);
            document_form(&st.pool, &session, &setup(s, format!("New {}", s.noun), path.clone(), path), &posted_header(k, &form), posted_lines(&form), Some(err.message)).await
        }
    }
}

async fn edit(State(st): State<AppState>, session: Session, s: &'static OrderScreen, Path(id): Path<i64>) -> Response {
    let k = s.side.kind;
    let loaded = async {
        let order = orders::get(&st.pool, s.side, id).await?;
        let lines = orders::lines(&st.pool, s.side, id).await?;
        Ok::<_, Error>((order, lines))
    }
    .await;
    match loaded {
        Ok((order, lines)) => {
            let path = format!("/{}/{id}", k.path);
            let title = format!("Edit {} {}", s.noun, show(&order["order_number"]));
            document_form(&st.pool, &session, &setup(s, title, path.clone(), path), &order, saved_lines(k, &lines), None).await
        }
        Err(err) => failure(&session, err),
    }
}

async fn update(State(st): State<AppState>, session: Session, s: &'static OrderScreen, Path(id): Path<i64>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    let k = s.side.kind;
    let result = match posted_document(k, &form) {
        Ok(doc) => documents::update(&st.pool, k, id, doc).await,
        Err(err) => Err(err),
    };
    match result {
        Ok(()) => see_other(&format!("/{}/{id}", k.path)),
        Err(err) => {
            let path = format!("/{}/{id}", k.path);
            document_form(&st.pool, &session, &setup(s, format!("Edit {}", s.noun), path.clone(), path), &posted_header(k, &form), posted_lines(&form), Some(err.message)).await
        }
    }
}

async fn confirm_delete(State(st): State<AppState>, session: Session, s: &'static OrderScreen, Path(id): Path<i64>) -> Response {
    let k = s.side.kind;
    match orders::get(&st.pool, s.side, id).await {
        Ok(o) => confirm(&session, &format!("{} {}", s.noun, show(&o["order_number"])), format!("/{}/{id}/delete", k.path), format!("/{}/{id}", k.path)),
        Err(err) => failure(&session, err),
    }
}

async fn delete(State(st): State<AppState>, session: Session, s: &'static OrderScreen, Path(id): Path<i64>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    match documents::delete(&st.pool, s.side.kind, id).await {
        Ok(()) => see_other(&format!("/{}", s.side.kind.path)),
        Err(err) => detail(&st.pool, &session, s, id, Some(err.message), None).await,
    }
}

/// O4, O7: confirm, close, cancel, and email.
async fn act(State(st): State<AppState>, session: Session, s: &'static OrderScreen, Path((id, action)): Path<(i64, String)>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    let outcome: Result<Option<String>> = match action.as_str() {
        "confirm" => orders::confirm(&st.pool, s.side, id).await.map(|_| None),
        "close" => orders::close(&st.pool, s.side, id).await.map(|_| None),
        "cancel" => orders::cancel(&st.pool, s.side, id).await.map(|_| None),
        "email" => email(&st, s.printable, id, &form.text("to")).await.map(Some),
        _ => return super::not_found(session).await,
    };
    match outcome {
        Ok(None) => see_other(&format!("/{}/{id}", s.side.kind.path)),
        Ok(Some(notice)) => detail(&st.pool, &session, s, id, None, Some(notice)).await,
        Err(err) => detail(&st.pool, &session, s, id, Some(err.message), None).await,
    }
}

async fn detail_page(State(st): State<AppState>, session: Session, s: &'static OrderScreen, Path(id): Path<i64>) -> Response {
    detail(&st.pool, &session, s, id, None, None).await
}

/// O3: the header, statuses, and each line's quantities on both axes.
async fn detail(pool: &PgPool, session: &Session, s: &'static OrderScreen, id: i64, error: Option<String>, notice: Option<String>) -> Response {
    let result: Result<(String, Vec<Section>)> = async {
        let side = s.side;
        let k = side.kind;
        let o = orders::get(pool, side, id).await?;
        let lines = orders::lines(pool, side, id).await?;
        let names = lookups::party_names(pool, s.sales).await?;
        let base = lookups::base_currency(pool).await?;
        let currency = show(&o["currency_code"]);
        let status = show(&o["status"]);
        let party = o[k.party].as_i64().unwrap_or(0);
        let facts = vec![
            (if s.sales { "Customer" } else { "Supplier" }.to_string(), Cell::text(names.get(&party).cloned().unwrap_or_default())),
            ("Date".to_string(), Cell::text(show(&o["order_date"]))),
            (s.due_label.to_string(), Cell::text(show(&o[k.due.unwrap_or_default()]))),
            ("Currency".to_string(), Cell::text(&currency)),
            ("Status".to_string(), Cell::text(&status)),
            (if s.sales { "Invoiced" } else { "Billed" }.to_string(), Cell::text(show(&o[side.document_status]))),
            (if s.sales { "Shipped" } else { "Received" }.to_string(), Cell::text(show(&o[side.stock_status]))),
            ("Total".to_string(), Cell::number(amount(&currency, &base, &show(&o["total"])))),
            ("Reference".to_string(), Cell::text(show(&o["reference"]))),
            ("Memo".to_string(), Cell::text(show(&o["memo"]))),
        ];
        let (done, stock_done, todo, stock_todo) = if s.sales {
            ("Invoiced#", "Shipped#", "To invoice#", "To ship#")
        } else {
            ("Billed#", "Received#", "To bill#", "To receive#")
        };
        let mut table = Table::new(&["#", "Description", "Ordered#", if s.sales { "Unit price#" } else { "Unit cost#" }, "Total#", done, stock_done, todo, stock_todo]);
        table.rows = lines
            .iter()
            .map(|l| {
                vec![
                    Cell::text(show(&l["line_no"])),
                    Cell::text(show(&l["description"])),
                    Cell::number(plain(&show(&l["quantity"]))),
                    Cell::number(money(&show(&l[k.price]))),
                    Cell::number(money(&show(&l["line_total"]))),
                    Cell::number(plain(&show(&l[side.document_done]))),
                    Cell::number(plain(&show(&l[side.stock_done]))),
                    Cell::number(plain(&show(&l[side.document_todo]))),
                    Cell::number(plain(&show(&l[side.stock_todo]))),
                ]
            })
            .collect();

        // O4: the actions each state allows.
        let path = format!("/{}/{id}", k.path);
        let mut actions = Vec::new();
        match status.as_str() {
            "draft" => {
                actions.push(Action::post("Confirm", format!("{path}/confirm")));
                actions.push(Action::link("Edit", format!("{path}/edit")));
                actions.push(Action::link("Delete", format!("{path}/delete")).danger());
                actions.push(Action::post("Cancel order", format!("{path}/cancel")).danger());
            }
            "open" => {
                actions.push(Action::link(&capitalize(s.document_verb), format!("{path}/{}", s.document_verb)));
                actions.push(Action::link(&capitalize(s.stock_verb), format!("{path}/{}", s.stock_verb)));
                actions.push(Action::post("Close", format!("{path}/close")));
                actions.push(Action::post("Cancel order", format!("{path}/cancel")).danger());
            }
            _ => {}
        }
        actions.push(Action::link("PDF", format!("/api/{}/{id}/pdf", k.path)));
        let mut sections = Vec::new();
        if let Some(n) = notice {
            sections.push(Section::Text(n));
        }
        sections.extend([Section::Facts(facts), Section::Actions(actions, error), Section::Table(table), email_form(&path)]);
        Ok((format!("{} {}", capitalize(s.noun), show(&o["order_number"])), sections))
    }
    .await;
    match result {
        Ok((title, sections)) => page(session, &title, sections),
        Err(err) => failure(session, err),
    }
}

/// The outstanding lines on one axis, as quantity fields filled with what
/// remains (O5, O6); each after a hidden field naming its order line.
async fn quantity_fields(pool: &PgPool, s: &'static OrderScreen, id: i64, todo: &str) -> Result<Vec<Field>> {
    let lines = orders::lines(pool, s.side, id).await?;
    let mut fields = Vec::new();
    for l in lines.iter().filter(|l| super::documents::positive(&show(&l[todo]))) {
        let remaining = plain(&show(&l[todo]));
        fields.push(Field { value: show(&l["order_line_id"]), ..Field::new("", "line_order_line_id", FieldKind::Hidden) });
        fields.push(Field { value: remaining.clone(), ..Field::new(&format!("{} ({remaining} remaining)", show(&l["description"])), "line_quantity", FieldKind::Decimal) });
    }
    Ok(fields)
}

fn posted_quantities(form: &FormData) -> Vec<LineQuantity> {
    form.all("line_order_line_id")
        .into_iter()
        .zip(form.all("line_quantity"))
        .map(|(line, qty)| LineQuantity {
            order_line_id: line.parse().unwrap_or(0),
            quantity: if qty.trim().is_empty() { "0".into() } else { qty.trim().to_string() },
        })
        .collect()
}

/// O5: invoice or bill an open order.
async fn document_page(State(st): State<AppState>, session: Session, s: &'static OrderScreen, Path(id): Path<i64>) -> Response {
    document_form_page(&st.pool, &session, s, id, &json!({}), None).await
}

async fn document_form_page(pool: &PgPool, session: &Session, s: &'static OrderScreen, id: i64, header: &Value, error: Option<String>) -> Response {
    let result: Result<Vec<Field>> = async {
        let t = s.target;
        let mut fields = vec![
            required(field(&format!("{} number", capitalize(s.document_verb)), t.number, FieldKind::Text, header)),
            required(field("Date", t.date, FieldKind::Date, header)),
            field("Due date", "due_date", FieldKind::Date, header),
        ];
        let lines = quantity_fields(pool, s, id, s.side.document_todo).await?;
        if lines.is_empty() {
            return Err(Error::unprocessable(format!("Nothing is left to {} on this order.", s.document_verb)));
        }
        fields.extend(lines);
        Ok(fields)
    }
    .await;
    match result {
        Ok(fields) => {
            let path = format!("/{}/{id}", s.side.kind.path);
            let form = FormView { get: false, action: format!("{path}/{}", s.document_verb), fields, submit: format!("Create draft {}", s.document_verb), error, cancel: Some(path) };
            page(session, &format!("{} the order", capitalize(s.document_verb)), vec![Section::Text("Lower a quantity for a partial document; a line at 0 is left out.".into()), Section::Form(form)])
        }
        Err(err) if err.status == axum::http::StatusCode::UNPROCESSABLE_ENTITY => detail(pool, session, s, id, Some(err.message), None).await,
        Err(err) => failure(session, err),
    }
}

async fn make_document(State(st): State<AppState>, session: Session, s: &'static OrderScreen, Path(id): Path<i64>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    let t = s.target;
    let request = DocumentRequest { number: form.text(t.number), date: form.text(t.date), due_date: Some(form.text("due_date")).filter(|d| !d.is_empty()), lines: posted_quantities(&form) };
    match orders::fulfil_document(&st.pool, s.side, t, id, request).await {
        Ok(document) => see_other(&format!("/{}/{document}", t.path)),
        Err(err) => document_form_page(&st.pool, &session, s, id, &echo(&form, &[(t.number, super::forms::Kind::Str), (t.date, super::forms::Kind::Str), ("due_date", super::forms::Kind::Str)]), Some(err.message)).await,
    }
}

/// O6: ship or receive an open order's stocked lines.
async fn stock_page(State(st): State<AppState>, session: Session, s: &'static OrderScreen, Path(id): Path<i64>) -> Response {
    stock_form_page(&st.pool, &session, s, id, &json!({}), None).await
}

async fn stock_form_page(pool: &PgPool, session: &Session, s: &'static OrderScreen, id: i64, header: &Value, error: Option<String>) -> Response {
    let result: Result<Vec<Field>> = async {
        let mut fields = vec![
            required(select("Warehouse", "warehouse_id", header, "(choose)", lookups::warehouses(pool).await?)),
            field("Date (blank: today)", "movement_date", FieldKind::Date, header),
            field("Reference", "reference", FieldKind::Text, header),
        ];
        let lines = quantity_fields(pool, s, id, s.side.stock_todo).await?;
        if lines.is_empty() {
            return Err(Error::unprocessable(format!("No stocked line is left to {} on this order.", s.stock_verb)));
        }
        fields.extend(lines);
        Ok(fields)
    }
    .await;
    match result {
        Ok(fields) => {
            let path = format!("/{}/{id}", s.side.kind.path);
            let form = FormView { get: false, action: format!("{path}/{}", s.stock_verb), fields, submit: capitalize(s.stock_verb), error, cancel: Some(path) };
            page(session, &format!("{} the order", capitalize(s.stock_verb)), vec![Section::Text("Only lines of stocked products are offered. Lower a quantity for a partial movement.".into()), Section::Form(form)])
        }
        Err(err) if err.status == axum::http::StatusCode::UNPROCESSABLE_ENTITY => detail(pool, session, s, id, Some(err.message), None).await,
        Err(err) => failure(session, err),
    }
}

async fn move_stock(State(st): State<AppState>, session: Session, s: &'static OrderScreen, Path(id): Path<i64>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    let request = StockRequest {
        warehouse_id: form.text("warehouse_id").parse().unwrap_or(0),
        movement_date: Some(form.text("movement_date")).filter(|d| !d.is_empty()),
        reference: Some(form.text("reference")).filter(|r| !r.is_empty()),
        lines: posted_quantities(&form),
    };
    let result = if s.sales { orders::ship(&st.pool, id, request).await } else { orders::receive(&st.pool, id, request).await };
    match result {
        Ok(movements) => {
            let mut table = Table::new(&["Movement"]);
            table.rows = movements.iter().map(|m| vec![Cell::link(format!("Stock movement #{m}"), format!("/stock-movements/{m}"))]).collect();
            let back = Action::link("Back to the order", format!("/{}/{id}", s.side.kind.path));
            page(&session, "Draft movements created", vec![Section::Text("Post each movement to put it in the ledger.".into()), Section::Table(table), Section::Actions(vec![back], None)])
        }
        Err(err) => {
            let echo = echo(&form, &[("warehouse_id", super::forms::Kind::Str), ("movement_date", super::forms::Kind::Str), ("reference", super::forms::Kind::Str)]);
            stock_form_page(&st.pool, &session, s, id, &echo, Some(err.message)).await
        }
    }
}

// ---------------------------------------------------------------------------
// S1 to S3 stock movements
// ---------------------------------------------------------------------------

const TYPES: [&str; 5] = ["receipt", "issue", "adjustment", "transfer_in", "transfer_out"];

async fn movement_list(State(st): State<AppState>, session: Session) -> Response {
    let result: Result<Table> = async {
        let list = inventory::list(&st.pool).await?;
        let products = lookups::product_names(&st.pool).await?;
        let warehouses = lookups::warehouse_names(&st.pool).await?;
        let mut table = Table::new(&["Date", "Product", "Warehouse", "Type", "Quantity#", "Unit cost#", "Total cost#", "Posted"]);
        table.rows = list
            .iter()
            .map(|m| {
                vec![
                    Cell::link(&m.movement_date, format!("/stock-movements/{}", m.id)),
                    Cell::text(products.get(&i64::from(m.product_id)).cloned().unwrap_or_default()),
                    Cell::text(warehouses.get(&i64::from(m.warehouse_id)).cloned().unwrap_or_default()),
                    Cell::text(&m.movement_type),
                    Cell::number(plain(&m.quantity)),
                    Cell::number(money(&m.unit_cost)),
                    Cell::number(money(&m.total_cost)),
                    Cell::text(if m.journal_entry_id.is_some() { "posted" } else { "" }),
                ]
            })
            .collect();
        Ok(table)
    }
    .await;
    match result {
        Ok(table) => page(&session, "Stock movements", vec![Section::Actions(vec![Action::link("New movement", "/stock-movements/new")], None), Section::Table(table)]),
        Err(err) => failure(&session, err),
    }
}

/// S2: the quantity is a magnitude, signed by the type; an adjustment
/// keeps the sign typed.
async fn movement_form(pool: &PgPool, session: &Session, record: &Value, id: Option<i64>, error: Option<String>) -> Response {
    let fields: Result<Vec<Field>> = async {
        let products: lookups::Options = lookups::products(pool).await?.into_iter().map(|p| (p.id.to_string(), p.label)).collect();
        Ok(vec![
            required(select("Product (inventory-tracked)", "product_id", record, "(choose)", products)),
            required(select("Warehouse", "warehouse_id", record, "(choose)", lookups::warehouses(pool).await?)),
            required(select("Type", "movement_type", record, "(choose)", TYPES.map(|t| (t.to_string(), t.replace('_', " "))).to_vec())),
            field("Date (blank: today)", "movement_date", FieldKind::Date, record),
            Field { help: Some("A magnitude: receipts and transfers in add, issues and transfers out remove. An adjustment takes its sign as typed.".into()), ..required(field("Quantity", "quantity", FieldKind::Decimal, record)) },
            field("Unit cost", "unit_cost", FieldKind::Decimal, record),
            field("Reference", "reference", FieldKind::Text, record),
            field("Notes", "notes", FieldKind::TextArea, record),
        ])
    }
    .await;
    let fields = match fields {
        Ok(f) => f,
        Err(err) => return failure(session, err),
    };
    let (title, action, cancel) = match id {
        Some(id) => ("Edit stock movement".to_string(), format!("/stock-movements/{id}"), format!("/stock-movements/{id}")),
        None => ("New stock movement".to_string(), "/stock-movements".to_string(), "/stock-movements".to_string()),
    };
    page(session, &title, vec![Section::Form(FormView { get: false, action, fields, submit: "Save".into(), error, cancel: Some(cancel) })])
}

/// The movement a posted form describes, its quantity signed by the type.
fn posted_movement(form: &FormData) -> MovementInput {
    let kind = form.text("movement_type");
    let magnitude = form.text("quantity").trim().trim_start_matches(['+', '-']).to_string();
    let quantity = match kind.as_str() {
        "issue" | "transfer_out" if !magnitude.is_empty() => format!("-{magnitude}"),
        "receipt" | "transfer_in" => magnitude,
        _ => form.text("quantity").trim().to_string(),
    };
    MovementInput {
        product_id: form.text("product_id").parse().unwrap_or(0),
        warehouse_id: form.text("warehouse_id").parse().unwrap_or(0),
        movement_type: kind,
        movement_date: Some(form.text("movement_date")).filter(|d| !d.is_empty()),
        quantity,
        unit_cost: Some(form.text("unit_cost")).filter(|c| !c.trim().is_empty()),
        reference: Some(form.text("reference")).filter(|r| !r.is_empty()),
        notes: Some(form.text("notes")).filter(|n| !n.is_empty()),
    }
}

const MOVEMENT_FIELDS: [(&str, super::forms::Kind); 8] = [
    ("product_id", super::forms::Kind::Str),
    ("warehouse_id", super::forms::Kind::Str),
    ("movement_type", super::forms::Kind::Str),
    ("movement_date", super::forms::Kind::Str),
    ("quantity", super::forms::Kind::Str),
    ("unit_cost", super::forms::Kind::Str),
    ("reference", super::forms::Kind::Str),
    ("notes", super::forms::Kind::Str),
];

async fn movement_new(State(st): State<AppState>, session: Session) -> Response {
    movement_form(&st.pool, &session, &json!({}), None, None).await
}

async fn movement_create(State(st): State<AppState>, session: Session, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    match inventory::create(&st.pool, posted_movement(&form)).await {
        Ok(id) => see_other(&format!("/stock-movements/{id}")),
        Err(err) => movement_form(&st.pool, &session, &echo(&form, &MOVEMENT_FIELDS), None, Some(err.message)).await,
    }
}

async fn movement_edit(State(st): State<AppState>, session: Session, Path(id): Path<i64>) -> Response {
    match inventory::get(&st.pool, id).await {
        Ok(m) => {
            let mut record = serde_json::to_value(&m).unwrap_or_default();
            let quantity = plain(&m.quantity);
            record["quantity"] = Value::String(if m.movement_type == "adjustment" { quantity } else { quantity.trim_start_matches('-').to_string() });
            record["unit_cost"] = Value::String(plain(&m.unit_cost));
            movement_form(&st.pool, &session, &record, Some(id), None).await
        }
        Err(err) => failure(&session, err),
    }
}

async fn movement_update(State(st): State<AppState>, session: Session, Path(id): Path<i64>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    match inventory::update(&st.pool, id, posted_movement(&form)).await {
        Ok(()) => see_other(&format!("/stock-movements/{id}")),
        Err(err) => movement_form(&st.pool, &session, &echo(&form, &MOVEMENT_FIELDS), Some(id), Some(err.message)).await,
    }
}

async fn movement_confirm_delete(State(st): State<AppState>, session: Session, Path(id): Path<i64>) -> Response {
    match inventory::get(&st.pool, id).await {
        Ok(m) => confirm(&session, &format!("this {} of {}", m.movement_type, plain(&m.quantity)), format!("/stock-movements/{id}/delete"), format!("/stock-movements/{id}")),
        Err(err) => failure(&session, err),
    }
}

async fn movement_delete(State(st): State<AppState>, session: Session, Path(id): Path<i64>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    match inventory::delete(&st.pool, id).await {
        Ok(()) => see_other("/stock-movements"),
        Err(err) => movement_detail(&st.pool, &session, id, Some(err.message)).await,
    }
}

async fn movement_act(State(st): State<AppState>, session: Session, Path((id, action)): Path<(i64, String)>, form: FormData) -> Response {
    if let Err(response) = form.check(&session) {
        return response;
    }
    let outcome = match action.as_str() {
        "post" => inventory::post(&st.pool, id, form.text("credit_account_id").parse().ok()).await.map(|_| ()),
        "unpost" if session.user.is_admin => inventory::unpost(&st.pool, id).await.map(|_| ()),
        "unpost" => Err(Error::forbidden("Only an administrator can unpost.")),
        _ => return super::not_found(session).await,
    };
    match outcome {
        Ok(()) => see_other(&format!("/stock-movements/{id}")),
        Err(err) => movement_detail(&st.pool, &session, id, Some(err.message)).await,
    }
}

async fn movement_detail_page(State(st): State<AppState>, session: Session, Path(id): Path<i64>) -> Response {
    movement_detail(&st.pool, &session, id, None).await
}

/// S3: the movement, a link to its entry, and its actions; posting a
/// receipt asks for the account to credit.
async fn movement_detail(pool: &PgPool, session: &Session, id: i64, error: Option<String>) -> Response {
    let result: Result<Vec<Section>> = async {
        let m = inventory::get(pool, id).await?;
        let products = lookups::product_names(pool).await?;
        let warehouses = lookups::warehouse_names(pool).await?;
        let mut facts = vec![
            ("Product".to_string(), Cell::text(products.get(&i64::from(m.product_id)).cloned().unwrap_or_default())),
            ("Warehouse".to_string(), Cell::text(warehouses.get(&i64::from(m.warehouse_id)).cloned().unwrap_or_default())),
            ("Date".to_string(), Cell::text(&m.movement_date)),
            ("Type".to_string(), Cell::text(m.movement_type.replace('_', " "))),
            ("Quantity".to_string(), Cell::number(plain(&m.quantity))),
            ("Unit cost".to_string(), Cell::number(money(&m.unit_cost))),
            ("Total cost".to_string(), Cell::number(money(&m.total_cost))),
            ("Status".to_string(), Cell::text(&m.status)),
            ("Source".to_string(), Cell::text(m.source_type.clone().unwrap_or_else(|| "entered by hand".into()).replace('_', " "))),
            ("Reference".to_string(), Cell::text(m.reference.clone().unwrap_or_default())),
            ("Notes".to_string(), Cell::text(m.notes.clone().unwrap_or_default())),
        ];
        if let Some(entry) = m.journal_entry_id {
            facts.push(("Journal entry".to_string(), Cell::link(format!("#{entry}"), format!("/journal-entries/{entry}"))));
        }
        let path = format!("/stock-movements/{id}");
        let mut actions = Vec::new();
        let mut sections = vec![Section::Facts(facts)];
        let mut post_form = None;
        if m.journal_entry_id.is_none() {
            match m.movement_type.as_str() {
                "issue" => actions.push(Action::post("Post", format!("{path}/post"))),
                "receipt" => {
                    let accounts = lookups::accounts(pool, Accounts::Postable).await?;
                    let grni = sqlx::query_scalar!("SELECT id FROM accounts WHERE code = '2150' AND is_active AND is_postable").fetch_optional(pool).await?;
                    let mut credit = select("Account to credit (usually Goods Received Not Invoiced)", "credit_account_id", &json!({"credit_account_id": grni.map(|g| g.to_string())}), "(choose)", accounts);
                    credit.required = true;
                    post_form = Some(Section::Form(FormView { get: false, action: format!("{path}/post"), fields: vec![credit], submit: "Post receipt".into(), error: None, cancel: None }));
                }
                _ => sections.push(Section::Text("Only receipts and issues post to the ledger.".into())),
            }
            if m.source_type.is_none() {
                actions.push(Action::link("Edit", format!("{path}/edit")));
            }
            actions.push(Action::link("Delete", format!("{path}/delete")).danger());
        } else if session.user.is_admin {
            actions.push(Action::post("Unpost", format!("{path}/unpost")).danger());
        }
        sections.push(Section::Actions(actions, error));
        sections.extend(post_form);
        Ok(sections)
    }
    .await;
    match result {
        Ok(sections) => page(session, &format!("Stock movement #{id}"), sections),
        Err(err) => failure(session, err),
    }
}
