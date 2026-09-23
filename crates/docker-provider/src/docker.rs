//! Thin wrapper over the `docker` CLI.

use std::collections::{BTreeMap, HashMap};
use std::process::Stdio;

use anyhow::{bail, Context};
use tokio::process::Command;

use crate::Running;

/// Label carrying the worker id on every container this provider starts.
pub const LABEL: &str = "software-factory.worker_id";
/// Label carrying the agent the container runs.
pub const AGENT_LABEL: &str = "software-factory.agent";

async fn output(mut cmd: Command) -> anyhow::Result<std::process::Output> {
    let args: Vec<String> = cmd.as_std().get_args().map(|a| a.to_string_lossy().into_owned()).collect();
    let out = cmd.stdin(Stdio::null()).output().await.context("running docker")?;
    tracing::debug!(status = %out.status, "docker {}", args.join(" "));
    Ok(out)
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).trim().to_string()
}

/// Whether every stderr line is a "no such container/object" complaint.
fn only_missing(out: &std::process::Output) -> bool {
    stderr(out).lines().all(|l| l.to_ascii_lowercase().contains("no such"))
}

/// Starts a detached container from `image` and returns its id.
/// Values are passed through the docker CLI's environment, not its arguments.
pub async fn run(image: &str, worker_id: &str, agent: &str, env: &BTreeMap<String, String>) -> anyhow::Result<String> {
    let mut cmd = Command::new("docker");
    cmd.args(["run", "-d", "--label", &format!("{LABEL}={worker_id}"), "--label", &format!("{AGENT_LABEL}={agent}")]);
    for (k, v) in env {
        cmd.arg("-e").arg(k).env(k, v);
    }
    cmd.arg(image);
    let out = output(cmd).await?;
    if !out.status.success() {
        bail!("docker run: {}", stderr(&out));
    }
    Ok(stdout(&out))
}

/// Stops and removes a container.
pub async fn remove(container_id: &str) -> anyhow::Result<()> {
    let mut cmd = Command::new("docker");
    cmd.args(["rm", "-f", container_id]);
    let out = output(cmd).await?;
    if !out.status.success() && !only_missing(&out) {
        bail!("docker rm: {}", stderr(&out));
    }
    Ok(())
}

/// Docker's state (`running`, `exited`, ...) of each container by id.
/// Containers Docker no longer knows are absent from the result.
pub async fn states(ids: impl Iterator<Item = &str>) -> anyhow::Result<HashMap<String, String>> {
    let ids: Vec<&str> = ids.collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut cmd = Command::new("docker");
    cmd.args(["inspect", "--format", "{{.Id}} {{.State.Status}}"]).args(ids);
    let out = output(cmd).await?;
    if !out.status.success() && !only_missing(&out) {
        bail!("docker inspect: {}", stderr(&out));
    }
    Ok(stdout(&out)
        .lines()
        .filter_map(|l| l.split_once(' '))
        .map(|(id, state)| (id.to_string(), state.to_string()))
        .collect())
}

/// Every container this provider ever started (by label), by worker id.
pub async fn tracked() -> anyhow::Result<BTreeMap<String, Running>> {
    let mut cmd = Command::new("docker");
    cmd.args(["ps", "-a", "--no-trunc", "--filter", &format!("label={LABEL}")]);
    cmd.args(["--format", &format!("{{{{.ID}}}} {{{{.Label \"{LABEL}\"}}}} {{{{.Label \"{AGENT_LABEL}\"}}}}")]);
    let out = output(cmd).await?;
    if !out.status.success() {
        bail!("docker ps: {}", stderr(&out));
    }
    Ok(stdout(&out)
        .lines()
        .filter_map(|l| {
            let mut parts = l.splitn(3, ' ');
            Some((parts.next()?, parts.next()?, parts.next().unwrap_or_default()))
        })
        .map(|(id, worker, agent)| (worker.to_string(), Running { container_id: id.to_string(), agent: agent.to_string() }))
        .collect())
}
