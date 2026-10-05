use std::error::Error;
use std::process::ExitCode;

use tadmor::config::Config;
use tadmor::db;
use tadmor::http::{AppState, router};

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("tadmor: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Applies any pending migrations, then serves until SIGINT or SIGTERM.
async fn run() -> Result<(), Box<dyn Error>> {
    let config = Config::from_env()?;
    let pool = db::connect(&config.database_url).await?;
    db::migrate(&pool).await?;
    let addr = config.listen_addr();
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("tadmor: listening on {addr}");
    axum::serve(listener, router(AppState { pool })).with_graceful_shutdown(shutdown()).await?;
    Ok(())
}

async fn shutdown() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}
