//! Worker providers (spec 5): developer-owned, kept in the database. The client that talks to one, the last status
//! each returned, and the `/providers` endpoints.

use std::collections::BTreeMap;
use std::sync::RwLock;

use anyhow::Context;
pub use api_client::{AgentInfo, ProviderStatus as Status, ProviderWorker};
use api_client::{CreateProvider, UpdateProvider};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::api::{ApiError, Caller, NOW};
use crate::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartWorker {
    pub worker_id: String,
    pub agent: String,
    pub orchestrator_url: String,
    pub worker_token: String,
}

#[derive(Clone)]
pub struct Provider {
    url: String,
    token: String,
    http: reqwest::Client,
}

impl Provider {
    pub fn new(url: &str, token: &str) -> Provider {
        Provider { url: url.trim_end_matches('/').to_string(), token: token.to_string(), http: reqwest::Client::new() }
    }

    pub async fn start(&self, req: &StartWorker) -> anyhow::Result<()> {
        let resp = self.http.post(format!("{}/workers", self.url)).bearer_auth(&self.token).json(req).send().await.context("provider start")?;
        check(resp).await.map(|_| ())
    }

    /// Stops a worker; one the provider no longer knows counts as stopped.
    pub async fn stop(&self, worker_id: &str) -> anyhow::Result<()> {
        let resp = self.http.delete(format!("{}/workers/{worker_id}", self.url)).bearer_auth(&self.token).send().await.context("provider stop")?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        check(resp).await.map(|_| ())
    }

    pub async fn status(&self) -> anyhow::Result<Status> {
        let resp = self.http.get(format!("{}/status", self.url)).send().await.context("provider status")?;
        check(resp).await?.json().await.context("provider status")
    }
}

/// The last status every provider returned, by id. The scheduler refreshes it; the API validates ticket agents and
/// models and filters polls against it.
pub struct Providers {
    pool: SqlitePool,
    pub statuses: RwLock<BTreeMap<i64, Status>>,
}

impl Providers {
    pub fn new(pool: SqlitePool) -> Providers {
        Providers { pool, statuses: RwLock::new(BTreeMap::new()) }
    }

    /// Loads every provider and asks each for its status, remembering each answer and forgetting removed providers.
    /// Returns a client per provider and the statuses of those that answered this time.
    pub async fn refresh(&self) -> sqlx::Result<(BTreeMap<i64, Provider>, BTreeMap<i64, Status>)> {
        let rows: Vec<(i64, String, String)> = sqlx::query_as("SELECT id, url, token FROM providers ORDER BY id").fetch_all(&self.pool).await?;
        let clients: BTreeMap<i64, Provider> = rows.into_iter().map(|(id, url, token)| (id, Provider::new(&url, &token))).collect();
        let mut fresh = BTreeMap::new();
        for (id, client) in &clients {
            match client.status().await {
                Ok(status) => {
                    fresh.insert(*id, status);
                }
                Err(e) => eprintln!("scheduler: provider {id}: {e:#}"),
            }
        }
        let mut statuses = self.statuses.write().unwrap();
        statuses.retain(|id, _| clients.contains_key(id));
        statuses.extend(fresh.clone());
        Ok((clients, fresh))
    }

    /// Whether some provider's last status advertised `agent` (any agent when `None`) supporting `model` (any when `None`).
    pub fn advertised(&self, agent: Option<&str>, model: Option<&str>) -> bool {
        self.statuses.read().unwrap().values().any(|s| {
            s.agents.iter().any(|(name, info)| agent.is_none_or(|a| a == name) && model.is_none_or(|m| info.models.contains(&m.to_string())))
        })
    }

    /// Every agent some provider's last status advertised, with the models advertised for it (in advertised order,
    /// so a default comes first).
    pub fn agents(&self) -> BTreeMap<String, Vec<String>> {
        let mut agents: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for s in self.statuses.read().unwrap().values() {
            for (name, info) in &s.agents {
                let models = agents.entry(name.clone()).or_default();
                for m in &info.models {
                    if !models.contains(m) {
                        models.push(m.clone());
                    }
                }
            }
        }
        agents
    }

    /// The models `provider` supports for `agent`, per its last status; empty when unknown.
    pub fn models(&self, provider: i64, agent: &str) -> Vec<String> {
        self.statuses.read().unwrap().get(&provider).and_then(|s| s.agents.get(agent)).map(|a| a.models.clone()).unwrap_or_default()
    }
}

async fn check(resp: reqwest::Response) -> anyhow::Result<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    anyhow::bail!("provider returned {status}: {body}")
}

#[derive(sqlx::FromRow)]
pub(crate) struct Row {
    id: i64,
    owner: String,
    name: String,
    url: String,
    token: String,
    created_at: String,
}

const COLUMNS: &str = "id, owner, name, url, token, created_at";

impl Row {
    fn into_api(self, status: Option<Status>) -> api_client::Provider {
        api_client::Provider { id: self.id, owner: self.owner, name: self.name, url: self.url, created_at: self.created_at, status }
    }
}

async fn row(state: &AppState, id: i64) -> Result<Row, ApiError> {
    let row: Option<Row> = sqlx::query_as(&format!("SELECT {COLUMNS} FROM providers WHERE id = ?1")).bind(id).fetch_optional(&state.pool).await?;
    row.ok_or(ApiError::NotFound)
}

/// Every provider with its last status; never the token.
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<api_client::Provider>>, ApiError> {
    let rows: Vec<Row> = sqlx::query_as(&format!("SELECT {COLUMNS} FROM providers ORDER BY id")).fetch_all(&state.pool).await?;
    let statuses = state.providers.statuses.read().unwrap();
    Ok(Json(rows.into_iter().map(|r| { let status = statuses.get(&r.id).cloned(); r.into_api(status) }).collect()))
}

/// A developer adds a provider of their own; `name` is unique per owner.
pub async fn create(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(req): Json<CreateProvider>,
) -> Result<(StatusCode, Json<api_client::Provider>), ApiError> {
    let Caller::User { principal, .. } = &caller else { return Err(ApiError::Forbidden) };
    if req.name.trim().is_empty() || req.url.trim().is_empty() || req.token.is_empty() {
        return Err(ApiError::BadRequest("name, url and token must not be empty"));
    }
    let row: Result<Row, sqlx::Error> =
        sqlx::query_as(&format!("INSERT INTO providers (owner, name, url, token, created_at) VALUES (?1, ?2, ?3, ?4, {NOW}) RETURNING {COLUMNS}"))
            .bind(principal)
            .bind(req.name.trim())
            .bind(req.url.trim())
            .bind(&req.token)
            .fetch_one(&state.pool)
            .await;
    match row {
        Ok(row) => Ok((StatusCode::CREATED, Json(row.into_api(None)))),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(ApiError::Conflict("you already have a provider by that name")),
        Err(e) => Err(e.into()),
    }
}

/// Owner only: url and token.
pub async fn update(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateProvider>,
) -> Result<Json<api_client::Provider>, ApiError> {
    let current = row(&state, id).await?;
    if !matches!(&caller, Caller::User { principal, .. } if *principal == current.owner) {
        return Err(ApiError::Forbidden);
    }
    let row: Row = sqlx::query_as(&format!("UPDATE providers SET url = COALESCE(?2, url), token = COALESCE(?3, token) WHERE id = ?1 RETURNING {COLUMNS}"))
        .bind(id)
        .bind(req.url.as_deref().map(str::trim))
        .bind(&req.token)
        .fetch_one(&state.pool)
        .await?;
    let status = state.providers.statuses.read().unwrap().get(&id).cloned();
    Ok(Json(row.into_api(status)))
}

/// Owner or admin. Stops the provider's workers and deletes their records.
pub async fn delete(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<i64>) -> Result<StatusCode, ApiError> {
    let current = row(&state, id).await?;
    let allowed = match &caller {
        Caller::Admin => true,
        Caller::User { principal, .. } => *principal == current.owner,
        Caller::Worker(_) => false,
    };
    if !allowed {
        return Err(ApiError::Forbidden);
    }
    remove(&state, current).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The providers `owner` added.
pub(crate) async fn owned(state: &AppState, owner: &str) -> Result<Vec<Row>, ApiError> {
    Ok(sqlx::query_as(&format!("SELECT {COLUMNS} FROM providers WHERE owner = ?1 ORDER BY id")).bind(owner).fetch_all(&state.pool).await?)
}

/// Asks the provider to stop its live workers (best effort), then deletes the provider, its workers and everything
/// they wrote (runs, log lines, usage), releasing the tickets they held.
pub(crate) async fn remove(state: &AppState, provider: Row) -> Result<(), ApiError> {
    let client = Provider::new(&provider.url, &provider.token);
    let live: Vec<(String,)> = sqlx::query_as("SELECT id FROM workers WHERE provider = ?1 AND status != 'dead'").bind(provider.id).fetch_all(&state.pool).await?;
    for (id,) in live {
        if let Err(e) = client.stop(&id).await {
            eprintln!("provider {}: stop {id}: {e:#}", provider.id);
        }
    }
    let mut tx = state.pool.begin().await?;
    let workers = "SELECT id FROM workers WHERE provider = ?1";
    sqlx::query(&format!("UPDATE tickets SET assignee = NULL, updated_at = {NOW} WHERE assignee IN ({workers})")).bind(provider.id).execute(&mut *tx).await?;
    for table in ["log_lines", "runs", "usage"] {
        sqlx::query(&format!("DELETE FROM {table} WHERE worker_id IN ({workers})")).bind(provider.id).execute(&mut *tx).await?;
    }
    sqlx::query("DELETE FROM workers WHERE provider = ?1").bind(provider.id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM providers WHERE id = ?1").bind(provider.id).execute(&mut *tx).await?;
    tx.commit().await?;
    state.providers.statuses.write().unwrap().remove(&provider.id);
    Ok(())
}
