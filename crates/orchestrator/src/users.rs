//! Developers: Internet Identity principals the admin approves, their sessions and personal tokens.

use api_client::{ApproveUser, CreateToken, NewToken, PersonalToken, User};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use sha2::{Digest, Sha256};
use sqlx::{Connection, SqliteExecutor};

use crate::api::{ApiError, Caller, NOW};
use crate::AppState;

/// Only hashes are stored: a leaked database reveals no token.
pub fn hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token))
}

const USER_COLUMNS: &str = "principal, name, status, created_at";
const TOKEN_COLUMNS: &str = "id, name, created_at";

#[derive(sqlx::FromRow)]
struct UserRow {
    principal: String,
    name: Option<String>,
    status: String,
    created_at: String,
}

impl From<UserRow> for User {
    fn from(r: UserRow) -> User {
        User { principal: r.principal, name: r.name, status: r.status, created_at: r.created_at }
    }
}

#[derive(sqlx::FromRow)]
struct TokenRow {
    id: i64,
    name: String,
    created_at: String,
}

/// The user behind a live session or a personal token, whatever their status.
pub async fn resolve(db: impl SqliteExecutor<'_>, token: &str) -> sqlx::Result<Option<Caller>> {
    let row: Option<UserRow> = sqlx::query_as(&format!(
        "SELECT {USER_COLUMNS} FROM users WHERE principal IN \
         (SELECT principal FROM sessions WHERE token_hash = ?1 AND expires_at > {NOW} \
          UNION SELECT principal FROM personal_tokens WHERE token_hash = ?1)"
    ))
    .bind(hash(token))
    .fetch_optional(db)
    .await?;
    Ok(row.map(|r| Caller::User { principal: r.principal, name: r.name, status: r.status }))
}

fn principal_of(caller: &Caller) -> Result<&str, ApiError> {
    match caller {
        Caller::User { principal, .. } => Ok(principal),
        _ => Err(ApiError::Forbidden),
    }
}

pub async fn me(State(state): State<AppState>, Extension(caller): Extension<Caller>) -> Result<Json<User>, ApiError> {
    let principal = principal_of(&caller).map_err(|_| ApiError::NotFound)?;
    let row: Option<UserRow> = sqlx::query_as(&format!("SELECT {USER_COLUMNS} FROM users WHERE principal = ?1")).bind(principal).fetch_optional(&state.pool).await?;
    row.map(|r| Json(r.into())).ok_or(ApiError::NotFound)
}

/// The admin sees everyone; others only approved users, to put names on owners and authors.
pub async fn list(State(state): State<AppState>, Extension(caller): Extension<Caller>) -> Result<Json<Vec<User>>, ApiError> {
    let all = matches!(caller, Caller::Admin);
    let rows: Vec<UserRow> = sqlx::query_as(&format!("SELECT {USER_COLUMNS} FROM users WHERE ?1 OR status = 'approved' ORDER BY created_at ASC, principal ASC"))
        .bind(all)
        .fetch_all(&state.pool)
        .await?;
    Ok(Json(rows.into_iter().map(User::from).collect()))
}

/// Approves a principal under `name`, creating it if it never signed in.
pub async fn approve(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(principal): Path<String>,
    Json(req): Json<ApproveUser>,
) -> Result<Json<User>, ApiError> {
    if !matches!(caller, Caller::Admin) {
        return Err(ApiError::Forbidden);
    }
    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name must not be empty"));
    }
    let row: UserRow = sqlx::query_as(&format!(
        "INSERT INTO users (principal, name, status, created_at) VALUES (?1, ?2, 'approved', {NOW}) \
         ON CONFLICT (principal) DO UPDATE SET name = excluded.name, status = 'approved' RETURNING {USER_COLUMNS}"
    ))
    .bind(&principal)
    .bind(req.name.trim())
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(row.into()))
}

/// Revokes a user and every session and personal token they hold.
pub async fn revoke(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(principal): Path<String>) -> Result<Json<User>, ApiError> {
    if !matches!(caller, Caller::Admin) {
        return Err(ApiError::Forbidden);
    }
    let mut conn = state.pool.acquire().await?;
    let mut tx = conn.begin().await?;
    let row: Option<UserRow> = sqlx::query_as(&format!("UPDATE users SET status = 'revoked' WHERE principal = ?1 RETURNING {USER_COLUMNS}"))
        .bind(&principal)
        .fetch_optional(&mut *tx)
        .await?;
    let row = row.ok_or(ApiError::NotFound)?;
    sqlx::query("DELETE FROM sessions WHERE principal = ?1").bind(&principal).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM personal_tokens WHERE principal = ?1").bind(&principal).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(row.into()))
}

pub async fn list_tokens(State(state): State<AppState>, Extension(caller): Extension<Caller>) -> Result<Json<Vec<PersonalToken>>, ApiError> {
    let principal = principal_of(&caller)?;
    let rows: Vec<TokenRow> = sqlx::query_as(&format!("SELECT {TOKEN_COLUMNS} FROM personal_tokens WHERE principal = ?1 ORDER BY id ASC"))
        .bind(principal)
        .fetch_all(&state.pool)
        .await?;
    Ok(Json(rows.into_iter().map(|r| PersonalToken { id: r.id, name: r.name, created_at: r.created_at }).collect()))
}

/// Mints a personal token for the CLI. The token is returned once and only its hash is kept.
pub async fn create_token(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(req): Json<CreateToken>,
) -> Result<(StatusCode, Json<NewToken>), ApiError> {
    let principal = principal_of(&caller)?;
    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name must not be empty"));
    }
    let (token,): (String,) = sqlx::query_as("SELECT lower(hex(randomblob(24)))").fetch_one(&state.pool).await?;
    let row: TokenRow = sqlx::query_as(&format!(
        "INSERT INTO personal_tokens (token_hash, principal, name, created_at) VALUES (?1, ?2, ?3, {NOW}) RETURNING {TOKEN_COLUMNS}"
    ))
    .bind(hash(&token))
    .bind(principal)
    .bind(req.name.trim())
    .fetch_one(&state.pool)
    .await?;
    Ok((StatusCode::CREATED, Json(NewToken { id: row.id, name: row.name, created_at: row.created_at, token })))
}

pub async fn delete_token(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<i64>) -> Result<StatusCode, ApiError> {
    let principal = principal_of(&caller)?;
    let deleted = sqlx::query("DELETE FROM personal_tokens WHERE id = ?1 AND principal = ?2").bind(id).bind(principal).execute(&state.pool).await?;
    if deleted.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}
