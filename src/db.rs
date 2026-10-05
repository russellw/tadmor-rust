//! The Postgres pool and the shared schema's migrations.

use std::time::Duration;

use sqlx::migrate::{MigrateError, Migrator};
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};

/// tadmor's db/migrations, embedded at compile time. SQLx records what has
/// run in its own `_sqlx_migrations` table.
pub static MIGRATOR: Migrator = sqlx::migrate!("./db/migrations");

/// Pool options for `url`, with every session in UTC: the aging views and
/// the default movement date use `current_date`, and "today" is the UTC
/// date (spec/README.md).
pub fn options(url: &str) -> Result<PgConnectOptions, sqlx::Error> {
    Ok(url.parse::<PgConnectOptions>()?.options([("timezone", "UTC")]))
}

pub fn pool_options() -> PgPoolOptions {
    // A short acquire timeout lets /readyz report an unreachable database
    // promptly instead of hanging the probe.
    PgPoolOptions::new().max_connections(10).acquire_timeout(Duration::from_secs(3))
}

pub async fn connect(url: &str) -> Result<PgPool, sqlx::Error> {
    pool_options().connect_with(options(url)?).await
}

pub async fn migrate(pool: &PgPool) -> Result<(), MigrateError> {
    MIGRATOR.run(pool).await
}
