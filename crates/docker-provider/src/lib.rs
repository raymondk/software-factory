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
}
