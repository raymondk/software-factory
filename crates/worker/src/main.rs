use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context};
use api_client::{Client, CreateComment, Error, ReportUsage, UpdateTicket};

mod adapter;
use adapter::{Adapter, CommandAdapter, Outcome, Usage};

enum Failure {
    /// The orchestrator rejected the token: the worker was reaped. Exit.
    Unauthorized,
    Api(Error),
}

struct Worker {
    client: Client,
    id: String,
    url: String,
    token: String,
    workspace: PathBuf,
    poll_interval: Duration,
}

fn env(name: &str) -> anyhow::Result<String> {
    std::env::var(name).with_context(|| format!("{name} is not set"))
}

fn duration(name: &str, default: &str) -> anyhow::Result<Duration> {
    let text = std::env::var(name).unwrap_or_else(|_| default.into());
    humantime::parse_duration(&text).with_context(|| format!("{name}={text:?}"))
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("worker: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> anyhow::Result<()> {
    let url = env("FACTORY_URL")?;
    let id = env("FACTORY_WORKER_ID")?;
    let token = std::env::var("FACTORY_WORKER_TOKEN").or_else(|_| env("FACTORY_TOKEN"))?;
    let worker_type = std::env::var("FACTORY_WORKER_TYPE").unwrap_or_default();
    let workspace = match std::env::var_os("FACTORY_WORKSPACE") {
        Some(dir) => PathBuf::from(dir),
        None if PathBuf::from("/workspace").is_dir() => PathBuf::from("/workspace"),
        None => std::env::temp_dir().join(format!("factory-{id}")),
    };
    let heartbeat_interval = duration("FACTORY_HEARTBEAT_INTERVAL", "10s")?;
    let poll_interval = duration("FACTORY_POLL_INTERVAL", "5s")?;
    let agent = std::env::var("FACTORY_AGENT").unwrap_or_else(|_| "command".into());
    let worker = Arc::new(Worker { client: Client::new(&url, &token), id, url, token, workspace, poll_interval });
    eprintln!("worker {} ({worker_type}, agent {agent}) starting; workspace {}", worker.id, worker.workspace.display());
    match agent.as_str() {
        "command" => worker.serve(&CommandAdapter { command: env("FACTORY_AGENT_COMMAND")? }, heartbeat_interval).await,
        other => bail!("unknown FACTORY_AGENT {other:?}"),
    }
}

async fn shutdown() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
}

impl Worker {
    async fn serve<A: Adapter>(self: &Arc<Self>, adapter: &A, heartbeat_interval: Duration) -> anyhow::Result<()> {
        let reaped = || anyhow::anyhow!("token rejected: reaped by the orchestrator");
        match self.call("register", || self.client.register(&self.id)).await {
            Ok(_) => {}
            Err(Failure::Unauthorized) => return Err(reaped()),
            Err(Failure::Api(e)) => return Err(e).context("registering"),
        }
        let (dead_tx, mut dead_rx) = tokio::sync::oneshot::channel();
        let w = self.clone();
        tokio::spawn(async move {
            w.heartbeat(heartbeat_interval).await;
            let _ = dead_tx.send(());
        });
        let mut stop = Box::pin(shutdown());
        loop {
            tokio::select! {
                _ = &mut stop => {
                    eprintln!("worker {}: stopping", self.id);
                    return Ok(());
                }
                _ = &mut dead_rx => return Err(reaped()),
                r = self.step(adapter) => match r {
                    Ok(()) => {}
                    Err(Failure::Unauthorized) => return Err(reaped()),
                    Err(Failure::Api(e)) => {
                        eprintln!("worker {}: {e}", self.id);
                        tokio::time::sleep(self.poll_interval).await;
                    }
                },
            }
        }
    }

    /// Heartbeats forever; returns only once the token stops authenticating.
    async fn heartbeat(&self, every: Duration) {
        let mut interval = tokio::time::interval(every);
        interval.tick().await;
        loop {
            interval.tick().await;
            match self.client.heartbeat(&self.id).await {
                Ok(_) => {}
                Err(Error::Api { status: 401, .. }) => return,
                Err(e) => eprintln!("worker {}: heartbeat: {e}", self.id),
            }
        }
    }

    /// Calls the orchestrator, retrying connection errors and 5xx with backoff.
    async fn call<T>(&self, what: &str, f: impl AsyncFn() -> Result<T, Error>) -> Result<T, Failure> {
        let mut delay = Duration::from_secs(1);
        loop {
            match f().await {
                Ok(v) => return Ok(v),
                Err(Error::Api { status: 401, .. }) => return Err(Failure::Unauthorized),
                Err(Error::Api { status, body }) if status < 500 => return Err(Failure::Api(Error::Api { status, body })),
                Err(e) => {
                    eprintln!("worker {}: {what}: {e}; retrying in {}", self.id, humantime::format_duration(delay));
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_secs(30));
                }
            }
        }
    }

    /// One poll: idles when nothing is available, otherwise runs the agent on the ticket and settles it afterwards.
    async fn step<A: Adapter>(&self, adapter: &A) -> Result<(), Failure> {
        let Some(job) = self.call("poll", || self.client.poll(&self.id)).await? else {
            tokio::time::sleep(self.poll_interval).await;
            return Ok(());
        };
        let ticket = job.ticket.id;
        eprintln!("worker {}: ticket #{ticket} ({}): {}", self.id, job.ticket.state, job.ticket.title);
        let result = match self.prepare(ticket) {
            Ok(env) => adapter.run(&job.prompt, &self.workspace, job.run_timeout, &env).await,
            Err(e) => Err(e),
        };
        let (outcome, usage) = result.unwrap_or_else(|e| {
            (Outcome { success: false, timed_out: false, summary: format!("agent could not run: {e:#}"), links: vec![] }, Usage::default())
        });
        eprintln!(
            "worker {}: ticket #{ticket}: {} (success {}, links {:?}; {} in, {} out, ${:.4})",
            self.id, outcome.summary, outcome.success, outcome.links, usage.tokens_in, usage.tokens_out, usage.cost
        );
        let report = ReportUsage { ticket_id: ticket, tokens_in: usage.tokens_in, tokens_out: usage.tokens_out, cost: usage.cost };
        self.call("report usage", || self.client.report_usage(&self.id, &report)).await?;
        // A state change by the agent releases the ticket; one still held is one the agent never moved.
        let current = self.call("get ticket", || self.client.get_ticket(ticket)).await?;
        let held = current.assignee.as_deref() == Some(&self.id);
        let (comment, patch) = if outcome.timed_out {
            let body = format!("Run timed out after {}; leaving {} for another worker", humantime::format_duration(job.run_timeout), current.state);
            (Some(body), held.then(|| UpdateTicket { assignee: Some(None), ..Default::default() }))
        } else if held {
            let body = format!("Agent finished without moving the ticket out of {}; marking it failed ({})", current.state, outcome.summary);
            (Some(body), Some(UpdateTicket { state: Some("failed".into()), ..Default::default() }))
        } else {
            (None, None)
        };
        if let Some(body) = comment {
            let comment = CreateComment { body };
            self.call("comment", || self.client.add_comment(ticket, &comment)).await?;
        }
        if let Some(patch) = patch {
            self.call("update ticket", || self.client.update_ticket(ticket, &patch)).await?;
        }
        Ok(())
    }

    /// Creates the workspace and returns the agent's extra environment: orchestrator access and, when `GIT_TOKEN` is
    /// set, git and gh credentials (a `.git-credentials` store in the workspace, wired in via `GIT_CONFIG_*`).
    fn prepare(&self, ticket: i64) -> anyhow::Result<Vec<(String, String)>> {
        std::fs::create_dir_all(&self.workspace).with_context(|| format!("creating {}", self.workspace.display()))?;
        let mut env = vec![
            ("FACTORY_URL".to_string(), self.url.clone()),
            ("FACTORY_TOKEN".to_string(), self.token.clone()),
            ("FACTORY_WORKER_ID".to_string(), self.id.clone()),
            ("FACTORY_TICKET".to_string(), ticket.to_string()),
        ];
        if let Ok(token) = std::env::var("GIT_TOKEN") {
            use std::os::unix::fs::PermissionsExt;
            let file = self.workspace.join(".git-credentials");
            std::fs::write(&file, format!("https://x-access-token:{token}@github.com\n"))?;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600))?;
            env.extend([
                ("GIT_CONFIG_COUNT".to_string(), "1".to_string()),
                ("GIT_CONFIG_KEY_0".to_string(), "credential.helper".to_string()),
                ("GIT_CONFIG_VALUE_0".to_string(), format!("store --file={}", file.display())),
                ("GH_TOKEN".to_string(), token),
            ]);
        }
        Ok(env)
    }
}
