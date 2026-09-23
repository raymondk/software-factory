//! Internet Identity login. The browser signs a one-time challenge with the delegation chain II issued it; the
//! envelope is verified off-chain against the IC mainnet root key and a session of ours is minted for the principal.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use api_client::{Challenge, Login, Session};
use axum::extract::State;
use axum::{Extension, Json};
use ic_auth_verifier::{sha3_256, SignedEnvelope};

use crate::api::{ApiError, Caller, NOW};
use crate::users::hash;
use crate::AppState;

/// Outstanding challenges with their expiry. Process-wide, so an orchestrator restart voids logins in flight.
static CHALLENGES: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(Default::default);
pub const CHALLENGE_TTL: Duration = Duration::from_secs(5 * 60);
const SESSION_HOURS: i64 = 8;
const ANONYMOUS: &str = "2vxsx-fae";

fn random_hex() -> String {
    hex::encode(rand::random::<[u8; 32]>())
}

/// Issues a challenge valid for `ttl`, dropping expired ones on the way.
pub fn issue_challenge(ttl: Duration) -> String {
    let mut all = CHALLENGES.lock().unwrap();
    let now = Instant::now();
    all.retain(|_, expires| *expires > now);
    let challenge = random_hex();
    all.insert(challenge.clone(), now + ttl);
    challenge
}

/// Removes the challenge; `false` when unknown, used or expired.
fn consume_challenge(challenge: &str) -> bool {
    CHALLENGES.lock().unwrap().remove(challenge).is_some_and(|expires| expires > Instant::now())
}

pub async fn challenge() -> Json<Challenge> {
    Json(Challenge { challenge: issue_challenge(CHALLENGE_TTL) })
}

/// The digest `@ldclabs/ic-auth`'s `signMessage(identity, challenge)` signs: SHA3-256 over the deterministic CBOR of
/// the challenge string.
fn expected_digest(challenge: &str) -> [u8; 32] {
    sha3_256(&ic_auth_types::deterministic_cbor_into_vec(challenge).unwrap())
}

/// Verifies the envelope, consumes its challenge, registers an unknown principal as `pending`, mints a session.
pub async fn login(State(state): State<AppState>, Json(req): Json<Login>) -> Result<Json<Session>, ApiError> {
    let envelope = SignedEnvelope::from_base64(&req.envelope).map_err(|_| ApiError::BadRequest("malformed envelope"))?;
    let challenge = envelope.digest.as_ref().map(|d| d.as_slice());
    // The signed digest tells which challenge this is; a digest not matching any live challenge fails the same way.
    let challenge = match challenge.and_then(|digest| {
        let all = CHALLENGES.lock().unwrap();
        all.keys().find(|c| expected_digest(c) == digest).cloned()
    }) {
        Some(c) => c,
        None => return Err(ApiError::BadRequest("unknown or expired challenge")),
    };
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
    if envelope.verify(now_ms, None, Some(&expected_digest(&challenge))).is_err() {
        return Err(ApiError::BadRequest("invalid signature"));
    }
    if !consume_challenge(&challenge) {
        return Err(ApiError::BadRequest("unknown or expired challenge"));
    }
    let principal = envelope.sender().to_text();
    if principal == ANONYMOUS {
        return Err(ApiError::BadRequest("anonymous principal"));
    }
    let token = random_hex();
    let mut conn = state.pool.acquire().await?;
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    let (status,): (String,) = sqlx::query_as(&format!(
        "INSERT INTO users (principal, status, created_at) VALUES (?1, 'pending', {NOW}) ON CONFLICT (principal) DO UPDATE SET principal = principal RETURNING status"
    ))
    .bind(&principal)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(&format!("INSERT INTO sessions (token_hash, principal, created_at, expires_at) VALUES (?1, ?2, {NOW}, strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?3))"))
        .bind(hash(&token))
        .bind(&principal)
        .bind(format!("+{SESSION_HOURS} hours"))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    tracing::info!(principal = %principal, status = %status, "login");
    Ok(Json(Session { token, principal, status }))
}

/// Deletes the session the request authenticated with. A personal token or the admin token has none: 404.
pub async fn logout(State(state): State<AppState>, Extension(caller): Extension<Caller>, Extension(token): Extension<BearerToken>) -> Result<axum::http::StatusCode, ApiError> {
    if !matches!(caller, Caller::User { .. }) {
        return Err(ApiError::NotFound);
    }
    let deleted = sqlx::query("DELETE FROM sessions WHERE token_hash = ?1").bind(hash(&token.0)).execute(&state.pool).await?;
    if deleted.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// The raw bearer token of the request, set by `api::auth` so logout can find its session.
#[derive(Clone)]
pub struct BearerToken(pub String);
