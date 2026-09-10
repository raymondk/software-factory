use std::sync::Arc;

use api_client::{Client, CreateComment, CreateTicket, CreateWorker, Error, MoveTicket, NewWorker, UpdateTicket};
use tokio::sync::Barrier;

mod common;
use common::TOKEN;

const CONFIG: &str = r#"
[project]
name = "test"
repos = ["https://github.com/org/a.git", "https://github.com/org/b.git"]
[orchestrator]
listen = "127.0.0.1:0"
token = "x"
heartbeat_timeout = "60s"
[scheduler]
max_workers = 4
[provider]
url = "http://localhost:8081"
[worker_types.default]
agent = "claude-code"
run_timeout = "1h"
[worker_types.default.prompts]
ready = "Work on #{{ticket.id}} ({{ticket.state}}): {{ticket.title}}\n{{ticket.description}}"
in_progress = "Resume #{{ticket.id}}"
[worker_types.reviewer]
agent = "claude-code"
run_timeout = "1h"
[worker_types.reviewer.prompts]
in_review = "Review #{{ticket.id}}"
"#;

async fn serve() -> (String, tempfile::TempDir) {
    common::serve(CONFIG).await
}

fn status<T: std::fmt::Debug>(r: Result<T, Error>) -> u16 {
    match r {
        Err(Error::Api { status, .. }) => status,
        other => panic!("expected an API error, got {other:?}"),
    }
}

/// Creates a worker of `worker_type` and returns it with a client authenticated as it.
async fn worker(url: &str, human: &Client, worker_type: &str) -> (NewWorker, Client) {
    let w = human.create_worker(&CreateWorker { worker_type: worker_type.into() }).await.unwrap();
    let client = Client::new(url, &w.token);
    (w, client)
}

async fn ticket(human: &Client, title: &str, state: &str) -> i64 {
    let t = human.create_ticket(&CreateTicket { title: title.into(), description: format!("about {title}") }).await.unwrap();
    human.update_ticket(t.id, &UpdateTicket { state: Some(state.into()), ..Default::default() }).await.unwrap();
    t.id
}

fn set_state(state: &str) -> UpdateTicket {
    UpdateTicket { state: Some(state.into()), ..Default::default() }
}

#[tokio::test]
async fn create_register_heartbeat_list() {
    let (url, _dir) = serve().await;
    let human = Client::new(&url, TOKEN);
    let (w, wc) = worker(&url, &human, "default").await;
    assert_eq!(w.worker_type, "default");
    assert!(w.id.len() >= 8 && w.token.len() >= 32);

    let listed = human.list_workers().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!((listed[0].id.as_str(), listed[0].status.as_str(), listed[0].last_heartbeat.as_deref(), listed[0].ticket), (w.id.as_str(), "starting", None, None));
    // Never the token, in any representation.
    let raw = serde_json::to_string(&listed).unwrap();
    assert!(!raw.contains(&w.token) && !raw.contains("token"));

    let r = wc.register(&w.id).await.unwrap();
    assert_eq!(r.status, "idle");
    let first = r.last_heartbeat.expect("register sets last_heartbeat");
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let h = wc.heartbeat(&w.id).await.unwrap();
    assert!(h.last_heartbeat.unwrap() > first);
    assert_eq!(h.status, "idle");
    // Workers may list workers too.
    assert_eq!(wc.list_workers().await.unwrap()[0].status, "idle");

    assert_eq!(status(human.create_worker(&CreateWorker { worker_type: "nope".into() }).await), 400);
    assert_eq!(status(wc.create_worker(&CreateWorker { worker_type: "default".into() }).await), 403);
}

#[tokio::test]
async fn worker_token_authenticates_only_itself() {
    let (url, _dir) = serve().await;
    let human = Client::new(&url, TOKEN);
    let (a, ac) = worker(&url, &human, "default").await;
    let (b, _) = worker(&url, &human, "default").await;

    for other in [b.id.as_str(), "w-missing"] {
        assert_eq!(status(ac.register(other).await), 403);
        assert_eq!(status(ac.heartbeat(other).await), 403);
        assert_eq!(status(ac.poll(other).await), 403);
    }
    // Humans are not workers.
    assert_eq!(status(human.register(&a.id).await), 403);
    assert_eq!(status(human.poll(&a.id).await), 403);
    // Garbage tokens are rejected outright.
    let garbage = Client::new(&url, "not-a-token");
    assert_eq!(status(garbage.register(&a.id).await), 401);
    assert_eq!(status(garbage.list_workers().await), 401);
    // Own id works.
    ac.register(&a.id).await.unwrap();
}

#[tokio::test]
async fn poll_takes_lowest_ranked_available_for_type() {
    let (url, _dir) = serve().await;
    let human = Client::new(&url, TOKEN);
    let (w, wc) = worker(&url, &human, "default").await;
    wc.register(&w.id).await.unwrap();

    let todo = ticket(&human, "todo", "todo").await;
    let ready1 = ticket(&human, "ready1", "ready").await;
    let held = ticket(&human, "held", "ready").await;
    human.update_ticket(held, &UpdateTicket { assignee: Some(Some("someone".into())), ..Default::default() }).await.unwrap();
    let resumable = ticket(&human, "resumable", "in_progress").await;
    let review = ticket(&human, "review", "in_review").await;
    // Move the in_progress one to the front: rank decides, not id.
    human.move_ticket(resumable, &MoveTicket { before: Some(todo), after: None }).await.unwrap();

    let p = wc.poll(&w.id).await.unwrap().expect("a ticket");
    assert_eq!(p.ticket.id, resumable);
    assert_eq!(p.ticket.assignee.as_deref(), Some(w.id.as_str()));
    assert_eq!(p.prompt, format!("Resume #{resumable}"));
    assert_eq!(p.repos, ["https://github.com/org/a.git", "https://github.com/org/b.git"]);
    assert_eq!(human.get_ticket(resumable).await.unwrap().assignee.as_deref(), Some(w.id.as_str()));
    let listed = human.list_workers().await.unwrap();
    assert_eq!((listed[0].status.as_str(), listed[0].ticket), ("busy", Some(resumable)));

    // Next: ready1. Held and todo/in_review are skipped. Prompt renders every field.
    let p = wc.poll(&w.id).await.unwrap().expect("a ticket");
    assert_eq!(p.ticket.id, ready1);
    assert_eq!(p.prompt, format!("Work on #{ready1} (ready): ready1\nabout ready1"));

    // Nothing left for this type: idle, 204.
    assert!(wc.poll(&w.id).await.unwrap().is_none());
    assert_eq!(human.list_workers().await.unwrap()[0].status, "idle");
    assert_eq!(human.get_ticket(review).await.unwrap().assignee, None);
    assert_eq!(human.get_ticket(todo).await.unwrap().assignee, None);

    // The reviewer type only sees in_review.
    let (r, rc) = worker(&url, &human, "reviewer").await;
    let p = rc.poll(&r.id).await.unwrap().expect("a ticket");
    assert_eq!((p.ticket.id, p.prompt.as_str()), (review, format!("Review #{review}").as_str()));
    assert!(rc.poll(&r.id).await.unwrap().is_none());
}

#[tokio::test]
async fn concurrent_polls_never_share_a_ticket() {
    let (url, _dir) = serve().await;
    let human = Client::new(&url, TOKEN);
    let mut tickets = vec![];
    for i in 0..3 {
        tickets.push(ticket(&human, &format!("t{i}"), "ready").await);
    }
    let barrier = Arc::new(Barrier::new(5));
    let mut handles = vec![];
    for _ in 0..5 {
        let (w, wc) = worker(&url, &human, "default").await;
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            wc.poll(&w.id).await.unwrap().map(|p| (w.id, p.ticket.id))
        }));
    }
    let mut got: Vec<(String, i64)> = vec![];
    for h in handles {
        if let Some(pair) = h.await.unwrap() {
            got.push(pair);
        }
    }
    let mut ids: Vec<i64> = got.iter().map(|(_, t)| *t).collect();
    ids.sort();
    tickets.sort();
    assert_eq!(ids, tickets, "each ticket handed out exactly once");
    for (w, t) in &got {
        assert_eq!(human.get_ticket(*t).await.unwrap().assignee.as_deref(), Some(w.as_str()));
    }
    let busy = human.list_workers().await.unwrap().iter().filter(|w| w.status == "busy").count();
    assert_eq!(busy, 3);
}

#[tokio::test]
async fn acl_workers_modify_only_held_tickets() {
    let (url, _dir) = serve().await;
    let human = Client::new(&url, TOKEN);
    let (w, wc) = worker(&url, &human, "default").await;
    let (other, oc) = worker(&url, &human, "default").await;
    let mine = ticket(&human, "mine", "ready").await;
    let theirs = ticket(&human, "theirs", "todo").await;
    assert_eq!(wc.poll(&w.id).await.unwrap().unwrap().ticket.id, mine);

    // Not held: PATCH and move are forbidden, for the poller and a bystander alike.
    for c in [&wc, &oc] {
        assert_eq!(status(c.update_ticket(theirs, &UpdateTicket { title: Some("x".into()), ..Default::default() }).await), 403);
        assert_eq!(status(c.move_ticket(theirs, &MoveTicket { before: Some(mine), after: None }).await), 403);
    }
    assert_eq!(status(oc.update_ticket(mine, &set_state("done")).await), 403);
    assert_eq!(status(oc.move_ticket(mine, &MoveTicket { after: Some(theirs), ..Default::default() }).await), 403);
    let t = human.get_ticket(theirs).await.unwrap();
    assert_eq!((t.title.as_str(), t.rank), ("theirs", 2.0));
    assert_eq!(human.get_ticket(mine).await.unwrap().state, "ready");
    // Unknown ticket is 404 before the ACL.
    assert_eq!(status(wc.update_ticket(9999, &UpdateTicket::default()).await), 404);

    // Held: allowed. Moving too.
    let u = wc.update_ticket(mine, &UpdateTicket { links: Some(vec!["https://x/pr/1".into()]), ..Default::default() }).await.unwrap();
    assert_eq!(u.links, ["https://x/pr/1"]);
    assert_eq!(wc.move_ticket(mine, &MoveTicket { after: Some(theirs), ..Default::default() }).await.unwrap().rank, 3.0);

    // Anyone may create tickets and comment on and resolve comments on any ticket; author is the worker id.
    let created = oc.create_ticket(&CreateTicket { title: "from worker".into(), description: String::new() }).await.unwrap();
    assert_eq!(created.state, "todo");
    let c = oc.add_comment(mine, &CreateComment { body: "hi".into() }).await.unwrap();
    assert_eq!(c.author, other.id);
    assert!(wc.resolve_comment(mine, c.id).await.unwrap().resolved);
    assert_eq!(oc.add_comment(created.id, &CreateComment { body: "note".into() }).await.unwrap().author, other.id);
    assert_eq!(oc.list_comments(mine).await.unwrap().len(), 1);
    assert_eq!(oc.get_ticket(mine).await.unwrap().comments[0].author, other.id);

    // A human may modify anything.
    human.update_ticket(mine, &UpdateTicket { title: Some("renamed".into()), ..Default::default() }).await.unwrap();
}

#[tokio::test]
async fn state_change_clears_assignee() {
    let (url, _dir) = serve().await;
    let human = Client::new(&url, TOKEN);
    let (w, wc) = worker(&url, &human, "default").await;
    let id = ticket(&human, "a", "ready").await;
    wc.poll(&w.id).await.unwrap().unwrap();

    // Same state: still held.
    let t = wc.update_ticket(id, &set_state("ready")).await.unwrap();
    assert_eq!(t.assignee.as_deref(), Some(w.id.as_str()));
    // Other fields: still held.
    let t = wc.update_ticket(id, &UpdateTicket { description: Some("d".into()), ..Default::default() }).await.unwrap();
    assert_eq!(t.assignee.as_deref(), Some(w.id.as_str()));
    // State change: cleared, and the worker no longer holds it.
    let t = wc.update_ticket(id, &set_state("in_progress")).await.unwrap();
    assert_eq!((t.state.as_str(), t.assignee), ("in_progress", None));
    assert_eq!(status(wc.update_ticket(id, &set_state("in_review")).await), 403);
    assert_eq!(human.list_workers().await.unwrap()[0].ticket, None);
    // Explicit assignee in the same patch wins.
    let t = human.update_ticket(id, &UpdateTicket { state: Some("in_review".into()), assignee: Some(Some("bob".into())), ..Default::default() }).await.unwrap();
    assert_eq!((t.state.as_str(), t.assignee.as_deref()), ("in_review", Some("bob")));
    let t = human.update_ticket(id, &set_state("done")).await.unwrap();
    assert_eq!(t.assignee, None);
}

#[tokio::test]
async fn reaper_marks_silent_workers_dead_and_frees_tickets() {
    let timeout = std::time::Duration::from_millis(200);
    let (url, dir) = common::serve(&CONFIG.replace("\"60s\"", "\"200ms\"")).await;
    let pool = orchestrator::db::open(&dir.path().join("test.db")).await.unwrap();
    let human = Client::new(&url, TOKEN);
    let (w, wc) = worker(&url, &human, "default").await;
    wc.register(&w.id).await.unwrap();
    let id = ticket(&human, "a", "in_progress").await;
    assert_eq!(wc.poll(&w.id).await.unwrap().unwrap().ticket.id, id);
    // Never registers: reaped from its creation time.
    let (silent, sc) = worker(&url, &human, "default").await;
    let (live, lc) = worker(&url, &human, "default").await;
    lc.register(&live.id).await.unwrap();

    assert!(orchestrator::reaper::reap(&pool, timeout).await.unwrap().is_empty());
    tokio::time::sleep(timeout + std::time::Duration::from_millis(100)).await;
    lc.heartbeat(&live.id).await.unwrap();
    let mut reaped = orchestrator::reaper::reap(&pool, timeout).await.unwrap();
    reaped.sort();
    let mut expected = vec![w.id.clone(), silent.id.clone()];
    expected.sort();
    assert_eq!(reaped, expected);
    assert!(orchestrator::reaper::reap(&pool, timeout).await.unwrap().is_empty());

    let by_id: std::collections::HashMap<_, _> = human.list_workers().await.unwrap().into_iter().map(|x| (x.id.clone(), x)).collect();
    assert_eq!((by_id[&w.id].status.as_str(), by_id[&w.id].ticket), ("dead", None));
    assert_eq!(by_id[&silent.id].status, "dead");
    assert_eq!(by_id[&live.id].status, "idle");
    // The ticket keeps its state with the assignee cleared.
    let t = human.get_ticket(id).await.unwrap();
    assert_eq!((t.state.as_str(), t.assignee), ("in_progress", None));

    // Dead tokens no longer authenticate.
    assert_eq!(status(wc.heartbeat(&w.id).await), 401);
    assert_eq!(status(wc.register(&w.id).await), 401);
    assert_eq!(status(wc.poll(&w.id).await), 401);
    assert_eq!(status(wc.update_ticket(id, &set_state("done")).await), 401);
    assert_eq!(status(sc.register(&silent.id).await), 401);
    lc.heartbeat(&live.id).await.unwrap();

    // The next poll resumes the ticket.
    let (f, fc) = worker(&url, &human, "default").await;
    fc.register(&f.id).await.unwrap();
    let p = fc.poll(&f.id).await.unwrap().expect("the freed ticket");
    assert_eq!((p.ticket.id, p.ticket.assignee.as_deref(), p.prompt.as_str()), (id, Some(f.id.as_str()), format!("Resume #{id}").as_str()));
}
