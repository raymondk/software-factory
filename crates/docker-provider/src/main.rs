use std::path::PathBuf;

use docker_provider::{api, config::Config, AppState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path = match std::env::args().nth(1) {
        Some(p) => PathBuf::from(p),
        None => {
            eprintln!("usage: docker-provider <provider.toml>");
            std::process::exit(2);
        }
    };
    let config = Config::load(&path)?;
    let listener = tokio::net::TcpListener::bind(config.provider.listen).await?;
    let state = AppState::recover(config).await?;
    let agents: Vec<&str> = state.config.agents.keys().map(String::as_str).collect();
    eprintln!("docker provider for {} listening on {}", agents.join(", "), listener.local_addr()?);
    axum::serve(listener, api::router(state)).await?;
    Ok(())
}
