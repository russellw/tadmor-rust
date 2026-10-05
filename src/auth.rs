//! Passwords and login sessions, over the shared `users` and `sessions`
//! tables.
//!
//! Passwords are hashed with PBKDF2-HMAC-SHA256 at 600,000 iterations, as
//! tadmor does, and stored as `pbkdf2-sha256$<iterations>$<salt>$<key>` with
//! the salt and key in hex. Session tokens are 32 random bytes, sent to the
//! client in hex and stored only as their SHA-256.

use std::io::Read;

use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::error::{Error, Result};

const SCHEME: &str = "pbkdf2-sha256";
const ITERATIONS: u32 = 600_000;
const SALT_LEN: usize = 16;
const KEY_LEN: usize = 32;

/// How long a session lasts: a fixed 30 days from login, not sliding.
pub const SESSION_DAYS: i64 = 30;

/// A valid hash of a random, discarded password. Login verifies against it
/// when the email matches no active user, so the response takes as long as
/// a real check and does not reveal which emails exist.
const DUMMY_HASH: &str = "pbkdf2-sha256$600000$e5b1ad7f9015821a569d13a022ef38ce$d09f0f3c8069a10195075c2ed557a710432945c4816af39c78e918507c7b87b8";

pub const MIN_PASSWORD_LEN: usize = 8;

/// The authenticated identity behind a request (spec/api.md §3, `User`).
#[derive(Clone, Debug, Serialize)]
pub struct User {
    pub id: i32,
    pub email: String,
    pub full_name: String,
    pub is_admin: bool,
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .expect("read /dev/urandom");
    buf
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// Compares in time that depends only on the lengths, not the contents.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Derives a storable hash of the password. CPU-bound for a good fraction of
/// a second, so async callers go through `hash_password_async`.
pub fn hash_password(password: &str) -> String {
    let salt = random_bytes::<SALT_LEN>();
    let mut key = [0u8; KEY_LEN];
    pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), &salt, ITERATIONS, &mut key);
    format!("{SCHEME}${ITERATIONS}${}${}", to_hex(&salt), to_hex(&key))
}

/// Whether the password matches the stored hash. The iteration count comes
/// from the hash, so it can be raised later without invalidating old ones.
pub fn verify_password(stored: &str, password: &str) -> bool {
    let parts: Vec<&str> = stored.split('$').collect();
    let [scheme, iterations, salt, key] = parts[..] else { return false };
    let (Ok(iterations), Some(salt), Some(want)) = (iterations.parse::<u32>(), from_hex(salt), from_hex(key)) else {
        return false;
    };
    if scheme != SCHEME || iterations == 0 || want.is_empty() {
        return false;
    }
    let mut got = vec![0u8; want.len()];
    pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), &salt, iterations, &mut got);
    constant_time_eq(&got, &want)
}

pub async fn hash_password_async(password: String) -> Result<String> {
    tokio::task::spawn_blocking(move || hash_password(&password)).await.map_err(Error::internal)
}

async fn verify_password_async(stored: String, password: String) -> Result<bool> {
    tokio::task::spawn_blocking(move || verify_password(&stored, &password)).await.map_err(Error::internal)
}

fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Checks the credentials and opens a session, returning its user and the
/// token for the cookie. An unknown email, a wrong password, and a
/// deactivated user are the same 401, after the same hashing work.
pub async fn login(pool: &PgPool, email: &str, password: &str) -> Result<(User, String)> {
    let email = email.trim();
    if email.is_empty() || password.is_empty() {
        return Err(Error::bad_request("email and password are required"));
    }
    let row = sqlx::query!(
        r#"SELECT id, email::text AS "email!", full_name, is_admin, password_hash
           FROM users WHERE email = $1::text::citext AND is_active"#,
        email
    )
    .fetch_optional(pool)
    .await?;
    let (stored, user) = match row {
        Some(r) => (r.password_hash, Some(User { id: r.id, email: r.email, full_name: r.full_name, is_admin: r.is_admin })),
        None => (DUMMY_HASH.to_string(), None),
    };
    let matches = verify_password_async(stored, password.to_string()).await?;
    let (Some(user), true) = (user, matches) else {
        return Err(Error::unauthorized("invalid email or password"));
    };
    let token = create_session(pool, user.id).await?;
    Ok((user, token))
}

/// Mints a session for the user. Expired sessions are pruned here, so no
/// background job is needed.
async fn create_session(pool: &PgPool, user_id: i32) -> Result<String> {
    let token = to_hex(&random_bytes::<32>());
    sqlx::query!("DELETE FROM sessions WHERE expires_at < now()").execute(pool).await?;
    sqlx::query!(
        "INSERT INTO sessions (token_hash, user_id, expires_at) VALUES ($1, $2, now() + make_interval(days => $3))",
        token_hash(&token),
        user_id,
        SESSION_DAYS as i32
    )
    .execute(pool)
    .await?;
    Ok(token)
}

/// The user behind a live session token, re-read on every call so that
/// deactivation and demotion take effect at once. None if the token is
/// unknown or expired, or its user is deactivated.
pub async fn session_user(pool: &PgPool, token: &str) -> Result<Option<User>> {
    Ok(sqlx::query_as!(
        User,
        r#"SELECT u.id, u.email::text AS "email!", u.full_name, u.is_admin
           FROM sessions s JOIN users u ON u.id = s.user_id
           WHERE s.token_hash = $1 AND s.expires_at > now() AND u.is_active"#,
        token_hash(token)
    )
    .fetch_optional(pool)
    .await?)
}

/// Revokes the session. An unknown token is not an error, so logout is
/// idempotent.
pub async fn logout(pool: &PgPool, token: &str) -> Result<()> {
    sqlx::query!("DELETE FROM sessions WHERE token_hash = $1", token_hash(token)).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_verify_only_their_password() {
        let hash = hash_password("correct horse");
        assert!(hash.starts_with("pbkdf2-sha256$600000$"));
        assert!(verify_password(&hash, "correct horse"));
        assert!(!verify_password(&hash, "correct horsf"));
        assert_ne!(hash, hash_password("correct horse"), "salts differ");
    }

    #[test]
    fn malformed_hashes_never_verify() {
        for stored in ["", "pbkdf2-sha256$600000$zz$00", "bcrypt$1$00$00", "pbkdf2-sha256$0$00$00", "pbkdf2-sha256$1$00$"] {
            assert!(!verify_password(stored, "anything"), "{stored}");
        }
    }

    #[test]
    fn dummy_hash_is_well_formed() {
        let parts: Vec<&str> = DUMMY_HASH.split('$').collect();
        assert_eq!(parts.len(), 4);
        assert_eq!(from_hex(parts[3]).unwrap().len(), KEY_LEN);
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(from_hex(&to_hex(&[0, 1, 0xab, 0xff])).unwrap(), [0, 1, 0xab, 0xff]);
        assert!(from_hex("abc").is_none());
    }
}
