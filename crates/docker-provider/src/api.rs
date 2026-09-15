use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{docker, AppState, Running};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/workers", post(start_worker))
        .route("/workers/{id}", delete(stop_worker))
        .route("/status", get(status))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
pub struct StartWorker {
    pub worker_id: String,
    pub agent: String,
    pub orchestrator_url: String,
    pub worker_token: String,
}

#[derive(Debug, Serialize)]
pub struct Worker {
    pub worker_id: String,
    pub agent: String,
    pub container_id: String,
    /// Docker container state: `running`, `exited`, ...
    pub status: String,
}

/// An advertised agent: the models it supports and the default among them. The image stays the provider's business.
#[derive(Debug, Serialize)]
pub struct Agent {
    pub models: Vec<String>,
    pub default_model: String,
}

#[derive(Debug, Serialize)]
pub struct Status {
    pub capacity: u32,
    pub in_use: u32,
    pub agents: BTreeMap<String, Agent>,
    pub workers: Vec<Worker>,
}

pub enum ApiError {
    BadRequest(String),
    NotFound(String),
    Conflict(String),
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> ApiError {
        ApiError::Internal(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (code, msg) = match self {
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            ApiError::Conflict(m) => (StatusCode::CONFLICT, m),
            ApiError::Internal(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
        };
        (code, Json(json!({ "error": msg }))).into_response()
    }
}

/// Refreshes the worker map against Docker, dropping containers Docker no longer knows.
async fn reconcile(workers: &mut BTreeMap<String, Running>) -> anyhow::Result<Vec<Worker>> {
    let states = docker::states(workers.values().map(|r| r.container_id.as_str())).await?;
    workers.retain(|_, r| states.contains_key(&r.container_id));
    Ok(workers
        .iter()
        .map(|(w, r)| Worker { worker_id: w.clone(), agent: r.agent.clone(), container_id: r.container_id.clone(), status: states[&r.container_id].clone() })
        .collect())
}

fn in_use(workers: &[Worker]) -> u32 {
    workers.iter().filter(|w| !matches!(w.status.as_str(), "exited" | "dead")).count() as u32
}

async fn start_worker(
    State(state): State<AppState>,
    Json(req): Json<StartWorker>,
) -> Result<(StatusCode, Json<Worker>), ApiError> {
    let Some(agent) = state.config.agents.get(&req.agent) else {
        return Err(ApiError::BadRequest(format!("unknown agent {}", req.agent)));
    };
    let mut workers = state.workers.lock().await;
    let current = reconcile(&mut workers).await?;
    if workers.contains_key(&req.worker_id) {
        return Err(ApiError::Conflict(format!("worker {} already exists", req.worker_id)));
    }
    let max = state.config.provider.max_workers;
    if in_use(&current) >= max {
        return Err(ApiError::Conflict(format!("at capacity ({max} workers)")));
    }
    let mut env = state.config.worker_env.clone();
    env.insert("FACTORY_URL".into(), req.orchestrator_url);
    env.insert("FACTORY_TOKEN".into(), req.worker_token.clone());
    env.insert("FACTORY_WORKER_ID".into(), req.worker_id.clone());
    env.insert("FACTORY_WORKER_TOKEN".into(), req.worker_token);
    env.insert("FACTORY_AGENT".into(), req.agent.clone());
    env.insert("FACTORY_MODEL".into(), agent.default_model.clone());
    let container_id = docker::run(&agent.image, &req.worker_id, &req.agent, &env).await?;
    workers.insert(req.worker_id.clone(), Running { container_id: container_id.clone(), agent: req.agent.clone() });
    let worker = Worker { worker_id: req.worker_id, agent: req.agent, container_id, status: "running".into() };
    Ok((StatusCode::CREATED, Json(worker)))
}

async fn stop_worker(State(state): State<AppState>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    let mut workers = state.workers.lock().await;
    let Some(running) = workers.get(&id) else {
        return Err(ApiError::NotFound(format!("no worker {id}")));
    };
    docker::remove(&running.container_id).await?;
    workers.remove(&id);
    Ok(StatusCode::NO_CONTENT)
}

async fn status(State(state): State<AppState>) -> Result<Json<Status>, ApiError> {
    let mut workers = state.workers.lock().await;
    let workers = reconcile(&mut workers).await?;
    let agents = state.config.agents.iter().map(|(n, a)| (n.clone(), Agent { models: a.models.clone(), default_model: a.default_model.clone() })).collect();
    Ok(Json(Status { capacity: state.config.provider.max_workers, in_use: in_use(&workers), agents, workers }))
}
