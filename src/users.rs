//! User administration (spec/api.md §5.1), and the out-of-band bootstrap of
//! the first administrator.

use serde::Serialize;
use sqlx::PgPool;

use crate::auth::{MIN_PASSWORD_LEN, User, hash_password_async};
use crate::error::{Error, Result};

/// `UserRecord`. The password hash never appears in any response.
#[derive(Debug, Serialize)]
pub struct UserRecord {
    pub id: i32,
    pub email: String,
    pub full_name: String,
    pub is_active: bool,
    pub is_admin: bool,
}

pub struct NewUser {
    pub email: String,
    pub full_name: String,
    pub password: String,
    pub is_admin: bool,
}

pub struct UserUpdate {
    pub email: String,
    pub full_name: String,
    pub is_active: bool,
    pub is_admin: bool,
}

/// A missing email or name is a 400, and an email without `@` a 422.
/// Returns them trimmed.
fn email_and_name(email: &str, full_name: &str) -> Result<(String, String)> {
    let (email, full_name) = (email.trim(), full_name.trim());
    if email.is_empty() {
        return Err(Error::bad_request("email is required"));
    }
    if full_name.is_empty() {
        return Err(Error::bad_request("full_name is required"));
    }
    if !email.contains('@') {
        return Err(Error::unprocessable("email must contain @"));
    }
    Ok((email.to_string(), full_name.to_string()))
}

/// A missing password is a 400, and a short one a 422.
pub fn check_password(password: &str) -> Result<()> {
    if password.is_empty() {
        return Err(Error::bad_request("password is required"));
    }
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(Error::unprocessable(format!("password must be at least {MIN_PASSWORD_LEN} characters")));
    }
    Ok(())
}

pub async fn list(pool: &PgPool) -> Result<Vec<UserRecord>> {
    Ok(sqlx::query_as!(
        UserRecord,
        r#"SELECT id, email::text AS "email!", full_name, is_active, is_admin FROM users ORDER BY email"#
    )
    .fetch_all(pool)
    .await?)
}

pub async fn get(pool: &PgPool, id: i64) -> Result<UserRecord> {
    sqlx::query_as!(
        UserRecord,
        r#"SELECT id, email::text AS "email!", full_name, is_active, is_admin FROM users WHERE id = $1::int8"#,
        id
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(Error::not_found)
}

pub async fn create(pool: &PgPool, new: NewUser) -> Result<i32> {
    let (email, full_name) = email_and_name(&new.email, &new.full_name)?;
    check_password(&new.password)?;
    let hash = hash_password_async(new.password).await?;
    Ok(sqlx::query_scalar!(
        "INSERT INTO users (email, full_name, password_hash, is_admin) VALUES ($1::text::citext, $2, $3, $4) RETURNING id",
        email,
        full_name,
        hash,
        new.is_admin
    )
    .fetch_one(pool)
    .await?)
}

/// Replaces the user's record. Administrators may not deactivate or demote
/// themselves, the guard against locking every administrator out one click
/// at a time. Deactivation and demotion need no session cleanup, because
/// every request re-reads the user.
pub async fn update(pool: &PgPool, caller: &User, id: i64, update: UserUpdate) -> Result<()> {
    let (email, full_name) = email_and_name(&update.email, &update.full_name)?;
    if i64::from(caller.id) == id {
        if !update.is_active {
            return Err(Error::unprocessable("you cannot deactivate your own account"));
        }
        if !update.is_admin {
            return Err(Error::unprocessable("you cannot remove your own administrator access"));
        }
    }
    let done = sqlx::query!(
        "UPDATE users SET email = $2::text::citext, full_name = $3, is_active = $4, is_admin = $5 WHERE id = $1::int8",
        id,
        email,
        full_name,
        update.is_active,
        update.is_admin
    )
    .execute(pool)
    .await?;
    if done.rows_affected() == 0 {
        return Err(Error::not_found());
    }
    Ok(())
}

/// Sets a new password and revokes all of the user's sessions, so whoever
/// held the old password stops being logged in.
pub async fn set_password(pool: &PgPool, id: i64, password: String) -> Result<()> {
    check_password(&password)?;
    let hash = hash_password_async(password).await?;
    let mut tx = pool.begin().await?;
    let done = sqlx::query!("UPDATE users SET password_hash = $2 WHERE id = $1::int8", id, hash).execute(&mut *tx).await?;
    if done.rows_affected() == 0 {
        return Err(Error::not_found());
    }
    sqlx::query!("DELETE FROM sessions WHERE user_id = $1::int8", id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

/// The bootstrap: creates the user or, if the email exists, resets its name,
/// password and role and reactivates it. Whoever runs this on the server can
/// already do anything to the database.
pub async fn upsert(pool: &PgPool, new: NewUser) -> Result<i32> {
    let (email, full_name) = email_and_name(&new.email, &new.full_name)?;
    check_password(&new.password)?;
    let hash = hash_password_async(new.password).await?;
    Ok(sqlx::query_scalar!(
        "INSERT INTO users (email, full_name, password_hash, is_admin) VALUES ($1::text::citext, $2, $3, $4)
         ON CONFLICT (email) DO UPDATE
         SET full_name = EXCLUDED.full_name, password_hash = EXCLUDED.password_hash,
             is_active = true, is_admin = EXCLUDED.is_admin
         RETURNING id",
        email,
        full_name,
        hash,
        new.is_admin
    )
    .fetch_one(pool)
    .await?)
}
