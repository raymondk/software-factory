use std::path::PathBuf;
use std::sync::Arc;

use orchestrator::{api, config::Config, db, provider::Providers, reaper, scheduler, AppState};

/// Timestamped, leveled lines on stderr; `RUST_LOG` (e.g. `debug`, `orchestrator::scheduler=debug`) sets verbosity, default `info`.
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
    init_logging();
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
    tracing::info!(project = %config.project.name, addr = %listener.local_addr()?, database = %config.orchestrator.database.as_deref().unwrap().display(), "orchestrator listening");
    let config = Arc::new(config);
    let providers = Arc::new(Providers::new(pool.clone()));
    tokio::spawn(reaper::run(pool.clone(), config.orchestrator.heartbeat_timeout));
    tokio::spawn(reaper::run_purge(pool.clone(), config.orchestrator.log_retention));
    tokio::spawn(scheduler::run(pool.clone(), config.clone(), providers.clone()));
    axum::serve(listener, api::router(AppState { pool, config, providers })).await?;
    Ok(())
}
