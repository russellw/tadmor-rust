//! Printed documents (spec/api.md §5.11, domain §11): invoices, bills, both
//! kinds of credit note, and both kinds of order, rendered as one shared A4
//! layout in Helvetica.

use sqlx::PgPool;

use crate::documents::{self, Kind};
use crate::error::{Error, Result};
use crate::orders;
use crate::pdf::{Doc, Font, Page};

/// What distinguishes one printable kind from another.
pub struct Printable {
    pub kind: &'static Kind,
    /// The title, as in "Invoice INV-1".
    pub title: &'static str,
    /// The email subject's label.
    pub label: &'static str,
    /// The download filename's prefix.
    pub prefix: &'static str,
    number_label: &'static str,
    date_label: &'static str,
    due_label: &'static str,
    unit_label: &'static str,
    party_label: &'static str,
    /// The labels of the applied amount and balance, for documents that settle.
    settles: Option<(&'static str, &'static str)>,
    /// Whether the document may name its own billing address.
    billing_address: bool,
}

pub static PRINTABLES: [Printable; 6] = [
    Printable {
        kind: &documents::SALES_INVOICES,
        title: "Invoice",
        label: "Invoice",
        prefix: "invoice",
        number_label: "Invoice no.",
        date_label: "Invoice date",
        due_label: "Due date",
        unit_label: "UNIT PRICE",
        party_label: "BILL TO",
        settles: Some(("Amount paid", "Balance due")),
        billing_address: true,
    },
    Printable {
        kind: &documents::PURCHASE_BILLS,
        title: "Bill",
        label: "Bill",
        prefix: "bill",
        number_label: "Bill no.",
        date_label: "Bill date",
        due_label: "Due date",
        unit_label: "UNIT COST",
        party_label: "SUPPLIER",
        settles: Some(("Amount paid", "Balance due")),
        billing_address: false,
    },
    Printable {
        kind: &documents::SALES_CREDIT_NOTES,
        title: "Credit Note",
        label: "Credit Note",
        prefix: "credit-note",
        number_label: "Credit note no.",
        date_label: "Credit note date",
        due_label: "",
        unit_label: "UNIT PRICE",
        party_label: "CREDIT TO",
        settles: Some(("Amount applied", "Unapplied")),
        billing_address: false,
    },
    Printable {
        kind: &documents::PURCHASE_CREDIT_NOTES,
        title: "Supplier Credit",
        label: "Credit Note",
        prefix: "supplier-credit",
        number_label: "Credit note no.",
        date_label: "Credit note date",
        due_label: "",
        unit_label: "UNIT COST",
        party_label: "SUPPLIER",
        settles: Some(("Amount applied", "Unapplied")),
        billing_address: false,
    },
    Printable {
        kind: &orders::SALES_ORDERS,
        title: "Sales Order",
        label: "Sales Order",
        prefix: "sales-order",
        number_label: "Order no.",
        date_label: "Order date",
        due_label: "Expected ship",
        unit_label: "UNIT PRICE",
        party_label: "CUSTOMER",
        settles: None,
        billing_address: false,
    },
    Printable {
        kind: &orders::PURCHASE_ORDERS,
        title: "Purchase Order",
        label: "Purchase Order",
        prefix: "purchase-order",
        number_label: "Order no.",
        date_label: "Order date",
        due_label: "Expected receipt",
        unit_label: "UNIT COST",
        party_label: "SUPPLIER",
        settles: None,
        billing_address: false,
    },
];

/// A rendered document.
pub struct Printed {
    pub number: String,
    pub pdf: Vec<u8>,
}

impl Printed {
    /// `<prefix>-<number>.pdf`, every character of the number outside
    /// `[A-Za-z0-9._-]` replaced by `-`.
    pub fn filename(&self, p: &Printable) -> String {
        let number: String =
            self.number.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' }).collect();
        format!("{}-{number}.pdf", p.prefix)
    }
}

struct Party {
    name: String,
    legal: Option<String>,
    tax_id: Option<String>,
    address: Vec<String>,
}

struct Line {
    no: i32,
    description: String,
    quantity: String,
    unit: String,
    tax_rate: String,
    subtotal: String,
}

struct Data {
    number: String,
    status: String,
    currency: String,
    meta: Vec<(&'static str, String)>,
    party: Party,
    seller: Option<Party>,
    subtotal: String,
    tax_total: String,
    total: String,
    applied: Option<String>,
    balance: Option<String>,
    reference: Option<String>,
    memo: Option<String>,
    lines: Vec<Line>,
}

type AddressRow = (Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>);

fn address_lines((line1, line2, city, region, postal, country): AddressRow) -> Vec<String> {
    let present = |s: &Option<String>| s.clone().filter(|s| !s.is_empty());
    let mut out: Vec<String> = [&line1, &line2].into_iter().filter_map(present).collect();
    let mut city_line = present(&city).unwrap_or_default();
    if let Some(r) = present(&region) {
        city_line = if city_line.is_empty() { r } else { format!("{city_line}, {r}") };
    }
    if let Some(p) = present(&postal) {
        city_line = if city_line.is_empty() { p } else { format!("{city_line} {p}") };
    }
    if !city_line.is_empty() {
        out.push(city_line);
    }
    out.extend(present(&country));
    out
}

/// The address SELECT list, with the country's name.
const ADDRESS: &str = "a.line1, a.line2, a.city, a.region, a.postal_code, co.name";

async fn fetch(pool: &PgPool, p: &Printable, id: i64) -> Result<Data> {
    let k = p.kind;
    let due = k.due.map(|c| format!("d.{c}::text")).unwrap_or_else(|| "NULL::text".to_string());
    let (amounts, join) = match p.settles {
        Some(_) => (
            "b.amount_applied::numeric(19,4)::text, b.balance::numeric(19,4)::text",
            format!("JOIN {} b ON b.{} = d.id", k.balances_view, k.view_id),
        ),
        None => ("NULL::text, NULL::text", String::new()),
    };
    let sql = format!(
        "SELECT d.{number}, d.{date}::text, {due}, d.currency_code::text, d.status,
                d.subtotal::text, d.tax_total::text, d.total::text, {amounts}, d.reference, d.memo,
                o.name, o.legal_name, o.tax_id, o.id
         FROM {table} d
         JOIN {parties} p ON p.id = d.{party}
         JOIN organizations o ON o.id = p.organization_id
         {join}
         WHERE d.id = $1::int8",
        number = k.number,
        date = k.date,
        table = k.table,
        parties = k.party_table,
        party = k.party,
    );
    #[allow(clippy::type_complexity)]
    let row: Option<(
        String, String, Option<String>, String, String, String, String, String,
        Option<String>, Option<String>, Option<String>, Option<String>,
        String, Option<String>, Option<String>, i32,
    )> = sqlx::query_as(&sql).bind(id).fetch_optional(pool).await?;
    let (number, date, due, currency, status, subtotal, tax_total, total, applied, balance, reference, memo, name, legal, tax_id, org) =
        row.ok_or_else(Error::not_found)?;

    // An invoice may name its billing address; otherwise the organization's first.
    let address_sql = if p.billing_address {
        format!(
            "SELECT {ADDRESS} FROM addresses a LEFT JOIN countries co ON co.code = a.country_code
             WHERE a.id = COALESCE((SELECT billing_address_id FROM sales_invoices WHERE id = $2::int8),
                                   (SELECT id FROM addresses WHERE organization_id = $1 ORDER BY id LIMIT 1))"
        )
    } else {
        format!(
            "SELECT {ADDRESS} FROM addresses a LEFT JOIN countries co ON co.code = a.country_code
             WHERE a.organization_id = $1 ORDER BY a.id LIMIT 1"
        )
    };
    let mut query = sqlx::query_as::<_, AddressRow>(&address_sql).bind(org);
    if p.billing_address {
        query = query.bind(id);
    }
    let address = query.fetch_optional(pool).await?;

    let seller_sql = format!(
        "SELECT o.name, o.legal_name, o.tax_id, {ADDRESS}
         FROM organizations o
         LEFT JOIN LATERAL (SELECT * FROM addresses WHERE organization_id = o.id ORDER BY id LIMIT 1) a ON true
         LEFT JOIN countries co ON co.code = a.country_code
         WHERE o.is_self"
    );
    #[allow(clippy::type_complexity)]
    let seller: Option<(String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>)> =
        sqlx::query_as(&seller_sql).fetch_optional(pool).await?;
    let seller = seller.map(|(name, legal, tax_id, l1, l2, city, region, postal, country)| Party {
        name,
        legal,
        tax_id,
        address: address_lines((l1, l2, city, region, postal, country)),
    });

    let sql = format!(
        "SELECT line_no, description, quantity::text, {price}::text, tax_rate::text, line_subtotal::text
         FROM {lines} WHERE {fk} = $1::int8 ORDER BY line_no",
        price = k.price,
        lines = k.lines_table,
        fk = k.line_fk,
    );
    let lines = sqlx::query_as::<_, (i32, String, String, String, String, String)>(&sql)
        .bind(id)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(no, description, quantity, unit, tax_rate, subtotal)| Line { no, description, quantity, unit, tax_rate, subtotal })
        .collect();

    let mut meta = vec![(p.number_label, number.clone()), (p.date_label, date)];
    if let Some(due) = due {
        meta.push((p.due_label, due));
    }
    meta.push(("Currency", currency.clone()));
    Ok(Data {
        number,
        status,
        currency,
        meta,
        party: Party { name, legal, tax_id, address: address.map(address_lines).unwrap_or_default() },
        seller,
        subtotal,
        tax_total,
        total,
        applied,
        balance,
        reference,
        memo,
        lines,
    })
}

/// Renders the document; 404 for an unknown one.
pub async fn render(pool: &PgPool, p: &Printable, id: i64) -> Result<Printed> {
    let data = fetch(pool, p, id).await?;
    Ok(Printed { number: data.number.clone(), pdf: layout(p, &data) })
}

/// The counterparty organization's email, the default recipient; 404 for
/// an unknown document.
pub async fn recipient(pool: &PgPool, p: &Printable, id: i64) -> Result<Option<String>> {
    let k = p.kind;
    let sql = format!(
        "SELECT o.email FROM {table} d JOIN {parties} p ON p.id = d.{party} JOIN organizations o ON o.id = p.organization_id
         WHERE d.id = $1::int8",
        table = k.table,
        parties = k.party_table,
        party = k.party,
    );
    sqlx::query_scalar::<_, Option<String>>(&sql).bind(id).fetch_optional(pool).await?.ok_or_else(Error::not_found)
}

// The A4 layout, in points from the bottom left.
const PAGE_W: f64 = 595.28;
const PAGE_H: f64 = 841.89;
const MARGIN_L: f64 = 54.0;
const RIGHT_X: f64 = PAGE_W - 54.0;
const TOP_Y: f64 = PAGE_H - 54.0;
const BOTTOM_Y: f64 = 72.0;
const NUM_X: f64 = MARGIN_L;
const DESC_X: f64 = MARGIN_L + 26.0;
const QTY_X: f64 = 360.0;
const PRICE_X: f64 = 432.0;
const TAX_X: f64 = 472.0;
const DESC_MAX: f64 = QTY_X - 60.0 - DESC_X;
const GRAY: f64 = 0.45;

fn title(p: &Printable, status: &str) -> String {
    let t = p.title.to_uppercase();
    match status {
        "draft" | "void" | "cancelled" => format!("{} {t}", status.to_uppercase()),
        _ => t,
    }
}

fn layout(p: &Printable, d: &Data) -> Vec<u8> {
    let mut doc = Doc::default();
    let mut page = doc.add_page(PAGE_W, PAGE_H);

    let title = title(p, &d.status);
    page.text(Font::HelveticaBold, 20.0, RIGHT_X - Font::HelveticaBold.width(20.0, &title), TOP_Y - 6.0, &title);
    let mut meta_y = TOP_Y - 36.0;
    for (label, value) in &d.meta {
        page.text_gray(Font::Helvetica, 9.0, RIGHT_X - 140.0, meta_y, GRAY, label);
        page.text(Font::Helvetica, 9.0, RIGHT_X - Font::Helvetica.width(9.0, value), meta_y, value);
        meta_y -= 13.0;
    }

    let mut y = TOP_Y - 6.0;
    if let Some(seller) = &d.seller {
        y = party_block(page, MARGIN_L, y, 11.0, seller);
    }
    y = y.min(meta_y) - 28.0;
    page.text_gray(Font::HelveticaBold, 8.0, MARGIN_L, y, GRAY, p.party_label);
    y = party_block(page, MARGIN_L, y - 14.0, 10.0, &d.party) - 24.0;
    y = table_header(page, y, p.unit_label);

    for line in &d.lines {
        if y < BOTTOM_Y + 20.0 {
            page = doc.add_page(PAGE_W, PAGE_H);
            y = table_header(page, TOP_Y, p.unit_label);
        }
        page.text_gray(Font::Helvetica, 9.0, NUM_X, y, GRAY, &line.no.to_string());
        page.text(Font::Helvetica, 9.0, DESC_X, y, &truncate(9.0, DESC_MAX, &line.description));
        for (x, s) in [
            (QTY_X, quantity(&line.quantity)),
            (PRICE_X, amount(&line.unit)),
            (TAX_X, quantity(&line.tax_rate)),
            (RIGHT_X, amount(&line.subtotal)),
        ] {
            page.text(Font::Helvetica, 9.0, x - Font::Helvetica.width(9.0, &s), y, &s);
        }
        y -= 6.0;
        page.line(MARGIN_L, y, RIGHT_X, y, 0.4, 0.9);
        y -= 12.0;
    }

    if y < BOTTOM_Y + 110.0 {
        page = doc.add_page(PAGE_W, PAGE_H);
        y = TOP_Y;
    }
    y -= 8.0;
    let totals_x = 400.0;
    let total = |page: &mut Page, y: &mut f64, label: &str, value: String, font: Font| {
        page.text(font, 9.0, totals_x, *y, label);
        page.text(font, 9.0, RIGHT_X - font.width(9.0, &value), *y, &value);
        *y -= 14.0;
    };
    total(page, &mut y, "Subtotal", amount(&d.subtotal), Font::Helvetica);
    total(page, &mut y, "Tax", amount(&d.tax_total), Font::Helvetica);
    page.line(totals_x, y + 9.0, RIGHT_X, y + 9.0, 0.8, 0.2);
    y -= 2.0;
    total(page, &mut y, "Total", format!("{} {}", d.currency, amount(&d.total)), Font::HelveticaBold);
    if let (Some((applied_label, balance_label)), Some(applied), Some(balance)) = (p.settles, &d.applied, &d.balance) {
        if amount(applied) != "0.00" {
            total(page, &mut y, applied_label, amount(applied), Font::Helvetica);
            total(page, &mut y, balance_label, format!("{} {}", d.currency, amount(balance)), Font::HelveticaBold);
        }
    }

    let mut note_y = y - 14.0;
    if let Some(reference) = d.reference.as_deref().filter(|r| !r.is_empty()) {
        page.text_gray(Font::Helvetica, 9.0, MARGIN_L, note_y, GRAY, &format!("Reference: {reference}"));
        note_y -= 13.0;
    }
    if let Some(memo) = d.memo.as_deref().filter(|m| !m.is_empty()) {
        for line in wrap(9.0, RIGHT_X - MARGIN_L, memo) {
            page.text_gray(Font::Helvetica, 9.0, MARGIN_L, note_y, GRAY, &line);
            note_y -= 13.0;
        }
    }

    let count = doc.pages_mut().len();
    for (i, page) in doc.pages_mut().iter_mut().enumerate() {
        let footer = format!("{} {}  ·  Page {} of {count}", p.title, d.number, i + 1);
        page.text_gray(Font::Helvetica, 8.0, (PAGE_W - Font::Helvetica.width(8.0, &footer)) / 2.0, 40.0, GRAY, &footer);
    }
    doc.bytes()
}

fn party_block(page: &mut Page, x: f64, mut y: f64, name_size: f64, party: &Party) -> f64 {
    page.text(Font::HelveticaBold, name_size, x, y, &party.name);
    y -= 13.0;
    if let Some(legal) = party.legal.as_deref().filter(|l| !l.is_empty() && *l != party.name) {
        page.text_gray(Font::Helvetica, 9.0, x, y, GRAY, legal);
        y -= 12.0;
    }
    for line in &party.address {
        page.text_gray(Font::Helvetica, 9.0, x, y, GRAY, line);
        y -= 12.0;
    }
    if let Some(tax_id) = party.tax_id.as_deref().filter(|t| !t.is_empty()) {
        page.text_gray(Font::Helvetica, 9.0, x, y, GRAY, &format!("Tax ID: {tax_id}"));
        y -= 12.0;
    }
    y
}

fn table_header(page: &mut Page, y: f64, unit_label: &str) -> f64 {
    for (x, label, right) in [
        (NUM_X, "#", false),
        (DESC_X, "DESCRIPTION", false),
        (QTY_X, "QTY", true),
        (PRICE_X, unit_label, true),
        (TAX_X, "TAX %", true),
        (RIGHT_X, "AMOUNT", true),
    ] {
        let x = if right { x - Font::HelveticaBold.width(8.0, label) } else { x };
        page.text_gray(Font::HelveticaBold, 8.0, x, y, GRAY, label);
    }
    page.line(MARGIN_L, y - 6.0, RIGHT_X, y - 6.0, 0.8, 0.2);
    y - 20.0
}

/// `s` cut with an ellipsis to fit `max` points.
fn truncate(size: f64, max: f64, s: &str) -> String {
    if Font::Helvetica.width(size, s) <= max {
        return s.to_string();
    }
    let mut chars: Vec<char> = s.chars().collect();
    while !chars.is_empty() && Font::Helvetica.width(size, &format!("{}…", chars.iter().collect::<String>())) > max {
        chars.pop();
    }
    format!("{}…", chars.into_iter().collect::<String>())
}

/// `s` broken into lines of at most `max` points, at spaces.
fn wrap(size: f64, max: f64, s: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in s.split_whitespace() {
        let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if Font::Helvetica.width(size, &candidate) > max && !line.is_empty() {
            lines.push(std::mem::replace(&mut line, word.to_string()));
        } else {
            line = candidate;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// A decimal as money: thousands grouped, at least two places.
fn amount(s: &str) -> String {
    let (sign, s) = s.strip_prefix('-').map_or(("", s), |rest| ("-", rest));
    let (whole, fraction) = s.split_once('.').unwrap_or((s, ""));
    let mut fraction = fraction.trim_end_matches('0').to_string();
    while fraction.len() < 2 {
        fraction.push('0');
    }
    let mut grouped = String::new();
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{sign}{grouped}.{fraction}")
}

/// A decimal with trailing zeros dropped.
fn quantity(s: &str) -> String {
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_formats() {
        assert_eq!(amount("1234567.5000"), "1,234,567.50");
        assert_eq!(amount("-12.3456"), "-12.3456");
        assert_eq!(amount("0.0000"), "0.00");
        assert_eq!(quantity("2.5000"), "2.5");
        assert_eq!(quantity("3.0000"), "3");
    }

    #[test]
    fn filenames_replace_unsafe_characters() {
        let printed = Printed { number: "INV/2026 01".into(), pdf: vec![] };
        assert_eq!(printed.filename(&PRINTABLES[0]), "invoice-INV-2026-01.pdf");
    }

    #[test]
    fn long_text_fits() {
        assert!(Font::Helvetica.width(9.0, &truncate(9.0, 50.0, "a very long description indeed")) <= 50.0);
        let text = "one two three four five six seven eight";
        let lines = wrap(9.0, 60.0, text);
        assert!(lines.len() > 1 && lines.iter().all(|l| Font::Helvetica.width(9.0, l) <= 60.0), "{lines:?}");
        assert_eq!(lines.join(" "), text);
    }
}
