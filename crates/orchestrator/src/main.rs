use std::path::PathBuf;
use std::sync::Arc;

use orchestrator::{api, config::Config, db, reaper, AppState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path = match std::env::args().nth(1) {
        Some(p) => PathBuf::from(p),
        None => {
            eprintln!("usage: orchestrator <config.toml>");
            std::process::exit(2);
        }
    };
    let config = Config::load(&path)?;
    let pool = db::open(config.orchestrator.database.as_deref().unwrap()).await?;
    let listener = tokio::net::TcpListener::bind(config.orchestrator.listen).await?;
    eprintln!("orchestrator for {} listening on {}", config.project.name, listener.local_addr()?);
    tokio::spawn(reaper::run(pool.clone(), config.orchestrator.heartbeat_timeout));
    axum::serve(listener, api::router(AppState { pool, config: Arc::new(config) })).await?;
    Ok(())
}
