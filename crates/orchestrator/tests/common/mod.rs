use std::sync::Arc;

use orchestrator::{api, config::Config, db, AppState};

pub const TOKEN: &str = "secret";

/// Serves the API on a random port with a fresh database. `config` is TOML; its token is replaced by `TOKEN`.
pub async fn serve(config: &str) -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::open(&dir.path().join("test.db")).await.unwrap();
    let mut config = Config::parse(config).unwrap();
    config.orchestrator.token = TOKEN.into();
    let router = api::router(AppState { pool, config: Arc::new(config) });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, dir)
}
