use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use orchestrator::provider::{Provider, ProviderWorker, StartWorker, Status};
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
[provider]
url = "unused"
[worker_types.default]
agent = "claude-code"
run_timeout = "1h"
[worker_types.default.prompts]
ready = "work"
[worker_types.reviewer]
agent = "claude-code"
run_timeout = "1h"
[worker_types.reviewer.prompts]
in_review = "review"
"#;

/// In-process provider: records starts and stops, lists what it runs, enforces `capacity`.
#[derive(Default)]
struct Fake {
    capacity: u32,
    running: Vec<String>,
    starts: Vec<StartWorker>,
    stops: Vec<String>,
    fail_start: bool,
}

type Shared = Arc<Mutex<Fake>>;

async fn serve_fake(capacity: u32, fail_start: bool) -> (Provider, Shared) {
    let fake = Arc::new(Mutex::new(Fake { capacity, fail_start, ..Default::default() }));
    let router = Router::new()
        .route("/workers", post(|State(f): State<Shared>, Json(req): Json<StartWorker>| async move {
            let mut f = f.lock().unwrap();
            if f.fail_start {
                return StatusCode::INTERNAL_SERVER_ERROR;
            }
            f.running.push(req.worker_id.clone());
            f.starts.push(req);
            StatusCode::CREATED
        }))
        .route("/workers/{id}", delete(|State(f): State<Shared>, Path(id): Path<String>| async move {
            let mut f = f.lock().unwrap();
            f.stops.push(id.clone());
            let before = f.running.len();
            f.running.retain(|w| w != &id);
            if f.running.len() < before { StatusCode::NO_CONTENT } else { StatusCode::NOT_FOUND }
        }))
        .route("/status", get(|State(f): State<Shared>| async move {
            let f = f.lock().unwrap();
            let workers = f.running.iter().map(|w| ProviderWorker { worker_id: w.clone(), status: "running".into() }).collect();
            Json(Status { capacity: f.capacity, in_use: f.running.len() as u32, workers })
        }))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (Provider::new(&url), fake)
}

async fn setup(capacity: u32) -> (SqlitePool, Config, Provider, Shared, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::open(&dir.path().join("test.db")).await.unwrap();
    let (provider, fake) = serve_fake(capacity, false).await;
    (pool, Config::parse(CONFIG).unwrap(), provider, fake, dir)
}

async fn ticket(pool: &SqlitePool, state: &str, assignee: Option<&str>) {
    sqlx::query("INSERT INTO tickets (title, description, state, rank, assignee, created_at, updated_at) VALUES ('t', '', ?1, 1, ?2, 'now', 'now')")
        .bind(state)
        .bind(assignee)
        .execute(pool)
        .await
        .unwrap();
}

/// A worker record in `status`, also running at the provider unless `status` is `dead` and `listed` is false.
async fn worker(pool: &SqlitePool, config: &Config, fake: &Shared, worker_type: &str, status: &str, listed: bool) -> String {
    let w = api::new_worker(pool, config, worker_type).await.ok().unwrap();
    sqlx::query("UPDATE workers SET status = ?2 WHERE id = ?1").bind(&w.id).bind(status).execute(pool).await.unwrap();
    if listed {
        fake.lock().unwrap().running.push(w.id.clone());
    }
    w.id
}

async fn statuses(pool: &SqlitePool) -> Vec<(String, String, String)> {
    sqlx::query_as("SELECT id, worker_type, status FROM workers ORDER BY id").fetch_all(pool).await.unwrap()
}

#[tokio::test]
async fn starts_one_worker_per_ticket_up_to_max_workers() {
    let (pool, config, provider, fake, _dir) = setup(10).await;
    for _ in 0..2 {
        ticket(&pool, "ready", None).await;
    }
    ticket(&pool, "in_review", None).await;
    ticket(&pool, "todo", None).await; // no prompt for todo
    ticket(&pool, "ready", Some("w-busy")).await; // taken

    scheduler::tick(&pool, &config, &provider).await.unwrap();
    let workers = statuses(&pool).await;
    assert_eq!(workers.iter().filter(|(_, t, s)| t == "default" && s == "starting").count(), 2);
    assert_eq!(workers.iter().filter(|(_, t, s)| t == "reviewer" && s == "starting").count(), 1);
    let starts = fake.lock().unwrap().starts.clone();
    assert_eq!(starts.len(), 3);
    let ids: Vec<&String> = workers.iter().map(|(id, _, _)| id).collect();
    for s in &starts {
        assert!(ids.contains(&&s.worker_id));
        assert_eq!(s.orchestrator_url, "http://127.0.0.1:8080");
        assert!(!s.worker_token.is_empty());
    }

    // Another ticket: max_workers = 3 is reached, nothing more starts.
    ticket(&pool, "ready", None).await;
    scheduler::tick(&pool, &config, &provider).await.unwrap();
    assert_eq!(statuses(&pool).await.len(), 3);
    assert_eq!(fake.lock().unwrap().starts.len(), 3);
}

#[tokio::test]
async fn blocked_tickets_do_not_start_workers() {
    let (pool, config, provider, fake, _dir) = setup(10).await;
    ticket(&pool, "ready", None).await; // id 1: blocked by 2
    ticket(&pool, "todo", None).await; // id 2
    sqlx::query("INSERT INTO ticket_relations (from_id, type, to_id) VALUES (1, 'depends_on', 2)").execute(&pool).await.unwrap();

    scheduler::tick(&pool, &config, &provider).await.unwrap();
    assert!(statuses(&pool).await.is_empty());
    assert!(fake.lock().unwrap().starts.is_empty());

    sqlx::query("UPDATE tickets SET state = 'done' WHERE id = 2").execute(&pool).await.unwrap();
    scheduler::tick(&pool, &config, &provider).await.unwrap();
    assert_eq!(statuses(&pool).await.len(), 1);
}

#[tokio::test]
async fn provider_capacity_caps_starts() {
    let (pool, config, provider, fake, _dir) = setup(1).await;
    for _ in 0..3 {
        ticket(&pool, "ready", None).await;
    }
    scheduler::tick(&pool, &config, &provider).await.unwrap();
    assert_eq!(fake.lock().unwrap().starts.len(), 1);
    assert_eq!(statuses(&pool).await.len(), 1);
    scheduler::tick(&pool, &config, &provider).await.unwrap();
    assert_eq!(fake.lock().unwrap().starts.len(), 1);
}

#[tokio::test]
async fn pending_workers_count_toward_wanted() {
    let (pool, config, provider, fake, _dir) = setup(10).await;
    for _ in 0..3 {
        ticket(&pool, "ready", None).await;
    }
    worker(&pool, &config, &fake, "default", "starting", true).await;
    worker(&pool, &config, &fake, "default", "idle", true).await;
    scheduler::tick(&pool, &config, &provider).await.unwrap();
    assert_eq!(fake.lock().unwrap().starts.len(), 1);
    assert_eq!(statuses(&pool).await.len(), 3);
    scheduler::tick(&pool, &config, &provider).await.unwrap();
    assert_eq!(fake.lock().unwrap().starts.len(), 1);
}

#[tokio::test]
async fn stops_idle_workers_of_empty_types_but_not_busy_ones() {
    let (pool, config, provider, fake, _dir) = setup(10).await;
    let idle = worker(&pool, &config, &fake, "default", "idle", true).await;
    let busy = worker(&pool, &config, &fake, "default", "busy", true).await;
    let starting = worker(&pool, &config, &fake, "default", "starting", true).await;
    let reviewer = worker(&pool, &config, &fake, "reviewer", "idle", true).await;
    ticket(&pool, "in_review", None).await; // reviewer still has work
    scheduler::tick(&pool, &config, &provider).await.unwrap();
    assert_eq!(fake.lock().unwrap().stops, vec![idle.clone()]);
    assert_eq!(status_of(&pool, &idle).await, "dead");
    assert_eq!(status_of(&pool, &busy).await, "busy");
    assert_eq!(status_of(&pool, &starting).await, "starting");
    assert_eq!(status_of(&pool, &reviewer).await, "idle");
    assert!(fake.lock().unwrap().starts.is_empty());
}

async fn status_of(pool: &SqlitePool, id: &str) -> String {
    let (s,): (String,) = sqlx::query_as("SELECT status FROM workers WHERE id = ?1").bind(id).fetch_one(pool).await.unwrap();
    s
}

#[tokio::test]
async fn dead_workers_still_listed_by_provider_are_stopped() {
    let (pool, config, provider, fake, _dir) = setup(10).await;
    let reaped = worker(&pool, &config, &fake, "default", "dead", true).await;
    let gone = worker(&pool, &config, &fake, "default", "dead", false).await;
    scheduler::tick(&pool, &config, &provider).await.unwrap();
    assert_eq!(fake.lock().unwrap().stops, vec![reaped.clone()]);
    assert!(fake.lock().unwrap().running.is_empty());
    scheduler::tick(&pool, &config, &provider).await.unwrap();
    assert_eq!(fake.lock().unwrap().stops.len(), 1, "{gone} was never listed and {reaped} is gone now");
}

#[tokio::test]
async fn unknown_provider_workers_are_stopped() {
    let (pool, config, provider, fake, _dir) = setup(10).await;
    let starting = worker(&pool, &config, &fake, "default", "starting", true).await;
    let idle = worker(&pool, &config, &fake, "default", "idle", true).await;
    let busy = worker(&pool, &config, &fake, "default", "busy", true).await;
    ticket(&pool, "ready", None).await; // idle still has work
    fake.lock().unwrap().running.push("w-unknown".into());
    scheduler::tick(&pool, &config, &provider).await.unwrap();
    assert_eq!(fake.lock().unwrap().stops, vec!["w-unknown".to_string()]);
    assert_eq!(fake.lock().unwrap().running, vec![starting.clone(), idle.clone(), busy.clone()]);
    assert_eq!(status_of(&pool, &starting).await, "starting");
    assert_eq!(status_of(&pool, &idle).await, "idle");
    assert_eq!(status_of(&pool, &busy).await, "busy");
    assert!(fake.lock().unwrap().starts.is_empty());
}

#[tokio::test]
async fn failed_start_marks_worker_dead() {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::open(&dir.path().join("test.db")).await.unwrap();
    let (provider, fake) = serve_fake(10, true).await;
    let config = Config::parse(CONFIG).unwrap();
    ticket(&pool, "ready", None).await;
    assert!(scheduler::tick(&pool, &config, &provider).await.is_err());
    let workers = statuses(&pool).await;
    assert_eq!(workers.len(), 1);
    assert_eq!(workers[0].2, "dead");
    assert!(fake.lock().unwrap().starts.is_empty());
}

#[tokio::test]
async fn provider_down_is_an_error_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::open(&dir.path().join("test.db")).await.unwrap();
    let config = Config::parse(CONFIG).unwrap();
    ticket(&pool, "ready", None).await;
    let err = scheduler::tick(&pool, &config, &Provider::new("http://127.0.0.1:1")).await.unwrap_err();
    assert!(err.to_string().contains("provider status"), "{err:#}");
    assert!(statuses(&pool).await.is_empty());
}
