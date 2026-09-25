use std::path::PathBuf;

use docker_provider::{api, config::Config, AppState};

/// Timestamped, leveled lines on stderr; `RUST_LOG` (e.g. `debug`, `docker_provider::docker=debug`) sets verbosity, default `info`.
/// Libraries (sqlx, hyper) stay at `warn` unless `RUST_LOG` names them.
fn init_logging() {
    use std::io::IsTerminal;
    let env = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into());
    let mut filter = tracing_subscriber::EnvFilter::new(&env);
    for lib in ["sqlx", "hyper", "h2", "reqwest"] {
        if !env.contains(lib) {
            filter = filter.add_directive(format!("{lib}=warn").parse().unwrap());
        }
    }
    tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).with_ansi(std::io::stderr().is_terminal()).init();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path = match std::env::args().nth(1).as_deref() {
        Some("--version" | "-V") => {
            println!("docker-provider {}", api_client::VERSION);
            return Ok(());
        }
        Some(p) => PathBuf::from(p),
        None => {
            eprintln!("usage: docker-provider <provider.toml>");
            std::process::exit(2);
        }
    };
    init_logging();
    let config = Config::load(&path)?;
    let listener = tokio::net::TcpListener::bind(config.provider.listen).await?;
    let state = AppState::recover(config).await?;
    let agents: Vec<&str> = state.config.agents.keys().map(String::as_str).collect();
    tracing::info!(version = api_client::VERSION, agents = %agents.join(", "), addr = %listener.local_addr()?, max_workers = state.config.provider.max_workers, "docker provider listening");
    axum::serve(listener, api::router(state)).await?;
    Ok(())
}
