pub mod api;
pub mod config;
pub mod db;

#[derive(Clone)]
pub struct AppState {
    pub pool: sqlx::SqlitePool,
    pub token: String,
}
