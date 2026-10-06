//! Runtime configuration from the environment, as tadmor reads it.

use crate::mailer::SmtpConfig;

/// The settings the server needs to start.
#[derive(Debug, PartialEq)]
pub struct Config {
    /// Postgres connection string (`DATABASE_URL`).
    pub database_url: String,
    /// Listen address (`HTTP_ADDR`, or `:` and `PORT`), in Go's form, where
    /// `:8080` means every interface.
    pub http_addr: String,
    /// Outbound email (`SMTP_ADDR`, `SMTP_USER`, `SMTP_PASS`, `MAIL_FROM`);
    /// with no address, email is off.
    pub smtp: SmtpConfig,
}

impl Config {
    pub fn from_env() -> Result<Config, String> {
        Config::from_vars(|key| std::env::var(key).ok())
    }

    /// Reads the settings through `get`, treating an empty value as unset.
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Result<Config, String> {
        let get = |key: &str| get(key).filter(|v| !v.is_empty());
        let database_url = get("DATABASE_URL").ok_or("DATABASE_URL is required")?;
        // Container platforms such as Cloud Run inject the listen port as PORT.
        let http_addr = match get("PORT") {
            Some(port) => format!(":{port}"),
            None => get("HTTP_ADDR").unwrap_or_else(|| ":8080".to_string()),
        };
        let smtp = SmtpConfig {
            addr: get("SMTP_ADDR").unwrap_or_default(),
            username: get("SMTP_USER").unwrap_or_default(),
            password: get("SMTP_PASS").unwrap_or_default(),
            from: get("MAIL_FROM").unwrap_or_default(),
        };
        Ok(Config { database_url, http_addr, smtp })
    }

    /// The listen address in the form `TcpListener::bind` accepts.
    pub fn listen_addr(&self) -> String {
        match self.http_addr.strip_prefix(':') {
            Some(port) => format!("0.0.0.0:{port}"),
            None => self.http_addr.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(vars: &[(&str, &str)]) -> Result<Config, String> {
        Config::from_vars(|key| vars.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string()))
    }

    #[test]
    fn database_url_is_required() {
        assert!(load(&[]).is_err());
        assert!(load(&[("DATABASE_URL", "")]).is_err());
    }

    #[test]
    fn http_addr_defaults_to_every_interface() {
        let c = load(&[("DATABASE_URL", "postgres://x")]).unwrap();
        assert_eq!(c.http_addr, ":8080");
        assert_eq!(c.listen_addr(), "0.0.0.0:8080");
    }

    #[test]
    fn port_overrides_http_addr() {
        let c = load(&[("DATABASE_URL", "postgres://x"), ("HTTP_ADDR", "127.0.0.1:9000"), ("PORT", "7000")]).unwrap();
        assert_eq!(c.listen_addr(), "0.0.0.0:7000");
        let c = load(&[("DATABASE_URL", "postgres://x"), ("HTTP_ADDR", "127.0.0.1:9000")]).unwrap();
        assert_eq!(c.listen_addr(), "127.0.0.1:9000");
    }
}
