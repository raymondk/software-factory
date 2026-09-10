pub mod api;
pub mod config;
pub mod docker;

use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::Mutex;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<config::Config>,
    /// worker id -> container id
    pub workers: Arc<Mutex<BTreeMap<String, String>>>,
}

impl AppState {
    pub fn new(config: config::Config) -> AppState {
        AppState { config: Arc::new(config), workers: Default::default() }
    }

    /// Like `new`, but seeds the map with containers a previous instance started.
    pub async fn recover(config: config::Config) -> anyhow::Result<AppState> {
        let workers = docker::tracked().await?;
        eprintln!("recovered {} workers from docker", workers.len());
        Ok(AppState { config: Arc::new(config), workers: Arc::new(Mutex::new(workers)) })
    }
}
