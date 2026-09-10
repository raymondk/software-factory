use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::Context;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// How a run ended. The worker learns the real result from the ticket's state, not from here.
#[derive(Debug)]
pub struct Outcome {
    pub success: bool,
    /// Killed for exceeding its timeout.
    pub timed_out: bool,
    pub summary: String,
    pub links: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub tokens_in: i64,
    #[serde(default)]
    pub tokens_out: i64,
    #[serde(default)]
    pub cost: f64,
}

pub trait Adapter {
    /// Runs the agent on `prompt` in `workspace`, killing it after `timeout`. `env` is extra environment for the agent
    /// process: orchestrator URL and token, worker and ticket ids, git credentials.
    async fn run(&self, prompt: &str, workspace: &Path, timeout: Duration, env: &[(String, String)]) -> anyhow::Result<(Outcome, Usage)>;
}

/// Runs a program with the prompt on stdin and in `PROMPT`. Usage is an optional JSON object on the last line of
/// stdout: `{"tokens_in":..,"tokens_out":..,"cost":..}`.
pub struct CommandAdapter {
    pub command: String,
}

/// Kills the process group on drop, so a timed-out or abandoned agent takes its children with it.
struct Group(u32);

impl Drop for Group {
    fn drop(&mut self) {
        unsafe { libc::kill(-(self.0 as i32), libc::SIGKILL) };
    }
}

impl Adapter for CommandAdapter {
    async fn run(&self, prompt: &str, workspace: &Path, timeout: Duration, env: &[(String, String)]) -> anyhow::Result<(Outcome, Usage)> {
        let mut child = tokio::process::Command::new(&self.command)
            .current_dir(workspace)
            .env("PROMPT", prompt)
            .envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .process_group(0)
            .spawn()
            .with_context(|| format!("spawning {}", self.command))?;
        let _group = Group(child.id().unwrap());
        let mut stdin = child.stdin.take().unwrap();
        let prompt = prompt.to_owned();
        tokio::spawn(async move {
            let _ = stdin.write_all(prompt.as_bytes()).await;
        });
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut last = String::new();
        let run = async {
            while let Some(line) = lines.next_line().await? {
                eprintln!("agent: {line}");
                if !line.trim().is_empty() {
                    last = line;
                }
            }
            child.wait().await
        };
        let (success, timed_out, summary) = match tokio::time::timeout(timeout, run).await {
            Ok(status) => {
                let status = status.context("waiting for the agent")?;
                (status.success(), false, format!("agent exited with {status}"))
            }
            Err(_) => {
                unsafe { libc::kill(-(child.id().unwrap() as i32), libc::SIGKILL) };
                child.wait().await.context("waiting for the killed agent")?;
                (false, true, format!("agent killed after {}", humantime::format_duration(timeout)))
            }
        };
        let usage = serde_json::from_str(&last).unwrap_or_default();
        Ok((Outcome { success, timed_out, summary, links: vec![] }, usage))
    }
}
