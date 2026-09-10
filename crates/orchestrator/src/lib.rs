use std::sync::Arc;

pub mod api;
pub mod config;
pub mod db;
pub mod reaper;

pub const STATES: [&str; 6] = ["todo", "ready", "in_progress", "in_review", "failed", "done"];

#[derive(Clone)]
pub struct AppState {
    pub pool: sqlx::SqlitePool,
    pub config: Arc<config::Config>,
}
