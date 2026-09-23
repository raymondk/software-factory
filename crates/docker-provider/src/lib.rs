pub mod api;
pub mod config;
pub mod docker;

use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::Mutex;

/// A container this provider started.
#[derive(Debug, Clone)]
pub struct Running {
    pub container_id: String,
    pub agent: String,
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<config::Config>,
    /// worker id -> container
    pub workers: Arc<Mutex<BTreeMap<String, Running>>>,
}

impl AppState {
    pub fn new(config: config::Config) -> AppState {
        AppState { config: Arc::new(config), workers: Default::default() }
    }

    /// Like `new`, but seeds the map with containers a previous instance started.
    pub async fn recover(config: config::Config) -> anyhow::Result<AppState> {
        let workers = docker::tracked().await?;
        tracing::info!(workers = workers.len(), ids = ?workers.keys().collect::<Vec<_>>(), "recovered workers from docker");
        Ok(AppState { config: Arc::new(config), workers: Arc::new(Mutex::new(workers)) })
    }
}
