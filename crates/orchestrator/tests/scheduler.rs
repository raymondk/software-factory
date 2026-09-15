use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use orchestrator::provider::{AgentInfo, Providers, ProviderWorker, StartWorker, Status};
use orchestrator::{api, config::Config, db, scheduler};
use sqlx::SqlitePool;

const CONFIG: &str = r#"
[project]
name = "test"
repos = []
[orchestrator]
listen = "127.0.0.1:8080"
token = "x"
heartbeat_timeout = "60s"
[scheduler]
max_workers = 3
[providers.a]
url = "unused"
[providers.b]
url = "unused"
[agents.claude-code]
run_timeout = "1h"
[agents.codex]
run_timeout = "1h"
[prompts]
ready = "work"
in_review = "review"
"#;

/// In-process provider: records starts and stops, lists what it runs, enforces `capacity`, advertises `agents`.
#[derive(Default)]
struct Fake {
    capacity: u32,
    agents: BTreeMap<String, AgentInfo>,
    /// (worker id, agent)
    running: Vec<(String, String)>,
    starts: Vec<StartWorker>,
    stops: Vec<String>,
    fail_start: bool,
}

type Shared = Arc<Mutex<Fake>>;

fn agent(models: &[&str]) -> AgentInfo {
    AgentInfo { models: models.iter().map(|m| m.to_string()).collect(), default_model: models[0].into() }
}

/// Both agents, one model each.
fn both() -> BTreeMap<String, AgentInfo> {
    BTreeMap::from([("claude-code".to_string(), agent(&["m"])), ("codex".to_string(), agent(&["m"]))])
}

async fn serve_fake(fake: Fake) -> (String, Shared) {
    let fake = Arc::new(Mutex::new(fake));
    let router = Router::new()
        .route("/workers", post(|State(f): State<Shared>, Json(req): Json<StartWorker>| async move {
            let mut f = f.lock().unwrap();
            if f.fail_start {
                return StatusCode::INTERNAL_SERVER_ERROR;
            }
            f.running.push((req.worker_id.clone(), req.agent.clone()));
            f.starts.push(req);
            StatusCode::CREATED
        }))
        .route("/workers/{id}", delete(|State(f): State<Shared>, Path(id): Path<String>| async move {
            let mut f = f.lock().unwrap();
            f.stops.push(id.clone());
            let before = f.running.len();
            f.running.retain(|(w, _)| w != &id);
            if f.running.len() < before { StatusCode::NO_CONTENT } else { StatusCode::NOT_FOUND }
        }))
        .route("/status", get(|State(f): State<Shared>| async move {
            let f = f.lock().unwrap();
            let workers = f.running.iter().map(|(w, a)| ProviderWorker { worker_id: w.clone(), agent: a.clone(), status: "running".into() }).collect();
            Json(Status { capacity: f.capacity, in_use: f.running.len() as u32, agents: f.agents.clone(), workers })
        }))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, fake)
}

struct Setup {
    pool: SqlitePool,
    config: Config,
    providers: Providers,
    fakes: BTreeMap<&'static str, Shared>,
    _dir: tempfile::TempDir,
}

/// Fake providers named as in `CONFIG` (`a`, `b`); a name absent from `fakes` points at a dead port.
async fn setup_with(fakes: Vec<(&'static str, Fake)>) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::open(&dir.path().join("test.db")).await.unwrap();
    let mut config = Config::parse(CONFIG).unwrap();
    let mut shared = BTreeMap::new();
    for p in config.providers.values_mut() {
        p.url = "http://127.0.0.1:1".into();
    }
    for (name, fake) in fakes {
        let (url, f) = serve_fake(fake).await;
        config.providers.get_mut(name).unwrap().url = url;
        shared.insert(name, f);
    }
    let providers = Providers::new(&config);
    Setup { pool, config, providers, fakes: shared, _dir: dir }
}

/// Provider `a` with `capacity` advertising both agents; `b` full, so `a` always wins.
async fn setup(capacity: u32) -> Setup {
    setup_with(vec![("a", Fake { capacity, agents: both(), ..Default::default() }), ("b", Fake { capacity: 0, agents: both(), ..Default::default() })]).await
}

impl Setup {
    async fn tick(&self) -> anyhow::Result<()> {
        scheduler::tick(&self.pool, &self.config, &self.providers).await
    }

    fn fake(&self, name: &str) -> std::sync::MutexGuard<'_, Fake> {
        self.fakes[name].lock().unwrap()
    }

    /// A worker record of `agent` on provider `a` in `status`, also running at the provider unless `listed` is false.
    async fn worker(&self, agent: &str, status: &str, listed: bool) -> String {
        self.worker_on("a", agent, status, listed).await
    }

    async fn worker_on(&self, provider: &'static str, agent: &str, status: &str, listed: bool) -> String {
        let w = api::new_worker(&self.pool, &self.config, agent, provider).await.ok().unwrap();
        sqlx::query("UPDATE workers SET status = ?2 WHERE id = ?1").bind(&w.id).bind(status).execute(&self.pool).await.unwrap();
        if listed {
            self.fake(provider).running.push((w.id.clone(), agent.to_string()));
        }
        w.id
    }

    /// (id, agent, provider, status) of every worker record.
    async fn workers(&self) -> Vec<(String, String, String, String)> {
        sqlx::query_as("SELECT id, agent, provider, status FROM workers ORDER BY id").fetch_all(&self.pool).await.unwrap()
    }

    async fn status_of(&self, id: &str) -> String {
        let (s,): (String,) = sqlx::query_as("SELECT status FROM workers WHERE id = ?1").bind(id).fetch_one(&self.pool).await.unwrap();
        s
    }
}

async fn ticket(pool: &SqlitePool, state: &str, assignee: Option<&str>) {
    pinned(pool, state, assignee, None, None).await
}

async fn pinned(pool: &SqlitePool, state: &str, assignee: Option<&str>, agent: Option<&str>, model: Option<&str>) {
    sqlx::query("INSERT INTO tickets (title, description, state, rank, assignee, created_at, updated_at, agent, model) VALUES ('t', '', ?1, 1, ?2, 'now', 'now', ?3, ?4)")
        .bind(state)
        .bind(assignee)
        .bind(agent)
        .bind(model)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn starts_one_worker_per_ticket_up_to_max_workers() {
    let s = setup(10).await;
    for _ in 0..2 {
        ticket(&s.pool, "ready", None).await;
    }
    ticket(&s.pool, "in_review", None).await;
    ticket(&s.pool, "todo", None).await; // no prompt for todo
    ticket(&s.pool, "ready", Some("w-busy")).await; // taken

    s.tick().await.unwrap();
    // Prompts are shared, so the pool runs the first agent the provider advertises.
    let workers = s.workers().await;
    assert_eq!(workers.iter().filter(|(_, a, p, st)| a == "claude-code" && p == "a" && st == "starting").count(), 3);
    let starts = s.fake("a").starts.clone();
    assert_eq!(starts.len(), 3);
    let ids: Vec<&String> = workers.iter().map(|(id, ..)| id).collect();
    for st in &starts {
        assert!(ids.contains(&&st.worker_id));
        assert_eq!(st.agent, "claude-code");
        assert_eq!(st.orchestrator_url, "http://127.0.0.1:8080");
        assert!(!st.worker_token.is_empty());
    }
    assert!(s.fake("b").starts.is_empty());

    // Another ticket: max_workers = 3 is reached, nothing more starts.
    ticket(&s.pool, "ready", None).await;
    s.tick().await.unwrap();
    assert_eq!(s.workers().await.len(), 3);
    assert_eq!(s.fake("a").starts.len(), 3);
}

#[tokio::test]
async fn blocked_tickets_do_not_start_workers() {
    let s = setup(10).await;
    ticket(&s.pool, "ready", None).await; // id 1: blocked by 2
    ticket(&s.pool, "todo", None).await; // id 2
    sqlx::query("INSERT INTO ticket_relations (from_id, type, to_id) VALUES (1, 'depends_on', 2)").execute(&s.pool).await.unwrap();

    s.tick().await.unwrap();
    assert!(s.workers().await.is_empty());
    assert!(s.fake("a").starts.is_empty());

    sqlx::query("UPDATE tickets SET state = 'done' WHERE id = 2").execute(&s.pool).await.unwrap();
    s.tick().await.unwrap();
    assert_eq!(s.workers().await.len(), 1);
}

#[tokio::test]
async fn provider_capacity_caps_starts() {
    let s = setup(1).await;
    for _ in 0..3 {
        ticket(&s.pool, "ready", None).await;
    }
    s.tick().await.unwrap();
    assert_eq!(s.fake("a").starts.len(), 1);
    assert_eq!(s.workers().await.len(), 1);
    s.tick().await.unwrap();
    assert_eq!(s.fake("a").starts.len(), 1);
}

#[tokio::test]
async fn pending_workers_count_toward_wanted() {
    let s = setup(10).await;
    for _ in 0..3 {
        ticket(&s.pool, "ready", None).await;
    }
    s.worker("claude-code", "starting", true).await;
    s.worker("claude-code", "idle", true).await;
    s.tick().await.unwrap();
    assert_eq!(s.fake("a").starts.len(), 1);
    assert_eq!(s.workers().await.len(), 3);
    s.tick().await.unwrap();
    assert_eq!(s.fake("a").starts.len(), 1);
}

#[tokio::test]
async fn pinned_tickets_start_workers_of_their_agent_and_spare_workers_drain_the_pool() {
    let s = setup(10).await;
    pinned(&s.pool, "ready", None, Some("codex"), None).await;
    pinned(&s.pool, "ready", None, Some("codex"), None).await;
    pinned(&s.pool, "ready", None, Some("nope"), None).await; // advertised by no one: never scheduled
    ticket(&s.pool, "ready", None).await; // pool: covered by the idle claude-code worker
    let idle = s.worker("claude-code", "idle", true).await;
    s.tick().await.unwrap();
    assert_eq!(s.fake("a").starts.iter().map(|x| x.agent.as_str()).collect::<Vec<_>>(), ["codex", "codex"]);
    assert_eq!(s.status_of(&idle).await, "idle");
    assert!(s.fake("a").stops.is_empty());

    // The pool is empty and claude-code has no pinned work: its idle worker stops; codex keeps its pending ones.
    sqlx::query("UPDATE tickets SET assignee = 'someone' WHERE agent IS NULL").execute(&s.pool).await.unwrap();
    s.tick().await.unwrap();
    assert_eq!(s.fake("a").stops, vec![idle.clone()]);
    assert_eq!(s.fake("a").starts.len(), 2);
}

#[tokio::test]
async fn advertised_but_unconfigured_agent_is_never_scheduled() {
    let mut agents = both();
    agents.insert("pi".into(), agent(&["m"]));
    let s = setup_with(vec![("a", Fake { capacity: 10, agents, ..Default::default() })]).await;
    pinned(&s.pool, "ready", None, Some("pi"), None).await;
    s.tick().await.unwrap();
    assert!(s.fake("a").starts.is_empty());
    assert!(s.workers().await.is_empty());
}

#[tokio::test]
async fn routes_to_the_provider_with_most_free_capacity_that_advertises_agent_and_model() {
    // a: claude-code with sonnet, plenty of room. b: claude-code with sonnet and opus, codex with o3, room for three.
    let a = BTreeMap::from([("claude-code".to_string(), agent(&["sonnet"]))]);
    let b = BTreeMap::from([("claude-code".to_string(), agent(&["sonnet", "opus"])), ("codex".to_string(), agent(&["o3"]))]);
    let s = setup_with(vec![("a", Fake { capacity: 5, agents: a, ..Default::default() }), ("b", Fake { capacity: 3, agents: b, ..Default::default() })]).await;
    let mut s = s;
    s.config.scheduler.max_workers = 10;
    // An idle claude-code worker on a cannot serve opus, so the opus ticket still starts one on b.
    let idle = s.worker_on("a", "claude-code", "idle", true).await;
    ticket(&s.pool, "ready", None).await; // pool: the idle worker takes it
    ticket(&s.pool, "ready", None).await; // pool: a has the most free capacity
    pinned(&s.pool, "ready", None, Some("claude-code"), Some("opus")).await; // only b
    pinned(&s.pool, "ready", None, Some("codex"), None).await; // only b
    pinned(&s.pool, "ready", None, Some("codex"), Some("gpt")).await; // nobody
    pinned(&s.pool, "ready", None, None, Some("o3")).await; // pool with a model: b, as codex

    s.tick().await.unwrap();
    assert_eq!(s.fake("a").starts.iter().map(|x| x.agent.as_str()).collect::<Vec<_>>(), ["claude-code"]);
    let mut on_b: Vec<String> = s.fake("b").starts.iter().map(|x| x.agent.clone()).collect();
    on_b.sort();
    assert_eq!(on_b, ["claude-code", "codex", "codex"]);
    assert_eq!(s.status_of(&idle).await, "idle");
    let mut recorded: Vec<(String, String)> = s.workers().await.into_iter().filter(|(.., st)| st == "starting").map(|(_, a, p, _)| (a, p)).collect();
    recorded.sort();
    assert_eq!(recorded, [("claude-code".into(), "a".into()), ("claude-code".into(), "b".into()), ("codex".into(), "b".into()), ("codex".into(), "b".into())]);

    // Nothing else fits: b is full, the gpt ticket has no taker.
    s.tick().await.unwrap();
    assert_eq!(s.fake("a").starts.len() + s.fake("b").starts.len(), 4);
}

#[tokio::test]
async fn unreachable_provider_is_skipped_for_the_pass() {
    let s = setup_with(vec![("b", Fake { capacity: 10, agents: both(), ..Default::default() })]).await;
    ticket(&s.pool, "ready", None).await;
    s.tick().await.unwrap();
    let workers = s.workers().await;
    assert_eq!((workers.len(), workers[0].2.as_str()), (1, "b"));
    assert_eq!(s.fake("b").starts.len(), 1);
}

#[tokio::test]
async fn stops_idle_workers_when_the_queue_is_empty_but_not_busy_ones() {
    let s = setup(10).await;
    let idle = s.worker("claude-code", "idle", true).await;
    let busy = s.worker("claude-code", "busy", true).await;
    let starting = s.worker("claude-code", "starting", true).await;
    let codex = s.worker("codex", "idle", true).await;
    ticket(&s.pool, "in_review", None).await; // work for anyone: nothing is stopped, and a pending worker covers it
    s.tick().await.unwrap();
    assert!(s.fake("a").stops.is_empty());
    assert!(s.fake("a").starts.is_empty());
    assert_eq!(s.status_of(&codex).await, "idle");

    sqlx::query("UPDATE tickets SET assignee = 'someone'").execute(&s.pool).await.unwrap();
    s.tick().await.unwrap();
    let mut stops = s.fake("a").stops.clone();
    stops.sort();
    let mut expected = vec![idle.clone(), codex.clone()];
    expected.sort();
    assert_eq!(stops, expected);
    assert_eq!(s.status_of(&idle).await, "dead");
    assert_eq!(s.status_of(&codex).await, "dead");
    assert_eq!(s.status_of(&busy).await, "busy");
    assert_eq!(s.status_of(&starting).await, "starting");
    assert!(s.fake("a").starts.is_empty());
}

#[tokio::test]
async fn stops_route_to_the_workers_provider() {
    let s = setup_with(vec![("a", Fake { capacity: 10, agents: both(), ..Default::default() }), ("b", Fake { capacity: 10, agents: both(), ..Default::default() })]).await;
    let idle_b = s.worker_on("b", "claude-code", "idle", true).await;
    let dead_a = s.worker_on("a", "claude-code", "dead", true).await;
    s.fake("b").running.push(("w-unknown".into(), "codex".into()));
    s.tick().await.unwrap();
    assert_eq!(s.fake("a").stops, vec![dead_a.clone()]);
    let mut stops_b = s.fake("b").stops.clone();
    stops_b.sort();
    let mut expected = vec!["w-unknown".to_string(), idle_b.clone()];
    expected.sort();
    assert_eq!(stops_b, expected);
    assert_eq!(s.status_of(&idle_b).await, "dead");
}

#[tokio::test]
async fn dead_workers_still_listed_by_provider_are_stopped() {
    let s = setup(10).await;
    let reaped = s.worker("claude-code", "dead", true).await;
    let gone = s.worker("claude-code", "dead", false).await;
    s.tick().await.unwrap();
    assert_eq!(s.fake("a").stops, vec![reaped.clone()]);
    assert!(s.fake("a").running.is_empty());
    s.tick().await.unwrap();
    assert_eq!(s.fake("a").stops.len(), 1, "{gone} was never listed and {reaped} is gone now");
}

#[tokio::test]
async fn unknown_provider_workers_are_stopped() {
    let s = setup(10).await;
    let starting = s.worker("claude-code", "starting", true).await;
    let idle = s.worker("claude-code", "idle", true).await;
    let busy = s.worker("claude-code", "busy", true).await;
    ticket(&s.pool, "ready", None).await; // idle still has work
    s.fake("a").running.push(("w-unknown".into(), "claude-code".into()));
    s.tick().await.unwrap();
    assert_eq!(s.fake("a").stops, vec!["w-unknown".to_string()]);
    assert_eq!(s.fake("a").running.iter().map(|(w, _)| w.clone()).collect::<Vec<_>>(), vec![starting.clone(), idle.clone(), busy.clone()]);
    assert_eq!(s.status_of(&starting).await, "starting");
    assert_eq!(s.status_of(&idle).await, "idle");
    assert_eq!(s.status_of(&busy).await, "busy");
    assert!(s.fake("a").starts.is_empty());
}

#[tokio::test]
async fn failed_start_marks_worker_dead() {
    let s = setup_with(vec![("a", Fake { capacity: 10, agents: both(), fail_start: true, ..Default::default() })]).await;
    ticket(&s.pool, "ready", None).await;
    s.tick().await.unwrap();
    let workers = s.workers().await;
    assert_eq!(workers.len(), 1);
    assert_eq!(workers[0].3, "dead");
    assert!(s.fake("a").starts.is_empty());
}

#[tokio::test]
async fn no_provider_answering_is_an_error_not_a_panic() {
    let s = setup_with(vec![]).await;
    ticket(&s.pool, "ready", None).await;
    let err = s.tick().await.unwrap_err();
    assert!(err.to_string().contains("no provider answered"), "{err:#}");
    assert!(s.workers().await.is_empty());
}
