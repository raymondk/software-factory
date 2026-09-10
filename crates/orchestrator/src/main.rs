use std::path::PathBuf;

use orchestrator::{api, config::Config, db, AppState};

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
    let state = AppState { pool, token: config.orchestrator.token.clone() };
    let listener = tokio::net::TcpListener::bind(config.orchestrator.listen).await?;
    eprintln!("orchestrator for {} listening on {}", config.project.name, listener.local_addr()?);
    axum::serve(listener, api::router(state)).await?;
    Ok(())
}
