//! Drives the worker binary against an in-process orchestrator with fake agent shell scripts.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Arc;
use std::time::{Duration, Instant};

use api_client::{Client, CreateTicket, Ticket, UpdateTicket};
use orchestrator::{api, config::Config, db, reaper, AppState};

const TOKEN: &str = "secret";
const CONFIG: &str = r#"
[project]
name = "test"
repos = ["https://github.com/org/a.git"]
[orchestrator]
listen = "127.0.0.1:0"
token = "x"
heartbeat_timeout = "60s"
[scheduler]
max_workers = 4
[provider]
url = "http://localhost:8081"
[agents.command]
run_timeout = "1s"
[prompts]
ready = "Work on #{{ticket.id}}: {{ticket.title}}"
in_progress = "Resume #{{ticket.id}}"
"#;

/// Shell prelude: `patch '{...}'` updates the ticket the worker handed us.
const PRELUDE: &str = r#"#!/bin/sh
set -e
patch() {
  curl -sf -X PATCH -H "Authorization: Bearer $FACTORY_TOKEN" -H 'content-type: application/json' -d "$1" "$FACTORY_URL/tickets/$FACTORY_TICKET" > /dev/null
}
"#;

struct Fixture {
    url: String,
    dir: tempfile::TempDir,
    human: Client,
}

/// A worker process, killed on drop so a failing test leaves nothing behind.
struct Proc(Child);

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Fixture {
    async fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::open(&dir.path().join("test.db")).await.unwrap();
        let mut config = Config::parse(CONFIG).unwrap();
        config.orchestrator.token = TOKEN.into();
        tokio::spawn(reaper::run(pool.clone(), config.orchestrator.heartbeat_timeout));
        let router = api::router(AppState { pool, config: Arc::new(config) });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Fixture { human: Client::new(&url, TOKEN), url, dir }
    }

    async fn ticket(&self, title: &str) -> i64 {
        self.ticket_with_model(title, None).await
    }

    async fn ticket_with_model(&self, title: &str, model: Option<&str>) -> i64 {
        let t = self.human.create_ticket(&CreateTicket { title: title.into(), model: model.map(str::to_owned), ..Default::default() }).await.unwrap();
        self.human.update_ticket(t.id, &UpdateTicket { state: Some("ready".into()), ..Default::default() }).await.unwrap();
        t.id
    }

    /// Starts the worker binary with `script` (appended to `PRELUDE`) as its agent. Returns the process, worker id, and workspace.
    async fn worker(&self, script: &str) -> (Proc, String, PathBuf) {
        let w = self.human.create_worker(&api_client::CreateWorker { agent: "command".into() }).await.unwrap();
        let agent = self.dir.path().join(format!("agent-{}.sh", w.id));
        std::fs::write(&agent, format!("{PRELUDE}{script}")).unwrap();
        std::fs::set_permissions(&agent, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let workspace = self.dir.path().join(format!("ws-{}", w.id));
        let child = Command::new(env!("CARGO_BIN_EXE_worker"))
            .env("FACTORY_URL", &self.url)
            .env("FACTORY_WORKER_ID", &w.id)
            .env("FACTORY_WORKER_TOKEN", &w.token)
            .env("FACTORY_AGENT", "command")
            .env("FACTORY_MODEL", "m-default")
            .env("FACTORY_WORKSPACE", &workspace)
            .env("FACTORY_AGENT_COMMAND", &agent)
            .env("FACTORY_POLL_INTERVAL", "100ms")
            .env("FACTORY_HEARTBEAT_INTERVAL", "300ms")
            .env("FACTORY_LOG_INTERVAL", "100ms")
            .env("GIT_TOKEN", "t0k")
            .spawn()
            .unwrap();
        (Proc(child), w.id, workspace)
    }

    async fn wait_for(&self, id: i64, ok: impl Fn(&Ticket) -> bool) -> Ticket {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let t = self.human.get_ticket(id).await.unwrap();
            if ok(&t) {
                return t;
            }
            assert!(Instant::now() < deadline, "timed out waiting on ticket #{id}: {t:?}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn status(&self, worker: &str) -> String {
        self.human.list_workers().await.unwrap().into_iter().find(|w| w.id == worker).unwrap().status
    }
}

/// SIGTERM: the worker exits cleanly.
async fn stop(mut child: Proc) {
    Command::new("kill").args(["-TERM", &child.0.id().to_string()]).status().unwrap();
    assert!(wait(&mut child).await.success());
}

// Async: the orchestrator shares this thread's runtime, so blocking here would starve the worker.
async fn wait(child: &mut Proc) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < deadline, "worker did not exit");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn alive(pid_file: &Path) -> bool {
    let pid = std::fs::read_to_string(pid_file).unwrap().trim().to_string();
    Command::new("kill").args(["-0", &pid]).stderr(std::process::Stdio::null()).status().unwrap().success()
}

#[tokio::test]
async fn success_moves_on_and_reports_usage() {
    let f = Fixture::new().await;
    let id = f.ticket("hello").await;
    let (child, wid, ws) = f
        .worker(
            r#"cat > stdin.txt
printf '%s' "$PROMPT" > prompt.txt
printf '%s' "$GIT_CONFIG_VALUE_0" > git.txt
printf '%s' "$FACTORY_REPOS" > repos.txt
printf '%s' "$MODEL" > model.txt
patch '{"state":"in_review"}'
echo working
echo '{"tokens_in":100,"tokens_out":20,"cost":0.25}'
"#,
        )
        .await;
    let t = f.wait_for(id, |t| t.state == "in_review").await;
    assert_eq!(t.assignee, None);
    assert!(t.comments.is_empty());
    // One run, ended by the usage report; the worker's and the agent's lines are shipped, run lines tagged with it.
    let t = f.wait_for(id, |t| t.runs.first().is_some_and(|r| r.ended_at.is_some())).await;
    assert_eq!((t.runs.len(), t.runs[0].worker_id.as_str(), t.runs[0].ticket_id), (1, wid.as_str(), id));
    // No model on the ticket: the provider's default from FACTORY_MODEL reaches the agent and is recorded on the run.
    assert_eq!(t.runs[0].model.as_deref(), Some("m-default"));
    assert_eq!(std::fs::read_to_string(ws.join("model.txt")).unwrap(), "m-default");
    let run = t.runs[0].id;
    let deadline = Instant::now() + Duration::from_secs(10);
    let lines = loop {
        let lines = f.human.worker_logs(&wid, None).await.unwrap();
        if lines.iter().any(|l| l.line.starts_with("working") && l.run_id == Some(run)) {
            break lines;
        }
        assert!(Instant::now() < deadline, "agent output never shipped: {lines:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(lines[0].line.starts_with(&format!("worker {wid} (agent command) starting")) && lines[0].run_id.is_none(), "{lines:?}");
    assert!(lines.iter().any(|l| l.line.contains(&format!("ticket #{id} (ready), run {run}, model m-default")) && l.run_id == Some(run)));
    let of_run = f.human.run_logs(run, None).await.unwrap();
    assert!(of_run.iter().all(|l| l.run_id == Some(run)) && of_run.iter().any(|l| l.line == "working"));
    assert!(f.human.run_logs(run, Some(of_run.last().unwrap().id)).await.unwrap().is_empty());
    let prompt = format!("Work on #{id}: hello");
    assert_eq!(std::fs::read_to_string(ws.join("stdin.txt")).unwrap(), prompt);
    assert_eq!(std::fs::read_to_string(ws.join("prompt.txt")).unwrap(), prompt);
    assert_eq!(std::fs::read_to_string(ws.join("git.txt")).unwrap(), format!("store --file={}", ws.join(".git-credentials").display()));
    assert_eq!(std::fs::read_to_string(ws.join(".git-credentials")).unwrap(), "https://x-access-token:t0k@github.com\n");
    assert_eq!(std::fs::read_to_string(ws.join("repos.txt")).unwrap(), "https://github.com/org/a.git");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let m = f.human.metrics().await.unwrap();
        if let Some(b) = m.per_ticket.iter().find(|b| b.key.ticket_id == id) {
            assert_eq!((b.totals.tokens_in, b.totals.tokens_out, b.totals.cost), (100, 20, 0.25));
            assert_eq!(m.per_worker[0].key.worker_id, wid);
            assert_eq!(m.per_model[0].key.model.as_deref(), Some("m-default"));
            break;
        }
        assert!(Instant::now() < deadline, "usage never reported");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Back to polling: idle, and the next ticket flows too.
    let deadline = Instant::now() + Duration::from_secs(10);
    while f.status(&wid).await != "idle" {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // A ticket with a model: that one wins over the default.
    let second = f.ticket_with_model("again", Some("m-2")).await;
    let t = f.wait_for(second, |t| t.state == "in_review" && t.runs.first().is_some_and(|r| r.ended_at.is_some())).await;
    assert_eq!(t.runs[0].model.as_deref(), Some("m-2"));
    assert_eq!(std::fs::read_to_string(ws.join("model.txt")).unwrap(), "m-2");
    stop(child).await;
    // The last line is flushed before exit; it belongs to no run.
    let last = f.human.worker_logs(&wid, None).await.unwrap().pop().unwrap();
    assert_eq!((last.line.as_str(), last.run_id), (format!("worker {wid}: stopping").as_str(), None));
}

#[tokio::test]
async fn timeout_kills_agent_and_releases_ticket() {
    let f = Fixture::new().await;
    let id = f.ticket("slow").await;
    // First run: claim the ticket and hang. The worker times out, releases it, and resumes it; the second run finishes.
    let (child, wid, ws) = f
        .worker(
            r#"if [ -e self.pid ]; then patch '{"state":"done"}'; exit 0; fi
patch "{\"state\":\"in_progress\",\"assignee\":\"$FACTORY_WORKER_ID\"}"
sleep 60 &
echo $! > child.pid
echo $$ > self.pid
wait
"#,
        )
        .await;
    let t = f.wait_for(id, |t| t.state == "done").await;
    assert_eq!(t.comments.len(), 1);
    let first_run = t.runs.last().unwrap().id; // newest first
    assert_eq!(t.comments[0].author, wid);
    assert_eq!(t.comments[0].body, format!("Run timed out after 1s; leaving in_progress for another worker. Log: [run {first_run}](#/tickets/{id}/runs/{first_run})"));
    assert_eq!(t.runs.len(), 2);
    assert!(t.runs.iter().all(|r| r.ended_at.is_some()));
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(&ws.join("self.pid")) || alive(&ws.join("child.pid")) {
        assert!(Instant::now() < deadline, "agent process group still alive");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let m = f.human.metrics().await.unwrap();
    assert_eq!(m.per_ticket.iter().find(|b| b.key.ticket_id == id).map(|b| b.totals.tokens_in), Some(0));
    stop(child).await;
}

#[tokio::test]
async fn timed_out_ticket_is_not_resumed_while_other_work_exists() {
    let f = Fixture::new().await;
    let slow = f.ticket("slow").await;
    let quick = f.ticket("quick").await;
    // Hangs on `slow`, finishes anything else; records the order tickets were run in.
    let (child, _, ws) = f
        .worker(&format!(
            r#"echo $FACTORY_TICKET >> order.txt
if [ "$FACTORY_TICKET" = {slow} ]; then sleep 60; fi
patch '{{"state":"done"}}'
"#
        ))
        .await;
    f.wait_for(quick, |t| t.state == "done").await;
    let order = std::fs::read_to_string(ws.join("order.txt")).unwrap();
    let runs: Vec<&str> = order.lines().collect();
    assert_eq!(&runs[..2], [slow.to_string().as_str(), quick.to_string().as_str()], "after timing out on slow the worker takes quick");
    stop(child).await;
}

#[tokio::test]
async fn left_in_progress_is_failed() {
    let f = Fixture::new().await;
    let id = f.ticket("lazy").await;
    let (child, wid, _) = f.worker("patch \"{\\\"state\\\":\\\"in_progress\\\",\\\"assignee\\\":\\\"$FACTORY_WORKER_ID\\\"}\"\nexit 3\n").await;
    let t = f.wait_for(id, |t| t.state == "failed").await;
    assert_eq!(t.assignee, None);
    assert_eq!(t.comments.len(), 1);
    assert_eq!(t.comments[0].author, wid);
    assert_eq!(
        t.comments[0].body,
        format!("Agent finished without moving the ticket out of in_progress; marking it failed (agent exited with exit status: 3). Log: [run {run}](#/tickets/{id}/runs/{run})", run = t.runs[0].id)
    );
    stop(child).await;
}

#[tokio::test]
async fn left_in_progress_without_explicit_assignee_is_failed() {
    let f = Fixture::new().await;
    let id = f.ticket("lazy").await;
    let (child, wid, _) = f.worker("patch '{\"state\":\"in_progress\"}'\nexit 0\n").await;
    let t = f.wait_for(id, |t| t.state == "failed").await;
    assert_eq!(t.assignee, None);
    assert_eq!(t.comments.len(), 1);
    assert_eq!(t.comments[0].author, wid);
    assert_eq!(
        t.comments[0].body,
        format!("Agent finished without moving the ticket out of in_progress; marking it failed (agent exited with exit status: 0). Log: [run {run}](#/tickets/{id}/runs/{run})", run = t.runs[0].id)
    );
    stop(child).await;
}

#[tokio::test]
async fn exits_when_token_stops_authenticating() {
    let f = Fixture::new().await;
    let (mut child, wid, _) = f.worker("exit 0\n").await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while f.status(&wid).await != "idle" {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let pool = db::open(&f.dir.path().join("test.db")).await.unwrap();
    while !reaper::reap(&pool, Duration::ZERO).await.unwrap().contains(&wid) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!wait(&mut child).await.success(), "a reaped worker exits non-zero");
}
