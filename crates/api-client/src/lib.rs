use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ticket {
    pub id: i64,
    pub title: String,
    pub description: String,
    pub state: String,
    pub rank: f64,
    pub assignee: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub links: Vec<String>,
    pub comments: Vec<Comment>,
    #[serde(default)]
    pub unresolved_comments: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comment {
    pub id: i64,
    pub ticket_id: i64,
    pub author: String,
    pub body: String,
    pub created_at: String,
    pub resolved: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateComment {
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTicket {
    pub title: String,
    #[serde(default)]
    pub description: String,
}

/// Partial update. `None` leaves a field untouched; `assignee: Some(None)` clears it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateTicket {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    pub assignee: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<String>>,
}

/// Distinguishes a present-but-null field (`Some(None)`) from an omitted one (`None`).
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

/// Exactly one of `before` or `after` must be set.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MoveTicket {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListTickets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
}

/// A worker as listed by the orchestrator. `ticket` is the ticket it currently holds. Never carries the token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Worker {
    pub id: String,
    pub worker_type: String,
    /// One of: starting, idle, busy, dead
    pub status: String,
    pub created_at: String,
    pub last_heartbeat: Option<String>,
    pub ticket: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWorker {
    pub worker_type: String,
}

/// Returned once, at creation: the only time the token is visible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewWorker {
    pub id: String,
    pub worker_type: String,
    pub token: String,
}

/// Poll body. `exclude`: a ticket to hand out only if nothing else is available (the one just timed out on).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PollRequest {
    pub exclude: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PollResponse {
    pub ticket: Ticket,
    pub prompt: String,
    pub repos: Vec<String>,
    /// The worker type's `run_timeout`, e.g. "1h".
    #[serde(with = "humantime_serde")]
    pub run_timeout: Duration,
}

/// One usage report: tokens and cost (dollars) a worker spent on a ticket in one agent run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub id: i64,
    pub ticket_id: i64,
    pub worker_id: String,
    pub worker_type: String,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub cost: f64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportUsage {
    pub ticket_id: i64,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub cost: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Totals {
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub cost: f64,
    pub tickets_completed: i64,
    pub tickets_failed: i64,
}

/// Totals under one key: a ticket id, worker id, or worker type. Serializes flat, with the key as `ticket_id`,
/// `worker_id`, or `worker_type`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Breakdown<K> {
    #[serde(flatten)]
    pub key: K,
    #[serde(flatten)]
    pub totals: Totals,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TicketKey {
    pub ticket_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerKey {
    pub worker_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerTypeKey {
    pub worker_type: String,
}

/// Completed and failed counts derive from ticket states `done` and `failed`. Totals count all such tickets; a
/// breakdown counts the distinct ones with a usage record under its key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metrics {
    pub totals: Totals,
    pub per_ticket: Vec<Breakdown<TicketKey>>,
    pub per_worker: Vec<Breakdown<WorkerKey>>,
    pub per_worker_type: Vec<Breakdown<WorkerTypeKey>>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("{status}: {body}")]
    Api { status: u16, body: String },
}

pub struct Client {
    base: String,
    token: String,
    http: reqwest::Client,
}

impl Client {
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        let mut base: String = base_url.into();
        while base.ends_with('/') {
            base.pop();
        }
        Self { base, token: token.into(), http: reqwest::Client::new() }
    }

    pub async fn create_ticket(&self, req: &CreateTicket) -> Result<Ticket, Error> {
        let r = self.http.post(format!("{}/tickets", self.base)).bearer_auth(&self.token).json(req);
        Self::send(r).await
    }

    pub async fn list_tickets(&self, filter: &ListTickets) -> Result<Vec<Ticket>, Error> {
        let r = self.http.get(format!("{}/tickets", self.base)).bearer_auth(&self.token).query(filter);
        Self::send(r).await
    }

    pub async fn update_ticket(&self, id: i64, req: &UpdateTicket) -> Result<Ticket, Error> {
        let r = self.http.patch(format!("{}/tickets/{id}", self.base)).bearer_auth(&self.token).json(req);
        Self::send(r).await
    }

    pub async fn move_ticket(&self, id: i64, req: &MoveTicket) -> Result<Ticket, Error> {
        let r = self.http.post(format!("{}/tickets/{id}/move", self.base)).bearer_auth(&self.token).json(req);
        Self::send(r).await
    }

    pub async fn get_ticket(&self, id: i64) -> Result<Ticket, Error> {
        let r = self.http.get(format!("{}/tickets/{id}", self.base)).bearer_auth(&self.token);
        Self::send(r).await
    }

    pub async fn list_comments(&self, id: i64) -> Result<Vec<Comment>, Error> {
        let r = self.http.get(format!("{}/tickets/{id}/comments", self.base)).bearer_auth(&self.token);
        Self::send(r).await
    }

    pub async fn add_comment(&self, id: i64, req: &CreateComment) -> Result<Comment, Error> {
        let r = self.http.post(format!("{}/tickets/{id}/comments", self.base)).bearer_auth(&self.token).json(req);
        Self::send(r).await
    }

    pub async fn resolve_comment(&self, id: i64, cid: i64) -> Result<Comment, Error> {
        let r = self.http.post(format!("{}/tickets/{id}/comments/{cid}/resolve", self.base)).bearer_auth(&self.token);
        Self::send(r).await
    }

    pub async fn create_worker(&self, req: &CreateWorker) -> Result<NewWorker, Error> {
        let r = self.http.post(format!("{}/workers", self.base)).bearer_auth(&self.token).json(req);
        Self::send(r).await
    }

    pub async fn list_workers(&self) -> Result<Vec<Worker>, Error> {
        let r = self.http.get(format!("{}/workers", self.base)).bearer_auth(&self.token);
        Self::send(r).await
    }

    pub async fn register(&self, id: &str) -> Result<Worker, Error> {
        let r = self.http.post(format!("{}/workers/{id}/register", self.base)).bearer_auth(&self.token);
        Self::send(r).await
    }

    pub async fn heartbeat(&self, id: &str) -> Result<Worker, Error> {
        let r = self.http.post(format!("{}/workers/{id}/heartbeat", self.base)).bearer_auth(&self.token);
        Self::send(r).await
    }

    /// `None` when no ticket is available.
    pub async fn poll(&self, id: &str, exclude: Option<i64>) -> Result<Option<PollResponse>, Error> {
        let r = self.http.post(format!("{}/workers/{id}/poll", self.base)).bearer_auth(&self.token).json(&PollRequest { exclude });
        let resp = Self::check(r).await?;
        if resp.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(None);
        }
        Ok(Some(resp.json().await?))
    }

    pub async fn report_usage(&self, id: &str, req: &ReportUsage) -> Result<Usage, Error> {
        let r = self.http.post(format!("{}/workers/{id}/usage", self.base)).bearer_auth(&self.token).json(req);
        Self::send(r).await
    }

    pub async fn metrics(&self) -> Result<Metrics, Error> {
        let r = self.http.get(format!("{}/metrics", self.base)).bearer_auth(&self.token);
        Self::send(r).await
    }

    async fn send<T: for<'de> Deserialize<'de>>(r: reqwest::RequestBuilder) -> Result<T, Error> {
        Ok(Self::check(r).await?.json().await?)
    }

    async fn check(r: reqwest::RequestBuilder) -> Result<reqwest::Response, Error> {
        let resp = r.send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(Error::Api { status: status.as_u16(), body: resp.text().await.unwrap_or_default() });
        }
        Ok(resp)
    }
}
