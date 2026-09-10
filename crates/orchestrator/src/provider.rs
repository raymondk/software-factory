//! Client for the worker provider (spec 5).

use anyhow::Context;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartWorker {
    pub worker_id: String,
    pub worker_type: String,
    pub orchestrator_url: String,
    pub worker_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderWorker {
    pub worker_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub capacity: u32,
    pub in_use: u32,
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

async fn check(resp: reqwest::Response) -> anyhow::Result<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    anyhow::bail!("provider returned {status}: {body}")
}
