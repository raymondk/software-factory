use std::sync::Arc;

pub mod api;
pub mod auth;
pub mod config;
pub mod db;
pub mod provider;
pub mod reaper;
pub mod scheduler;
pub mod users;

pub const STATES: [&str; 6] = ["todo", "ready", "in_progress", "in_review", "failed", "done"];

#[derive(Clone)]
pub struct AppState {
    pub pool: sqlx::SqlitePool,
    pub config: Arc<config::Config>,
    pub providers: Arc<provider::Providers>,
}
