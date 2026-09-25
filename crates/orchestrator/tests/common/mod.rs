#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::Arc;

use orchestrator::provider::{AgentInfo, Providers, Status};
use orchestrator::users::hash;
use orchestrator::{api, config::Config, db, reaper, AppState};
use sqlx::SqlitePool;

pub const TOKEN: &str = "secret";
/// The approved developer who owns the seeded providers, and a personal token of theirs.
pub const OWNER: &str = "owner-principal";
pub const OWNER_TOKEN: &str = "owner-token";
/// Names of the seeded providers; their ids are 1 and 2, in this order.
pub const PROVIDERS: [&str; 2] = ["a", "b"];
const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%fZ', 'now')";

/// What the test providers advertise: `claude-code` with `sonnet` (default) and `opus`, `codex` with `o3`.
pub fn advertised() -> BTreeMap<String, AgentInfo> {
    BTreeMap::from([
        ("claude-code".to_string(), AgentInfo { models: vec!["sonnet".into(), "opus".into()], default_model: "sonnet".into() }),
        ("codex".to_string(), AgentInfo { models: vec!["o3".into()], default_model: "o3".into() }),
    ])
}

pub fn status(agents: BTreeMap<String, AgentInfo>) -> Status {
    Status { version: None, capacity: 4, in_use: 0, agents, workers: vec![] }
}

/// Serves the API on a random port with a fresh database. `config` is TOML; its token is replaced by `TOKEN`. Two
/// providers (`PROVIDERS`, owned by `OWNER`) are seeded, each with a last status advertising `advertised()`.
pub async fn serve(config: &str) -> (String, tempfile::TempDir) {
    serve_with(config, |_| status(advertised())).await
}

/// Like `serve`, with each provider's last status from `seed`, by provider name.
pub async fn serve_with(config: &str, seed: impl Fn(&str) -> Status) -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::open(&dir.path().join("test.db")).await.unwrap();
    let mut config = Config::parse(config).unwrap();
    config.orchestrator.token = TOKEN.into();
    let providers = Providers::new(pool.clone());
    for (i, name) in PROVIDERS.iter().enumerate() {
        let id = provider(&pool, name, &format!("http://localhost:808{}", i + 1), "p").await;
        providers.statuses.write().unwrap().insert(id, seed(name));
    }
    sqlx::query(&format!("INSERT INTO personal_tokens (token_hash, principal, name, created_at) VALUES (?1, ?2, 'cli', {NOW})"))
        .bind(hash(OWNER_TOKEN))
        .bind(OWNER)
        .execute(&pool)
        .await
        .unwrap();
    tokio::spawn(reaper::run(pool.clone(), config.orchestrator.heartbeat_timeout));
    let router = api::router(AppState { pool, config: Arc::new(config), providers: Arc::new(providers) });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, dir)
}

/// A provider row owned by `OWNER` (created approved if missing, so it is the first user listed). Returns its id.
pub async fn provider(pool: &SqlitePool, name: &str, url: &str, token: &str) -> i64 {
    provider_of(pool, OWNER, name, url, token).await
}

/// A provider row owned by `owner`, an approved user (created if missing). Returns its id.
pub async fn provider_of(pool: &SqlitePool, owner: &str, name: &str, url: &str, token: &str) -> i64 {
    sqlx::query(&format!("INSERT OR IGNORE INTO users (principal, name, status, created_at) VALUES (?1, ?1, 'approved', {NOW})")).bind(owner).execute(pool).await.unwrap();
    let (id,): (i64,) = sqlx::query_as(&format!("INSERT INTO providers (owner, name, url, token, created_at) VALUES (?1, ?2, ?3, ?4, {NOW}) RETURNING id"))
        .bind(owner)
        .bind(name)
        .bind(url)
        .bind(token)
        .fetch_one(pool)
        .await
        .unwrap();
    id
}

/// Signs `principal` in as the login endpoint will: a users row (pending unless already there) and a session valid
/// for `hours` from now. Returns the session token.
pub async fn session(dir: &tempfile::TempDir, principal: &str, hours: i64) -> String {
    let pool = SqlitePool::connect(&format!("sqlite://{}/test.db", dir.path().display())).await.unwrap();
    let now = "strftime('%Y-%m-%dT%H:%M:%fZ', 'now')";
    sqlx::query(&format!("INSERT OR IGNORE INTO users (principal, status, created_at) VALUES (?1, 'pending', {now})")).bind(principal).execute(&pool).await.unwrap();
    let token = format!("session-{principal}-{hours}");
    sqlx::query(&format!("INSERT INTO sessions (token_hash, principal, created_at, expires_at) VALUES (?1, ?2, {now}, strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?3))"))
        .bind(hash(&token))
        .bind(principal)
        .bind(format!("{hours} hours"))
        .execute(&pool)
        .await
        .unwrap();
    token
}
