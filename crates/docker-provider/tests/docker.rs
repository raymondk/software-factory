//! Integration tests against the local Docker daemon. Skipped when it is unavailable.

use std::process::Command;
use std::time::{Duration, Instant};

use docker_provider::{api, config::Config, docker::LABEL, AppState};
use reqwest::StatusCode;
use serde_json::{json, Value};

/// Built from alpine on first use; sleeps so containers stay running.
const SLEEP_IMAGE: &str = "software-factory/test-sleep";
/// Exits immediately (plain `sh` with no stdin).
const EXIT_IMAGE: &str = "alpine:3.20";

fn docker(args: &[&str]) -> Option<String> {
    let out = Command::new("docker").args(args).output().ok()?;
    if !out.status.success() {
        eprintln!("docker {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Returns false (after printing why) when Docker is unavailable.
fn docker_ready() -> bool {
    if docker(&["info"]).is_none() {
        eprintln!("skipping: docker daemon unavailable");
        return false;
    }
    if docker(&["image", "inspect", SLEEP_IMAGE]).is_none() {
        let mut child = Command::new("docker")
            .args(["build", "-q", "-t", SLEEP_IMAGE, "-"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(child.stdin.as_mut().unwrap(), b"FROM alpine:3.20\nCMD [\"sleep\",\"300\"]\n").unwrap();
        assert!(child.wait().unwrap().success(), "building {SLEEP_IMAGE}");
    }
    true
}

async fn serve(image: &str, max_workers: u32) -> String {
    let config = Config::parse(&format!(
        "[provider]\nlisten = \"127.0.0.1:0\"\nmax_workers = {max_workers}\n\
         [docker]\nimage = \"{image}\"\n[worker_env]\nGIT_TOKEN = \"git-secret\"\n"
    ))
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, api::router(AppState::new(config))).await.unwrap() });
    url
}

/// Removes every container labelled with one of its worker ids, even when a test panics.
struct Workers(Vec<String>);

impl Workers {
    fn new(tag: &str, n: usize) -> Workers {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        Workers((0..n).map(|i| format!("test-{tag}-{}-{nanos}-{i}", std::process::id())).collect())
    }
}

impl Drop for Workers {
    fn drop(&mut self) {
        for id in &self.0 {
            if let Some(cids) = docker(&["ps", "-aq", "--filter", &format!("label={LABEL}={id}")]) {
                for cid in cids.lines() {
                    docker(&["rm", "-f", cid]);
                }
            }
        }
    }
}

async fn start(url: &str, worker_id: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{url}/workers"))
        .json(&json!({
            "worker_id": worker_id,
            "worker_type": "default",
            "orchestrator_url": "http://orchestrator:8080",
            "worker_token": "worker-secret",
        }))
        .send()
        .await
        .unwrap()
}

async fn stop(url: &str, worker_id: &str) -> StatusCode {
    reqwest::Client::new().delete(format!("{url}/workers/{worker_id}")).send().await.unwrap().status()
}

async fn status(url: &str) -> Value {
    let res = reqwest::get(format!("{url}/status")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    res.json().await.unwrap()
}

#[tokio::test]
async fn start_sets_env_and_stop_removes() {
    if !docker_ready() {
        return;
    }
    let url = serve(SLEEP_IMAGE, 4).await;
    let w = Workers::new("env", 1);
    let id = &w.0[0];

    let res = start(&url, id).await;
    assert_eq!(res.status(), StatusCode::CREATED);
    let worker: Value = res.json().await.unwrap();
    assert_eq!(worker["worker_id"], *id);
    let cid = worker["container_id"].as_str().unwrap();

    let env: Vec<String> = serde_json::from_str(&docker(&["inspect", "--format", "{{json .Config.Env}}", cid]).unwrap()).unwrap();
    for expected in [
        "FACTORY_URL=http://orchestrator:8080",
        "FACTORY_TOKEN=worker-secret",
        &format!("FACTORY_WORKER_ID={id}"),
        "FACTORY_WORKER_TOKEN=worker-secret",
        "FACTORY_WORKER_TYPE=default",
        "GIT_TOKEN=git-secret",
    ] {
        assert!(env.iter().any(|e| e == expected), "missing {expected} in {env:?}");
    }
    assert_eq!(docker(&["inspect", "--format", "{{.State.Status}}", cid]).unwrap(), "running");

    let s = status(&url).await;
    assert_eq!(s["capacity"], 4);
    assert_eq!(s["in_use"], 1);
    assert_eq!(s["image"], SLEEP_IMAGE);
    assert_eq!(s["workers"], json!([{ "worker_id": id, "container_id": cid, "status": "running" }]));

    assert_eq!(stop(&url, id).await, StatusCode::NO_CONTENT);
    assert!(docker(&["inspect", cid]).is_none(), "container still exists");
    let s = status(&url).await;
    assert_eq!(s["in_use"], 0);
    assert_eq!(s["workers"], json!([]));
    assert_eq!(stop(&url, id).await, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn refuses_beyond_capacity_and_duplicates() {
    if !docker_ready() {
        return;
    }
    let url = serve(SLEEP_IMAGE, 1).await;
    let w = Workers::new("cap", 2);
    let (a, b) = (&w.0[0], &w.0[1]);

    assert_eq!(start(&url, a).await.status(), StatusCode::CREATED);
    let dup = start(&url, a).await;
    assert_eq!(dup.status(), StatusCode::CONFLICT);
    assert!(dup.json::<Value>().await.unwrap()["error"].as_str().unwrap().contains("already exists"));
    let full = start(&url, b).await;
    assert_eq!(full.status(), StatusCode::CONFLICT);
    assert!(full.json::<Value>().await.unwrap()["error"].as_str().unwrap().contains("capacity"));
    assert_eq!(status(&url).await["in_use"], 1);

    assert_eq!(stop(&url, a).await, StatusCode::NO_CONTENT);
    assert_eq!(start(&url, b).await.status(), StatusCode::CREATED);
    assert_eq!(stop(&url, b).await, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn status_tracks_containers_that_exit_or_vanish() {
    if !docker_ready() {
        return;
    }
    let url = serve(EXIT_IMAGE, 1).await;
    let w = Workers::new("exit", 2);
    let (a, b) = (&w.0[0], &w.0[1]);

    let worker: Value = start(&url, a).await.json().await.unwrap();
    let cid = worker["container_id"].as_str().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let s = loop {
        let s = status(&url).await;
        if s["workers"][0]["status"] == "exited" || Instant::now() > deadline {
            break s;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert_eq!(s["workers"], json!([{ "worker_id": a, "container_id": cid, "status": "exited" }]));
    assert_eq!(s["in_use"], 0, "exited workers do not use capacity");
    assert_eq!(start(&url, b).await.status(), StatusCode::CREATED, "capacity is free again");

    docker(&["rm", "-f", cid]).unwrap();
    let s = status(&url).await;
    assert_eq!(s["workers"].as_array().unwrap().len(), 1, "vanished container is dropped");
    assert_eq!(s["workers"][0]["worker_id"], *b);
    assert_eq!(stop(&url, a).await, StatusCode::NOT_FOUND);
    assert_eq!(stop(&url, b).await, StatusCode::NO_CONTENT);
}
