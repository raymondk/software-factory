//! Client for the worker provider (spec 5).

use std::collections::BTreeMap;
use std::sync::RwLock;

use anyhow::Context;

use crate::config::Config;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartWorker {
    pub worker_id: String,
    pub agent: String,
    pub orchestrator_url: String,
    pub worker_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderWorker {
    pub worker_id: String,
    #[serde(default)]
    pub agent: String,
    pub status: String,
}

/// An agent a provider advertises: the models it supports and the default among them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub models: Vec<String>,
    pub default_model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub capacity: u32,
    pub in_use: u32,
    #[serde(default)]
    pub agents: BTreeMap<String, AgentInfo>,
    pub workers: Vec<ProviderWorker>,
}

#[derive(Clone)]
pub struct Provider {
    url: String,
    http: reqwest::Client,
}

impl Provider {
    pub fn new(url: &str) -> Provider {
        Provider { url: url.trim_end_matches('/').to_string(), http: reqwest::Client::new() }
    }

    pub async fn start(&self, req: &StartWorker) -> anyhow::Result<()> {
        let resp = self.http.post(format!("{}/workers", self.url)).json(req).send().await.context("provider start")?;
        check(resp).await.map(|_| ())
    }

    /// Stops a worker; one the provider no longer knows counts as stopped.
    pub async fn stop(&self, worker_id: &str) -> anyhow::Result<()> {
        let resp = self.http.delete(format!("{}/workers/{worker_id}", self.url)).send().await.context("provider stop")?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        check(resp).await.map(|_| ())
    }

    pub async fn status(&self) -> anyhow::Result<Status> {
        let resp = self.http.get(format!("{}/status", self.url)).send().await.context("provider status")?;
        check(resp).await?.json().await.context("provider status")
    }
}

/// Every configured provider, with the last status each returned. Shared by the scheduler, which refreshes it, and
/// the API, which validates ticket agents and models and filters polls against it.
pub struct Providers {
    pub clients: BTreeMap<String, Provider>,
    pub statuses: RwLock<BTreeMap<String, Status>>,
}

impl Providers {
    pub fn new(config: &Config) -> Providers {
        let clients = config.providers.iter().map(|(name, p)| (name.clone(), Provider::new(&p.url))).collect();
        Providers { clients, statuses: RwLock::new(BTreeMap::new()) }
    }

    /// Asks every provider for its status, remembering each answer. Returns the ones that answered this time.
    pub async fn refresh(&self) -> BTreeMap<String, Status> {
        let mut fresh = BTreeMap::new();
        for (name, client) in &self.clients {
            match client.status().await {
                Ok(status) => {
                    fresh.insert(name.clone(), status);
                }
                Err(e) => eprintln!("scheduler: provider {name}: {e:#}"),
            }
        }
        self.statuses.write().unwrap().extend(fresh.clone());
        fresh
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
    pub fn models(&self, provider: &str, agent: &str) -> Vec<String> {
        self.statuses.read().unwrap().get(provider).and_then(|s| s.agents.get(agent)).map(|a| a.models.clone()).unwrap_or_default()
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
