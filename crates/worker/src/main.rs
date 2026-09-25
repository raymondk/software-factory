use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context};
use api_client::{Client, CreateComment, Error, ReportUsage, UpdateTicket};

mod adapter;
mod log;
use adapter::{Adapter, ClaudeCodeAdapter, CommandAdapter, Outcome, Usage};
use log::{Log, Shipping};

enum Failure {
    /// The orchestrator rejected the token: the worker was reaped. Exit.
    Unauthorized,
    /// A 2xx this worker cannot read: its image and the orchestrator are out of step. Exit; retrying cannot help.
    Incompatible(Error),
    Api(Error),
}

struct Worker {
    client: Client,
    id: String,
    url: String,
    token: String,
    workspace: PathBuf,
    poll_interval: Duration,
    /// The provider's default model for this agent, used when the ticket sets none.
    model: Option<String>,
    log: Log,
}

fn env(name: &str) -> anyhow::Result<String> {
    std::env::var(name).with_context(|| format!("{name} is not set"))
}

fn duration(name: &str, default: &str) -> anyhow::Result<Duration> {
    let text = std::env::var(name).unwrap_or_else(|_| default.into());
    humantime::parse_duration(&text).with_context(|| format!("{name}={text:?}"))
}

fn number(name: &str, default: usize) -> anyhow::Result<usize> {
    match std::env::var(name) {
        Ok(text) => text.parse().with_context(|| format!("{name}={text:?}")),
        Err(_) => Ok(default),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    if matches!(std::env::args().nth(1).as_deref(), Some("--version" | "-V")) {
        println!("worker {}", api_client::VERSION);
        return ExitCode::SUCCESS;
    }
    match run().await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("worker: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// `Err` only for configuration problems, before the log ships; a failure while serving is logged and flushed.
async fn run() -> anyhow::Result<ExitCode> {
    let url = env("FACTORY_URL")?;
    let id = env("FACTORY_WORKER_ID")?;
    let token = std::env::var("FACTORY_WORKER_TOKEN").or_else(|_| env("FACTORY_TOKEN"))?;
    let workspace = match std::env::var_os("FACTORY_WORKSPACE") {
        Some(dir) => PathBuf::from(dir),
        None if PathBuf::from("/workspace").is_dir() => PathBuf::from("/workspace"),
        None => std::env::temp_dir().join(format!("factory-{id}")),
    };
    let heartbeat_interval = duration("FACTORY_HEARTBEAT_INTERVAL", "10s")?;
    let poll_interval = duration("FACTORY_POLL_INTERVAL", "5s")?;
    let agent = std::env::var("FACTORY_AGENT").unwrap_or_else(|_| "command".into());
    let model = std::env::var("FACTORY_MODEL").ok().filter(|m| !m.is_empty());
    let shipping = Shipping {
        interval: duration("FACTORY_LOG_INTERVAL", "1s")?,
        batch: number("FACTORY_LOG_BATCH", 100)?,
        max_line: number("FACTORY_LOG_MAX_LINE", 16 * 1024)?,
    };
    let command = match agent.as_str() {
        "command" => Some(env("FACTORY_AGENT_COMMAND")?),
        "claude-code" => None,
        other => bail!("unknown FACTORY_AGENT {other:?}"),
    };
    let log = Log::start(Client::new(&url, &token), id.clone(), shipping);
    let worker = Arc::new(Worker { client: Client::new(&url, &token), id, url, token, workspace, poll_interval, model, log });
    worker.log.line(format!("worker {} v{} (agent {agent}) starting; workspace {}", worker.id, api_client::VERSION, worker.workspace.display()));
    let result = match command {
        Some(command) => worker.serve(&CommandAdapter { command }, heartbeat_interval).await,
        None => {
            let bin = std::env::var("FACTORY_CLAUDE_BIN").unwrap_or_else(|_| "claude".into());
            worker.serve(&ClaudeCodeAdapter { bin }, heartbeat_interval).await
        }
    };
    let code = match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            worker.log.line(format!("worker: {e:#}"));
            ExitCode::FAILURE
        }
    };
    worker.log.flush().await;
    Ok(code)
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
        let incompatible = |e: Error| anyhow::anyhow!("{e}; the worker image and the orchestrator are out of step, exiting");
        match self.call("register", || self.client.register(&self.id)).await {
            Ok(_) => {}
            Err(Failure::Unauthorized) => return Err(reaped()),
            Err(Failure::Incompatible(e)) => return Err(incompatible(e)),
            Err(Failure::Api(e)) => return Err(e).context("registering"),
        }
        let (dead_tx, mut dead_rx) = tokio::sync::oneshot::channel();
        let w = self.clone();
        tokio::spawn(async move {
            w.heartbeat(heartbeat_interval).await;
            let _ = dead_tx.send(());
        });
        let mut stop = Box::pin(shutdown());
        // The ticket just timed out on: skipped on the very next poll while other work exists.
        let mut exclude = None;
        loop {
            tokio::select! {
                _ = &mut stop => {
                    self.log.line(format!("worker {}: stopping", self.id));
                    return Ok(());
                }
                _ = &mut dead_rx => return Err(reaped()),
                r = self.step(adapter, exclude.take()) => match r {
                    Ok(next) => exclude = next,
                    Err(Failure::Unauthorized) => return Err(reaped()),
                    Err(Failure::Incompatible(e)) => return Err(incompatible(e)),
                    Err(Failure::Api(e)) => {
                        self.log.line(format!("worker {}: {e}", self.id));
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
                Err(e) => self.log.line(format!("worker {}: heartbeat: {e}", self.id)),
            }
        }
    }

    /// Calls the orchestrator, retrying connection errors and 5xx with backoff. A body it cannot decode is fatal.
    async fn call<T>(&self, what: &str, f: impl AsyncFn() -> Result<T, Error>) -> Result<T, Failure> {
        let mut delay = Duration::from_secs(1);
        loop {
            match f().await {
                Ok(v) => return Ok(v),
                Err(Error::Api { status: 401, .. }) => return Err(Failure::Unauthorized),
                Err(Error::Api { status, body }) if status < 500 => return Err(Failure::Api(Error::Api { status, body })),
                Err(e @ Error::Decode { .. }) => return Err(Failure::Incompatible(e)),
                Err(e) => {
                    self.log.line(format!("worker {}: {what}: {e}; retrying in {}", self.id, humantime::format_duration(delay)));
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_secs(30));
                }
            }
        }
    }

    /// One poll: idles when nothing is available, otherwise runs the agent on the ticket and settles it afterwards.
    /// Returns the ticket to exclude from the next poll, if the run timed out.
    async fn step<A: Adapter>(&self, adapter: &A, exclude: Option<i64>) -> Result<Option<i64>, Failure> {
        let Some(job) = self.call("poll", || self.client.poll(&self.id, exclude)).await? else {
            tokio::time::sleep(self.poll_interval).await;
            return Ok(None);
        };
        let ticket = job.ticket.id;
        let model = job.model.as_deref().or(self.model.as_deref());
        self.log.set_run(Some(job.run));
        self.log.line(format!(
            "worker {}: ticket #{ticket} ({}), run {}, model {}: {}",
            self.id, job.ticket.state, job.run, model.unwrap_or("default"), job.ticket.title
        ));
        let result = match self.prepare(ticket, &job.repos) {
            Ok(env) => adapter.run(&job.prompt, model, &self.workspace, job.run_timeout, &env, &self.log).await,
            Err(e) => Err(e),
        };
        let (outcome, usage) = result.unwrap_or_else(|e| {
            (Outcome { success: false, timed_out: false, summary: format!("agent could not run: {e:#}"), links: vec![] }, Usage::default())
        });
        self.log.line(format!(
            "worker {}: ticket #{ticket}: {} (success {}, links {:?}; {} in, {} out, ${:.4})",
            self.id, outcome.summary, outcome.success, outcome.links, usage.tokens_in, usage.tokens_out, usage.cost
        ));
        let report = ReportUsage { ticket_id: ticket, tokens_in: usage.tokens_in, tokens_out: usage.tokens_out, cost: usage.cost, model: model.map(str::to_owned) };
        // The usage report ends the run on the orchestrator; lines from here on belong to no run.
        self.call("report usage", || self.client.report_usage(&self.id, &report)).await?;
        self.log.set_run(None);
        // Relative to the UI, so it works from whatever address a browser opens the UI at.
        let log_link = format!("[run {run}](#/tickets/{ticket}/runs/{run})", run = job.run);
        // A state change by the agent releases the ticket; one still held is one the agent never moved.
        let current = self.call("get ticket", || self.client.get_ticket(ticket)).await?;
        let held = current.assignee.as_deref() == Some(&self.id);
        let (comment, patch) = if outcome.timed_out {
            let body = format!("Run timed out after {}; leaving {} for another worker. Log: {log_link}", humantime::format_duration(job.run_timeout), current.state);
            (Some(body), held.then(|| UpdateTicket { assignee: Some(None), ..Default::default() }))
        } else if held {
            let body = format!("Agent finished without moving the ticket out of {}; marking it failed ({}). Log: {log_link}", current.state, outcome.summary);
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
        Ok(outcome.timed_out.then_some(ticket))
    }

    /// Creates the workspace and returns the agent's extra environment: orchestrator access, the project's repos
    /// (space-separated in `FACTORY_REPOS`), a git identity per worker so commits never fail on a missing author, and,
    /// when `GIT_TOKEN` is set, git and gh credentials (a `.git-credentials` store in the workspace). Git settings go
    /// in via `GIT_CONFIG_*`, leaving the image untouched.
    fn prepare(&self, ticket: i64, repos: &[String]) -> anyhow::Result<Vec<(String, String)>> {
        std::fs::create_dir_all(&self.workspace).with_context(|| format!("creating {}", self.workspace.display()))?;
        let mut env = vec![
            ("FACTORY_URL".to_string(), self.url.clone()),
            ("FACTORY_TOKEN".to_string(), self.token.clone()),
            ("FACTORY_WORKER_ID".to_string(), self.id.clone()),
            ("FACTORY_TICKET".to_string(), ticket.to_string()),
            ("FACTORY_REPOS".to_string(), repos.join(" ")),
        ];
        let mut git = vec![("user.name", format!("factory worker {}", self.id)), ("user.email", format!("{}@factory.invalid", self.id))];
        if let Ok(token) = std::env::var("GIT_TOKEN") {
            use std::os::unix::fs::PermissionsExt;
            let file = self.workspace.join(".git-credentials");
            std::fs::write(&file, format!("https://x-access-token:{token}@github.com\n"))?;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600))?;
            git.push(("credential.helper", format!("store --file={}", file.display())));
            env.push(("GH_TOKEN".to_string(), token));
        }
        env.push(("GIT_CONFIG_COUNT".to_string(), git.len().to_string()));
        for (i, (key, value)) in git.into_iter().enumerate() {
            env.push((format!("GIT_CONFIG_KEY_{i}"), key.to_string()));
            env.push((format!("GIT_CONFIG_VALUE_{i}"), value));
        }
        Ok(env)
    }
}
