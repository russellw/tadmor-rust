//! Posted forms: reading them, checking their token, and turning their
//! values into the JSON the services' input types deserialize from. Also
//! the number formats every screen uses.

use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use super::Session;
use crate::error::Error;

/// A posted `application/x-www-form-urlencoded` body, in order, with
/// repeated names kept (the line editor posts one of each per line).
pub struct FormData(pub Vec<(String, String)>);

impl<S: Send + Sync> FromRequest<S> for FormData {
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Response> {
        let axum::Form(pairs) = axum::Form::<Vec<(String, String)>>::from_request(req, state).await.map_err(IntoResponse::into_response)?;
        Ok(FormData(pairs))
    }
}

impl FormData {
    /// Refuses a post without the session's form token.
    pub fn check(&self, session: &Session) -> Result<(), Response> {
        if self.get("_token") == Some(session.token.as_str()) {
            Ok(())
        } else {
            Err((StatusCode::FORBIDDEN, "The form has expired or did not come from this site; reload it and try again.").into_response())
        }
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// The value, or empty when absent.
    pub fn text(&self, name: &str) -> String {
        self.get(name).unwrap_or_default().to_string()
    }

    /// Every value posted under `name`, in order.
    pub fn all(&self, name: &str) -> Vec<String> {
        self.0.iter().filter(|(k, _)| k == name).map(|(_, v)| v.clone()).collect()
    }

    /// A checkbox: present means true.
    pub fn flag(&self, name: &str) -> bool {
        self.get(name).is_some()
    }
}

/// How a form value becomes JSON for a service input.
#[derive(Clone, Copy)]
pub enum Kind {
    /// A string, empty if blank.
    Str,
    /// A string, or null if blank.
    OptStr,
    /// A checkbox.
    Bool,
    /// An integer, 0 if blank.
    Int,
    /// An integer, or null if blank.
    OptInt,
}

/// Builds the JSON object a service input deserializes from.
pub struct Values(Map<String, Value>);

impl Values {
    pub fn from_form(form: &FormData, fields: &[(&str, Kind)]) -> Result<Values, Error> {
        let mut m = Map::new();
        for (name, kind) in fields {
            let raw = form.get(name).map(str::trim).unwrap_or_default();
            let value = match kind {
                Kind::Str => Value::String(form.text(name)),
                Kind::OptStr if raw.is_empty() => Value::Null,
                Kind::OptStr => Value::String(form.text(name)),
                Kind::Bool => Value::Bool(form.flag(name)),
                Kind::Int | Kind::OptInt if raw.is_empty() => {
                    if matches!(kind, Kind::Int) { Value::from(0) } else { Value::Null }
                }
                Kind::Int | Kind::OptInt => {
                    Value::from(raw.parse::<i64>().map_err(|_| Error::bad_request(format!("{name} must be a whole number")))?)
                }
            };
            m.insert(name.to_string(), value);
        }
        Ok(Values(m))
    }

    pub fn set(&mut self, name: &str, value: Value) {
        self.0.insert(name.to_string(), value);
    }

    pub fn json(self) -> Value {
        Value::Object(self.0)
    }

    pub fn into<T: DeserializeOwned>(self) -> Result<T, Error> {
        serde_json::from_value(Value::Object(self.0)).map_err(|e| Error::bad_request(e.to_string()))
    }
}

/// Percent-encodes `s` for a query string.
pub fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A decimal as money (G7): exact, thousands grouped, at least two places.
pub fn money(s: &str) -> String {
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

/// A decimal with trailing zeros dropped: quantities, rates.
pub fn plain(s: &str) -> String {
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s.to_string() }
}

/// A JSON value as display text: strings as they are, null as empty.
pub fn show(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(true) => "yes".to_string(),
        Value::Bool(false) => "no".to_string(),
        other => other.to_string(),
    }
}

/// A JSON field as the text a form field holds.
pub fn field_value(v: &Value) -> String {
    match v {
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => String::new(),
        other => show(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(money("1234567.5000"), "1,234,567.50");
        assert_eq!(money("-0.1234"), "-0.1234");
        assert_eq!(plain("2.5000"), "2.5");
        assert_eq!(url_encode("/sales-invoices?x=1&y"), "/sales-invoices%3Fx%3D1%26y");
    }

    #[test]
    fn form_values_become_json() {
        let form = FormData(vec![
            ("name".into(), "Acme".into()),
            ("legal_name".into(), " ".into()),
            ("is_self".into(), "true".into()),
            ("parent_id".into(), "".into()),
            ("due_days".into(), "30".into()),
        ]);
        let v = Values::from_form(
            &form,
            &[("name", Kind::Str), ("legal_name", Kind::OptStr), ("is_self", Kind::Bool), ("is_active", Kind::Bool),
              ("parent_id", Kind::OptInt), ("due_days", Kind::Int)],
        )
        .unwrap()
        .json();
        assert_eq!(v, serde_json::json!({"name": "Acme", "legal_name": null, "is_self": true, "is_active": false, "parent_id": null, "due_days": 30}));
        assert!(Values::from_form(&form, &[("name", Kind::Int)]).is_err());
    }
}
