use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::Context;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::log::Log;

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
    /// Runs the agent on `prompt` in `workspace` with `model` (the agent's own default when `None`), killing it after
    /// `timeout`. `env` is extra environment for the agent process: orchestrator URL and token, worker and ticket ids,
    /// git credentials. Every line the agent prints goes to `log`.
    async fn run(&self, prompt: &str, model: Option<&str>, workspace: &Path, timeout: Duration, env: &[(String, String)], log: &Log) -> anyhow::Result<(Outcome, Usage)>;
}

/// Runs a program with the prompt on stdin and in `PROMPT`, and the model, if any, in `MODEL`. Usage is an optional
/// JSON object on the last line of stdout: `{"tokens_in":..,"tokens_out":..,"cost":..}`.
pub struct CommandAdapter {
    pub command: String,
}

impl Adapter for CommandAdapter {
    async fn run(&self, prompt: &str, model: Option<&str>, workspace: &Path, timeout: Duration, env: &[(String, String)], log: &Log) -> anyhow::Result<(Outcome, Usage)> {
        let mut cmd = Command::new(&self.command);
        cmd.current_dir(workspace).env("PROMPT", prompt).envs(env.iter().map(|(k, v)| (k, v)));
        if let Some(model) = model {
            cmd.env("MODEL", model);
        }
        let (outcome, last) = run(cmd, prompt, timeout, log).await?;
        Ok((outcome, serde_json::from_str(&last).unwrap_or_default()))
    }
}

/// Runs the Claude Code CLI non-interactively, streaming one JSON event per line. It inherits the worker's
/// environment, so `CLAUDE_CODE_OAUTH_TOKEN` reaches it. Usage comes from the final `result` event; tokens in count
/// cache reads and writes.
pub struct ClaudeCodeAdapter {
    pub bin: String,
}

#[derive(Default, Deserialize)]
struct ClaudeResult {
    #[serde(default)]
    total_cost_usd: f64,
    #[serde(default)]
    usage: ClaudeUsage,
}

#[derive(Default, Deserialize)]
struct ClaudeUsage {
    #[serde(default)]
    input_tokens: i64,
    #[serde(default)]
    cache_creation_input_tokens: i64,
    #[serde(default)]
    cache_read_input_tokens: i64,
    #[serde(default)]
    output_tokens: i64,
}

impl Adapter for ClaudeCodeAdapter {
    async fn run(&self, prompt: &str, model: Option<&str>, workspace: &Path, timeout: Duration, env: &[(String, String)], log: &Log) -> anyhow::Result<(Outcome, Usage)> {
        let mut cmd = Command::new(&self.bin);
        cmd.args(["-p", "--output-format", "stream-json", "--verbose", "--dangerously-skip-permissions"]).current_dir(workspace).envs(env.iter().map(|(k, v)| (k, v)));
        if let Some(model) = model {
            cmd.args(["--model", model]);
        }
        let (outcome, last) = run(cmd, prompt, timeout, log).await?;
        let r: ClaudeResult = serde_json::from_str(&last).unwrap_or_default();
        let u = r.usage;
        let usage = Usage {
            tokens_in: u.input_tokens + u.cache_creation_input_tokens + u.cache_read_input_tokens,
            tokens_out: u.output_tokens,
            cost: r.total_cost_usd,
        };
        Ok((outcome, usage))
    }
}

/// Kills the process group on drop, so a timed-out or abandoned agent takes its children with it.
struct Group(u32);

impl Drop for Group {
    fn drop(&mut self) {
        unsafe { libc::kill(-(self.0 as i32), libc::SIGKILL) };
    }
}

/// Spawns `cmd` in its own process group with `prompt` on stdin, sends each stdout line to `log`, and kills it after
/// `timeout`. Returns the outcome and the last non-empty stdout line.
async fn run(mut cmd: Command, prompt: &str, timeout: Duration, log: &Log) -> anyhow::Result<(Outcome, String)> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .process_group(0)
        .spawn()
        .with_context(|| format!("spawning {:?}", cmd.as_std().get_program()))?;
    let _group = Group(child.id().unwrap());
    let mut stdin = child.stdin.take().unwrap();
    let prompt = prompt.to_owned();
    tokio::spawn(async move {
        let _ = stdin.write_all(prompt.as_bytes()).await;
    });
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut last = String::new();
    let wait = async {
        while let Some(line) = lines.next_line().await? {
            if !line.trim().is_empty() {
                last = line.clone();
            }
            log.line(line);
        }
        child.wait().await
    };
    let outcome = match tokio::time::timeout(timeout, wait).await {
        Ok(status) => {
            let status = status.context("waiting for the agent")?;
            Outcome { success: status.success(), timed_out: false, summary: format!("agent exited with {status}"), links: vec![] }
        }
        Err(_) => {
            unsafe { libc::kill(-(child.id().unwrap() as i32), libc::SIGKILL) };
            child.wait().await.context("waiting for the killed agent")?;
            Outcome { success: false, timed_out: true, summary: format!("agent killed after {}", humantime::format_duration(timeout)), links: vec![] }
        }
    };
    Ok((outcome, last))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real `claude -p` result event, trimmed.
    const RESULT: &str = r#"{"duration_api_ms":5065,"stop_reason":"end_turn","session_id":"d4ba97d1","total_cost_usd":0.218687,"usage":{"input_tokens":2,"cache_creation_input_tokens":10745,"cache_read_input_tokens":10480,"output_tokens":4,"output_tokens_details":{"thinking_tokens":0},"service_tier":"standard"},"is_error":false,"num_turns":1,"subtype":"success","result":"ok","type":"result"}"#;

    /// Shims are written once, before any test forks: a file open for writing in one thread makes exec fail with
    /// ETXTBSY in another.
    static SHIMS: std::sync::LazyLock<tempfile::TempDir> = std::sync::LazyLock::new(|| {
        let dir = tempfile::tempdir().unwrap();
        let scripts = [
            ("ok", format!("printf '%s' \"$*\" > argv.txt\ncat > stdin.txt\nprintf '%s|%s' \"$FACTORY_TICKET\" \"$CLAUDE_CODE_OAUTH_TOKEN\" > env.txt\necho '{RESULT}'\n")),
            ("bare", "echo '{\"type\":\"result\",\"result\":\"ok\"}'\nexit 1\n".to_string()),
            ("hang", "sleep 60 &\necho $! > child.pid\nwait\n".to_string()),
        ];
        for (name, body) in scripts {
            let path = dir.path().join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
            std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        }
        dir
    });

    fn shim(name: &str) -> ClaudeCodeAdapter {
        ClaudeCodeAdapter { bin: SHIMS.path().join(name).to_string_lossy().into_owned() }
    }

    #[tokio::test]
    async fn claude_code_passes_flags_prompt_and_env_and_parses_usage() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: tests here run single-threaded per process; the value is only read by the spawned child.
        unsafe { std::env::set_var("CLAUDE_CODE_OAUTH_TOKEN", "oauth-1") };
        let env = vec![("FACTORY_TICKET".to_string(), "7".to_string())];
        let (outcome, usage) = shim("ok").run("do #7", Some("opus"), dir.path(), Duration::from_secs(10), &env, &Log::stderr()).await.unwrap();
        assert!(outcome.success && !outcome.timed_out);
        assert_eq!(std::fs::read_to_string(dir.path().join("argv.txt")).unwrap(), "-p --output-format stream-json --verbose --dangerously-skip-permissions --model opus");
        assert_eq!(std::fs::read_to_string(dir.path().join("stdin.txt")).unwrap(), "do #7");
        assert_eq!(std::fs::read_to_string(dir.path().join("env.txt")).unwrap(), "7|oauth-1");
        assert_eq!((usage.tokens_in, usage.tokens_out, usage.cost), (2 + 10745 + 10480, 4, 0.218687));
    }

    #[tokio::test]
    async fn claude_code_tolerates_missing_usage() {
        let dir = tempfile::tempdir().unwrap();
        let (outcome, usage) = shim("bare").run("x", None, dir.path(), Duration::from_secs(10), &[], &Log::stderr()).await.unwrap();
        assert!(!outcome.success);
        assert_eq!((usage.tokens_in, usage.tokens_out, usage.cost), (0, 0, 0.0));
    }

    #[tokio::test]
    async fn claude_code_timeout_kills_the_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let (outcome, usage) = shim("hang").run("x", None, dir.path(), Duration::from_millis(300), &[], &Log::stderr()).await.unwrap();
        assert!(outcome.timed_out && !outcome.success);
        assert_eq!(usage.tokens_in, 0);
        let pid = std::fs::read_to_string(dir.path().join("child.pid")).unwrap().trim().to_string();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::process::Command::new("kill").args(["-0", &pid]).stderr(Stdio::null()).status().unwrap().success() {
            assert!(std::time::Instant::now() < deadline, "child still alive");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}
