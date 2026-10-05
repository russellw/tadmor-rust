//! User administration endpoints (spec/api.md §5.1), all administrator-only.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::AppState;
use super::extract::{Body, CurrentUser, Id};
use crate::error::Result;
use crate::users::{self, NewUser, UserRecord, UserUpdate};

pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<UserRecord>>> {
    Ok(Json(users::list(&state.pool).await?))
}

pub async fn get(State(state): State<AppState>, Id(id): Id) -> Result<Json<UserRecord>> {
    Ok(Json(users::get(&state.pool, id).await?))
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct CreateBody {
    email: String,
    full_name: String,
    password: String,
    is_admin: bool,
}

pub async fn create(State(state): State<AppState>, Body(b): Body<CreateBody>) -> Result<(StatusCode, Json<Value>)> {
    let new = NewUser { email: b.email, full_name: b.full_name, password: b.password, is_admin: b.is_admin };
    let id = users::create(&state.pool, new).await?;
    Ok((StatusCode::CREATED, Json(json!({"id": id}))))
}

/// A full replacement: omitted booleans are false (spec/api.md §1.3).
#[derive(Deserialize, Default)]
#[serde(default)]
pub struct UpdateBody {
    email: String,
    full_name: String,
    is_active: bool,
    is_admin: bool,
}

pub async fn update(
    State(state): State<AppState>,
    CurrentUser(caller): CurrentUser,
    Id(id): Id,
    Body(b): Body<UpdateBody>,
) -> Result<StatusCode> {
    let update = UserUpdate { email: b.email, full_name: b.full_name, is_active: b.is_active, is_admin: b.is_admin };
    users::update(&state.pool, &caller, id, update).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct PasswordBody {
    password: String,
}

pub async fn set_password(State(state): State<AppState>, Id(id): Id, Body(b): Body<PasswordBody>) -> Result<StatusCode> {
    users::set_password(&state.pool, id, b.password).await?;
    Ok(StatusCode::NO_CONTENT)
}
