use std::sync::{Arc, Mutex};

use api_client::{ApproveUser, Client, CreateProvider, CreateWorker, Error, ShipLogs, UpdateProvider, UpdateTicket};
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::routing::delete;
use axum::Router;

mod common;
use common::{session, OWNER, TOKEN};

async fn serve() -> (String, tempfile::TempDir) {
    common::serve(include_str!("../../../factory.example.toml")).await
}

fn status<T: std::fmt::Debug>(r: Result<T, Error>) -> u16 {
    match r {
        Err(Error::Api { status, .. }) => status,
        other => panic!("expected an API error, got {other:?}"),
    }
}

fn add(name: &str) -> CreateProvider {
    CreateProvider { name: name.into(), url: "http://localhost:9000".into(), token: "pt".into() }
}

/// Two approved developers, signed in.
async fn developers(url: &str, dir: &tempfile::TempDir) -> (Client, Client) {
    let admin = Client::new(url, TOKEN);
    admin.approve_user("alice-principal", &ApproveUser { name: "Alice".into() }).await.unwrap();
    admin.approve_user("bob-principal", &ApproveUser { name: "Bob".into() }).await.unwrap();
    (Client::new(url, session(dir, "alice-principal", 1).await), Client::new(url, session(dir, "bob-principal", 1).await))
}

#[tokio::test]
async fn developers_add_list_update_and_remove_their_own_providers() {
    let (url, dir) = serve().await;
    let admin = Client::new(&url, TOKEN);
    let (alice, bob) = developers(&url, &dir).await;
    let worker = admin.create_worker(&CreateWorker { agent: "claude-code".into(), provider: None }).await.unwrap();
    let worker = Client::new(&url, &worker.token);

    // Developers only.
    assert_eq!(status(admin.create_provider(&add("x")).await), 403);
    assert_eq!(status(worker.create_provider(&add("x")).await), 403);
    assert_eq!(status(alice.create_provider(&CreateProvider { name: " ".into(), ..add("x") }).await), 400);
    assert_eq!(status(alice.create_provider(&CreateProvider { token: "".into(), ..add("x") }).await), 400);
    let mine = alice.create_provider(&add("mine")).await.unwrap();
    assert_eq!((mine.id, mine.owner.as_str(), mine.name.as_str(), mine.url.as_str()), (3, "alice-principal", "mine", "http://localhost:9000"));
    assert!(mine.status.is_none(), "never answered yet");
    // Names are unique per owner.
    assert_eq!(status(alice.create_provider(&add("mine")).await), 409);
    let bobs = bob.create_provider(&add("mine")).await.unwrap();
    assert_eq!(bobs.owner, "bob-principal");

    // Everyone lists every provider, with the last status and never the token.
    let raw = reqwest::Client::new().get(format!("{url}/providers")).bearer_auth(TOKEN).send().await.unwrap().text().await.unwrap();
    assert!(!raw.contains("token") && !raw.contains("\"pt\""), "{raw}");
    let listed = worker.list_providers().await.unwrap();
    assert_eq!(listed.iter().map(|p| (p.id, p.owner.as_str())).collect::<Vec<_>>(), [(1, OWNER), (2, OWNER), (3, "alice-principal"), (4, "bob-principal")]);
    assert_eq!(listed[0].status.as_ref().unwrap().capacity, 4);
    assert!(listed[2].status.is_none());

    // Only the owner edits url and token.
    assert_eq!(status(bob.update_provider(mine.id, &UpdateProvider { url: Some("http://x".into()), ..Default::default() }).await), 403);
    assert_eq!(status(admin.update_provider(mine.id, &UpdateProvider { url: Some("http://x".into()), ..Default::default() }).await), 403);
    assert_eq!(status(alice.update_provider(99, &UpdateProvider { url: Some("http://x".into()), ..Default::default() }).await), 404);
    let updated = alice.update_provider(mine.id, &UpdateProvider { url: Some("http://elsewhere:9001/".into()), token: Some("pt2".into()) }).await.unwrap();
    assert_eq!(updated.url, "http://elsewhere:9001/");

    // The owner or the admin removes.
    assert_eq!(status(bob.delete_provider(mine.id).await), 403);
    assert_eq!(status(worker.delete_provider(mine.id).await), 403);
    assert_eq!(status(alice.delete_provider(99).await), 404);
    alice.delete_provider(mine.id).await.unwrap();
    admin.delete_provider(bobs.id).await.unwrap();
    assert_eq!(status(alice.delete_provider(mine.id).await), 404);
    assert_eq!(alice.list_providers().await.unwrap().iter().map(|p| p.id).collect::<Vec<_>>(), [1, 2]);
}

/// A provider that records the workers it is asked to stop, and the token it was asked with.
async fn fake_provider() -> (String, Arc<Mutex<Vec<(String, Option<String>)>>>) {
    let stops: Arc<Mutex<Vec<(String, Option<String>)>>> = Default::default();
    let router = Router::new()
        .route("/workers/{id}", delete(|State(stops): State<Arc<Mutex<Vec<(String, Option<String>)>>>>, headers: HeaderMap, Path(id): Path<String>| async move {
            let token = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).map(str::to_owned);
            stops.lock().unwrap().push((id, token));
            StatusCode::NO_CONTENT
        }))
        .with_state(stops.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, stops)
}

#[tokio::test]
async fn removing_a_provider_stops_its_workers_and_forgets_them() {
    let (url, dir) = serve().await;
    let admin = Client::new(&url, TOKEN);
    let (alice, _) = developers(&url, &dir).await;
    let (fake, stops) = fake_provider().await;
    let p = alice.create_provider(&CreateProvider { name: "docker".into(), url: fake, token: "pt".into() }).await.unwrap();
    let w = admin.create_worker(&CreateWorker { agent: "claude-code".into(), provider: Some(p.id) }).await.unwrap();
    let wc = Client::new(&url, &w.token);
    wc.register(&w.id).await.unwrap();
    let dead = admin.create_worker(&CreateWorker { agent: "claude-code".into(), provider: Some(p.id) }).await.unwrap();
    sqlx::SqlitePool::connect(&format!("sqlite://{}/test.db", dir.path().display()))
        .await
        .unwrap()
        .execute_dead(&dead.id)
        .await;
    let t = admin.create_ticket(&api_client::CreateTicket { title: "t".into(), state: Some("ready".into()), ..Default::default() }).await.unwrap();
    let run = wc.poll(&w.id, None).await.unwrap().unwrap().run;
    wc.ship_logs(&w.id, &ShipLogs { run: Some(run), lines: vec!["hi".into()] }).await.unwrap();
    assert_eq!(admin.get_ticket(t.id).await.unwrap().assignee.as_deref(), Some(w.id.as_str()));

    alice.delete_provider(p.id).await.unwrap();
    // Only the live worker was stopped, with the provider's token.
    assert_eq!(*stops.lock().unwrap(), [(w.id.clone(), Some("Bearer pt".to_string()))]);
    // Its records are gone, its token no longer works, and the ticket is free again.
    assert!(admin.list_workers().await.unwrap().iter().all(|x| x.id != w.id && x.id != dead.id));
    assert_eq!(status(wc.heartbeat(&w.id).await), 401);
    assert_eq!(status(admin.run_logs(run, None).await), 404);
    let t = admin.get_ticket(t.id).await.unwrap();
    assert!(t.assignee.is_none() && t.runs.is_empty());
    admin.update_ticket(t.id, &UpdateTicket { state: Some("todo".into()), ..Default::default() }).await.unwrap();
}

trait ExecuteDead {
    async fn execute_dead(&self, id: &str);
}

impl ExecuteDead for sqlx::SqlitePool {
    async fn execute_dead(&self, id: &str) {
        sqlx::query("UPDATE workers SET status = 'dead' WHERE id = ?1").bind(id).execute(self).await.unwrap();
    }
}

#[tokio::test]
async fn revoking_a_developer_removes_their_providers() {
    let (url, dir) = serve().await;
    let admin = Client::new(&url, TOKEN);
    let (alice, bob) = developers(&url, &dir).await;
    let (fake, stops) = fake_provider().await;
    let p = alice.create_provider(&CreateProvider { name: "docker".into(), url: fake, token: "pt".into() }).await.unwrap();
    bob.create_provider(&add("bobs")).await.unwrap();
    let w = admin.create_worker(&CreateWorker { agent: "claude-code".into(), provider: Some(p.id) }).await.unwrap();

    admin.revoke_user("alice-principal").await.unwrap();
    assert_eq!(stops.lock().unwrap().iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), [w.id.as_str()]);
    assert_eq!(admin.list_providers().await.unwrap().iter().map(|p| p.owner.as_str()).collect::<Vec<_>>(), [OWNER, OWNER, "bob-principal"]);
    assert!(admin.list_workers().await.unwrap().is_empty());
}
