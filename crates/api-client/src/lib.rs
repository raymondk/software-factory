use serde::{Deserialize, Serialize};

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
    pub comments: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTicket {
    pub title: String,
    #[serde(default)]
    pub description: String,
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

    pub async fn list_tickets(&self) -> Result<Vec<Ticket>, Error> {
        let r = self.http.get(format!("{}/tickets", self.base)).bearer_auth(&self.token);
        Self::send(r).await
    }

    pub async fn get_ticket(&self, id: i64) -> Result<Ticket, Error> {
        let r = self.http.get(format!("{}/tickets/{id}", self.base)).bearer_auth(&self.token);
        Self::send(r).await
    }

    async fn send<T: for<'de> Deserialize<'de>>(r: reqwest::RequestBuilder) -> Result<T, Error> {
        let resp = r.send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(Error::Api { status: status.as_u16(), body: resp.text().await.unwrap_or_default() });
        }
        Ok(resp.json().await?)
    }
}
