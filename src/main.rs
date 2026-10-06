//! The server, and its out-of-band bootstrap of logins:
//!
//!     tadmor                      serve (applying pending migrations first)
//!     tadmor adduser --email=you@example.com --name='Your Name' [--admin=false]
//!                                 create or reset a login, password read
//!                                 from the first line of stdin, then exit

use std::error::Error;
use std::process::ExitCode;

use tadmor::config::Config;
use tadmor::db;
use tadmor::http::{AppState, router};
use tadmor::users::{self, NewUser};

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        None => serve().await,
        Some("adduser") => add_user(&args[1..]).await,
        Some(other) => Err(format!("unknown command {other:?}; expected none or adduser").into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("tadmor: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Applies any pending migrations, then serves until SIGINT or SIGTERM.
async fn serve() -> Result<(), Box<dyn Error>> {
    let config = Config::from_env()?;
    let pool = db::connect(&config.database_url).await?;
    db::migrate(&pool).await?;
    let mailer = tadmor::mailer::Mailer::new(&config.smtp)?.map(std::sync::Arc::new);
    if mailer.is_none() {
        eprintln!("tadmor: email is off (no SMTP_ADDR)");
    }
    let addr = config.listen_addr();
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("tadmor: listening on {addr}");
    axum::serve(listener, router(AppState { pool, mailer })).with_graceful_shutdown(shutdown()).await?;
    Ok(())
}

async fn shutdown() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

/// Creates or resets a login, migrating first so it works on a fresh
/// database. The user is an administrator unless `--admin=false`.
async fn add_user(args: &[String]) -> Result<(), Box<dyn Error>> {
    let (mut email, mut name, mut is_admin) = (String::new(), String::new(), true);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) => (flag, Some(value.to_string())),
            None => (arg.as_str(), None),
        };
        let mut value = || inline.clone().or_else(|| args.next().cloned()).ok_or(format!("{flag} needs a value"));
        match flag {
            "--email" => email = value()?,
            "--name" => name = value()?,
            "--admin" => {
                is_admin = match inline.as_deref() {
                    None | Some("true") => true,
                    Some("false") => false,
                    Some(other) => return Err(format!("--admin={other}: expected true or false").into()),
                }
            }
            _ => return Err(format!("unknown flag {flag:?}").into()),
        }
    }
    if email.is_empty() || name.is_empty() {
        return Err("adduser requires --email and --name".into());
    }
    let mut password = String::new();
    std::io::stdin().read_line(&mut password)?;
    let password = password.trim_end_matches(['\r', '\n']).to_string();

    let config = Config::from_env()?;
    let pool = db::connect(&config.database_url).await?;
    db::migrate(&pool).await?;
    let new = NewUser { email: email.clone(), full_name: name, password, is_admin };
    let id = users::upsert(&pool, new).await.map_err(|e| e.message)?;
    println!("{} {id} ({email}) ready", if is_admin { "admin" } else { "user" });
    Ok(())
}
