//! What the templates render. Handlers build these view models; the
//! templates only lay them out, so every screen shares a few templates.

use askama::Template;

/// A table cell or a fact's value: text, perhaps a link, perhaps a number
/// (right-aligned).
#[derive(Clone, Debug, Default)]
pub struct Cell {
    pub text: String,
    pub href: Option<String>,
    pub numeric: bool,
}

impl Cell {
    pub fn text(text: impl Into<String>) -> Cell {
        Cell { text: text.into(), ..Cell::default() }
    }

    pub fn link(text: impl Into<String>, href: impl Into<String>) -> Cell {
        Cell { text: text.into(), href: Some(href.into()), numeric: false }
    }

    pub fn number(text: impl Into<String>) -> Cell {
        Cell { text: text.into(), href: None, numeric: true }
    }
}

#[derive(Clone, Debug)]
pub struct Column {
    pub label: String,
    pub numeric: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Table {
    pub title: Option<String>,
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Cell>>,
    pub footer: Option<Vec<Cell>>,
    /// Shown instead of the table when there are no rows.
    pub empty: String,
}

impl Table {
    /// A table with these column labels; a label ending in `#` is numeric
    /// (the `#` is dropped).
    pub fn new(labels: &[&str]) -> Table {
        let columns = labels
            .iter()
            .map(|l| match l.strip_suffix('#') {
                Some(label) => Column { label: label.to_string(), numeric: true },
                None => Column { label: l.to_string(), numeric: false },
            })
            .collect();
        Table { columns, empty: "None yet.".to_string(), ..Table::default() }
    }

    pub fn titled(mut self, title: impl Into<String>) -> Table {
        self.title = Some(title.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum FieldKind {
    Text,
    Decimal,
    Integer,
    Date,
    Email,
    Password,
    TextArea,
    Checkbox,
    Select,
    Hidden,
}

#[derive(Clone, Debug)]
pub struct Field {
    pub label: String,
    pub name: String,
    pub kind: FieldKind,
    pub value: String,
    /// (value, label) pairs for a select; the first is usually a blank.
    pub options: Vec<(String, String)>,
    pub required: bool,
    pub readonly: bool,
    pub help: Option<String>,
}

impl Field {
    pub fn new(label: &str, name: &str, kind: FieldKind) -> Field {
        Field {
            label: label.to_string(),
            name: name.to_string(),
            kind,
            value: String::new(),
            options: Vec::new(),
            required: false,
            readonly: false,
            help: None,
        }
    }

    pub fn checked(&self) -> bool {
        self.value == "true"
    }
}

/// A form that posts to `action`.
#[derive(Clone, Debug, Default)]
pub struct Form {
    pub action: String,
    pub fields: Vec<Field>,
    pub submit: String,
    pub error: Option<String>,
    pub cancel: Option<String>,
}

/// A button that posts (with no fields but the token and `hidden`), or a
/// link.
#[derive(Clone, Debug, Default)]
pub struct Action {
    pub label: String,
    pub href: String,
    pub post: bool,
    pub danger: bool,
    pub hidden: Vec<(String, String)>,
}

impl Action {
    pub fn link(label: &str, href: impl Into<String>) -> Action {
        Action { label: label.to_string(), href: href.into(), ..Action::default() }
    }

    pub fn post(label: &str, href: impl Into<String>) -> Action {
        Action { label: label.to_string(), href: href.into(), post: true, ..Action::default() }
    }

    pub fn danger(mut self) -> Action {
        self.danger = true;
        self
    }
}

#[derive(Clone, Debug)]
pub enum Section {
    Heading(String),
    Text(String),
    Error(String),
    Facts(Vec<(String, Cell)>),
    Table(Table),
    Actions(Vec<Action>, Option<String>),
    Form(Form),
}

/// A navigation group in the sidebar.
pub struct NavGroup {
    pub title: &'static str,
    pub links: Vec<(&'static str, &'static str)>,
}

/// What every page shows around its content.
pub struct Chrome {
    pub user_name: String,
    pub is_admin: bool,
    pub token: String,
    pub nav: Vec<NavGroup>,
}

#[derive(Template)]
#[template(path = "page.html")]
pub struct PageTemplate<'a> {
    pub chrome: &'a Chrome,
    pub title: &'a str,
    pub sections: &'a [Section],
}

#[derive(Template)]
#[template(path = "login.html")]
pub struct LoginTemplate<'a> {
    pub next: &'a str,
    pub email: &'a str,
    pub error: Option<&'a str>,
}
