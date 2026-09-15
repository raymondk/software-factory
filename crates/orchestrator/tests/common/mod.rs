use std::collections::BTreeMap;
use std::sync::Arc;

use orchestrator::provider::{AgentInfo, Providers, Status};
use orchestrator::{api, config::Config, db, reaper, AppState};

pub const TOKEN: &str = "secret";

/// What the test providers advertise: `claude-code` with `sonnet` (default) and `opus`, `codex` with `o3`.
pub fn advertised() -> BTreeMap<String, AgentInfo> {
    BTreeMap::from([
        ("claude-code".to_string(), AgentInfo { models: vec!["sonnet".into(), "opus".into()], default_model: "sonnet".into() }),
        ("codex".to_string(), AgentInfo { models: vec!["o3".into()], default_model: "o3".into() }),
    ])
}

pub fn status(agents: BTreeMap<String, AgentInfo>) -> Status {
    Status { capacity: 4, in_use: 0, agents, workers: vec![] }
}

/// Serves the API on a random port with a fresh database. `config` is TOML; its token is replaced by `TOKEN`. Every
/// configured provider is seeded with a last status advertising `advertised()`.
pub async fn serve(config: &str) -> (String, tempfile::TempDir) {
    serve_with(config, |_| status(advertised())).await
}

/// Like `serve`, with each provider's last status from `seed`.
pub async fn serve_with(config: &str, seed: impl Fn(&str) -> Status) -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::open(&dir.path().join("test.db")).await.unwrap();
    let mut config = Config::parse(config).unwrap();
    config.orchestrator.token = TOKEN.into();
    let providers = Providers::new(&config);
    providers.statuses.write().unwrap().extend(config.providers.keys().map(|name| (name.clone(), seed(name))));
    tokio::spawn(reaper::run(pool.clone(), config.orchestrator.heartbeat_timeout));
    let router = api::router(AppState { pool, config: Arc::new(config), providers: Arc::new(providers) });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, dir)
}
