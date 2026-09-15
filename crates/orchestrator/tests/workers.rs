use std::sync::Arc;

use api_client::{Client, CreateComment, CreateRelation, CreateTicket, CreateWorker, Error, MoveTicket, NewWorker, ReportUsage, ShipLogs, Totals, UpdateTicket};
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
[providers.a]
url = "http://localhost:8081"
[providers.b]
url = "http://localhost:8082"
[agents.claude-code]
run_timeout = "1h"
[agents.codex]
run_timeout = "2h"
[prompts]
ready = "Work on #{{ticket.id}} ({{ticket.state}}): {{ticket.title}}\n{{ticket.description}}"
in_progress = "Resume #{{ticket.id}}"
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

/// Creates a worker of `agent` on the default provider and returns it with a client authenticated as it.
async fn worker(url: &str, human: &Client, agent: &str) -> (NewWorker, Client) {
    worker_on(url, human, agent, None).await
}

async fn worker_on(url: &str, human: &Client, agent: &str, provider: Option<&str>) -> (NewWorker, Client) {
    let w = human.create_worker(&CreateWorker { agent: agent.into(), provider: provider.map(str::to_owned) }).await.unwrap();
    let client = Client::new(url, &w.token);
    (w, client)
}

async fn ticket(human: &Client, title: &str, state: &str) -> i64 {
    let t = human.create_ticket(&CreateTicket { title: title.into(), description: format!("about {title}"), ..Default::default() }).await.unwrap();
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
    let (w, wc) = worker(&url, &human, "claude-code").await;
    assert_eq!((w.agent.as_str(), w.provider.as_str()), ("claude-code", "a"), "the first configured provider by default");
    assert!(w.id.len() >= 8 && w.token.len() >= 32);

    let listed = human.list_workers().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!((listed[0].id.as_str(), listed[0].status.as_str(), listed[0].last_heartbeat.as_deref(), listed[0].ticket), (w.id.as_str(), "starting", None, None));
    assert_eq!((listed[0].agent.as_str(), listed[0].provider.as_str()), ("claude-code", "a"));
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

    assert_eq!(status(human.create_worker(&CreateWorker { agent: "nope".into(), provider: None }).await), 400);
    assert_eq!(status(human.create_worker(&CreateWorker { agent: "claude-code".into(), provider: Some("nope".into()) }).await), 400);
    assert_eq!(status(wc.create_worker(&CreateWorker { agent: "claude-code".into(), provider: None }).await), 403);
    let (b, _) = worker_on(&url, &human, "codex", Some("b")).await;
    assert_eq!(b.provider, "b");
}

#[tokio::test]
async fn poll_hands_a_model_only_to_a_worker_whose_provider_supports_it() {
    // Provider a runs claude-code with sonnet only; b also has opus.
    let (url, _dir) = common::serve_with(CONFIG, |name| {
        let mut agents = common::advertised();
        if name == "a" {
            agents.get_mut("claude-code").unwrap().models = vec!["sonnet".into()];
        }
        common::status(agents)
    })
    .await;
    let human = Client::new(&url, TOKEN);
    let (a, ac) = worker_on(&url, &human, "claude-code", Some("a")).await;
    let (b, bc) = worker_on(&url, &human, "claude-code", Some("b")).await;
    let opus = ticket(&human, "needs opus", "ready").await;
    human.update_ticket(opus, &UpdateTicket { model: Some(Some("opus".into())), ..Default::default() }).await.unwrap();
    let any = ticket(&human, "any model", "ready").await;

    assert_eq!(ac.poll(&a.id, None).await.unwrap().unwrap().ticket.id, any, "a skips the opus ticket");
    assert!(ac.poll(&a.id, None).await.unwrap().is_none());
    let p = bc.poll(&b.id, None).await.unwrap().expect("a ticket");
    assert_eq!((p.ticket.id, p.model.as_deref()), (opus, Some("opus")));
}

#[tokio::test]
async fn worker_token_authenticates_only_itself() {
    let (url, _dir) = serve().await;
    let human = Client::new(&url, TOKEN);
    let (a, ac) = worker(&url, &human, "claude-code").await;
    let (b, _) = worker(&url, &human, "claude-code").await;

    for other in [b.id.as_str(), "w-missing"] {
        assert_eq!(status(ac.register(other).await), 403);
        assert_eq!(status(ac.heartbeat(other).await), 403);
        assert_eq!(status(ac.poll(other, None).await), 403);
    }
    // Humans are not workers.
    assert_eq!(status(human.register(&a.id).await), 403);
    assert_eq!(status(human.poll(&a.id, None).await), 403);
    // Garbage tokens are rejected outright.
    let garbage = Client::new(&url, "not-a-token");
    assert_eq!(status(garbage.register(&a.id).await), 401);
    assert_eq!(status(garbage.list_workers().await), 401);
    // Own id works.
    ac.register(&a.id).await.unwrap();
}

#[tokio::test]
async fn poll_takes_lowest_ranked_available() {
    let (url, _dir) = serve().await;
    let human = Client::new(&url, TOKEN);
    let (w, wc) = worker(&url, &human, "claude-code").await;
    wc.register(&w.id).await.unwrap();

    let todo = ticket(&human, "todo", "todo").await;
    let ready1 = ticket(&human, "ready1", "ready").await;
    let held = ticket(&human, "held", "ready").await;
    human.update_ticket(held, &UpdateTicket { assignee: Some(Some("someone".into())), ..Default::default() }).await.unwrap();
    let resumable = ticket(&human, "resumable", "in_progress").await;
    let review = ticket(&human, "review", "in_review").await;
    // Move the in_progress one to the front: rank decides, not id.
    human.move_ticket(resumable, &MoveTicket { before: Some(todo), after: None }).await.unwrap();

    let p = wc.poll(&w.id, None).await.unwrap().expect("a ticket");
    assert_eq!(p.ticket.id, resumable);
    assert_eq!(p.ticket.assignee.as_deref(), Some(w.id.as_str()));
    assert_eq!(p.prompt, format!("Resume #{resumable}"));
    assert_eq!(p.repos, ["https://github.com/org/a.git", "https://github.com/org/b.git"]);
    assert_eq!(p.run_timeout, std::time::Duration::from_secs(3600));
    assert_eq!(human.get_ticket(resumable).await.unwrap().assignee.as_deref(), Some(w.id.as_str()));
    let listed = human.list_workers().await.unwrap();
    assert_eq!((listed[0].status.as_str(), listed[0].ticket), ("busy", Some(resumable)));

    // Next: ready1. Held and todo/in_review are skipped. Prompt renders every field.
    let p = wc.poll(&w.id, None).await.unwrap().expect("a ticket");
    assert_eq!(p.ticket.id, ready1);
    assert_eq!(p.prompt, format!("Work on #{ready1} (ready): ready1\nabout ready1"));

    // Nothing left: idle, 204. States without a prompt are never handed out.
    assert!(wc.poll(&w.id, None).await.unwrap().is_none());
    assert_eq!(human.list_workers().await.unwrap()[0].status, "idle");
    assert_eq!(human.get_ticket(review).await.unwrap().assignee, None);
    assert_eq!(human.get_ticket(todo).await.unwrap().assignee, None);

    // Prompts are shared: another agent sees the same queue, with its own run_timeout.
    let (c, cc) = worker(&url, &human, "codex").await;
    assert!(cc.poll(&c.id, None).await.unwrap().is_none());
    let again = ticket(&human, "again", "ready").await;
    let p = cc.poll(&c.id, None).await.unwrap().expect("a ticket");
    assert_eq!((p.ticket.id, p.run_timeout), (again, std::time::Duration::from_secs(7200)));
}

#[tokio::test]
async fn poll_filters_by_agent_and_carries_the_model() {
    let (url, _dir) = serve().await;
    let human = Client::new(&url, TOKEN);
    let (w, wc) = worker(&url, &human, "claude-code").await;
    let (c, cc) = worker(&url, &human, "codex").await;
    let pinned = ticket(&human, "for codex", "ready").await;
    human.update_ticket(pinned, &UpdateTicket { agent: Some(Some("codex".into())), model: Some(Some("o3".into())), ..Default::default() }).await.unwrap();
    let any = ticket(&human, "anyone", "ready").await;
    let pinned_no_model = ticket(&human, "for claude", "ready").await;
    human.update_ticket(pinned_no_model, &UpdateTicket { agent: Some(Some("claude-code".into())), ..Default::default() }).await.unwrap();

    // The first ticket in rank order is pinned to codex, so the claude-code worker skips it.
    let p = wc.poll(&w.id, None).await.unwrap().expect("a ticket");
    assert_eq!((p.ticket.id, p.model), (any, None));
    let p = wc.poll(&w.id, None).await.unwrap().expect("a ticket");
    assert_eq!((p.ticket.id, p.model), (pinned_no_model, None));
    assert!(wc.poll(&w.id, None).await.unwrap().is_none());
    assert_eq!(human.get_ticket(pinned).await.unwrap().assignee, None);

    // The codex worker gets it, with the model.
    let p = cc.poll(&c.id, None).await.unwrap().expect("a ticket");
    assert_eq!((p.ticket.id, p.model.as_deref()), (pinned, Some("o3")));
    assert!(cc.poll(&c.id, None).await.unwrap().is_none());

    // The usage report records the model on the run and the usage, and metrics break it down.
    cc.report_usage(&c.id, &ReportUsage { model: Some("o3".into()), ..usage(pinned, 10, 1, 0.1) }).await.unwrap();
    let u = wc.report_usage(&w.id, &usage(any, 5, 1, 0.05)).await.unwrap();
    assert_eq!(u.model, None);
    let runs = human.get_ticket(pinned).await.unwrap().runs;
    assert_eq!((runs[0].model.as_deref(), runs[0].ended_at.is_some()), (Some("o3"), true));
    assert_eq!(human.get_ticket(any).await.unwrap().runs[0].model, None);
    let per_model: Vec<_> = human.metrics().await.unwrap().per_model.into_iter().map(|b| (b.key.model, b.totals.tokens_in)).collect();
    assert_eq!(per_model, [(None, 5), (Some("o3".into()), 10)]);
}

fn lines(run: Option<i64>, lines: &[&str]) -> ShipLogs {
    ShipLogs { run, lines: lines.iter().map(|l| l.to_string()).collect() }
}

#[tokio::test]
async fn poll_opens_a_run_usage_ends_it_and_logs_are_kept_per_worker_and_run() {
    let (url, _dir) = serve().await;
    let human = Client::new(&url, TOKEN);
    let (w, wc) = worker(&url, &human, "claude-code").await;
    let (w2, wc2) = worker(&url, &human, "claude-code").await;
    let t = ticket(&human, "a", "ready").await;

    wc.ship_logs(&w.id, &lines(None, &["starting"])).await.unwrap();
    let job = wc.poll(&w.id, None).await.unwrap().unwrap();
    let run = job.run;
    let runs = human.get_ticket(t).await.unwrap().runs;
    assert_eq!((runs.len(), runs[0].id, runs[0].worker_id.as_str(), runs[0].ticket_id), (1, run, w.id.as_str(), t));
    assert!(runs[0].ended_at.is_none() && !runs[0].started_at.is_empty());
    assert_eq!(runs[0].agent, "claude-code");

    wc.ship_logs(&w.id, &lines(Some(run), &["a", "b"])).await.unwrap();
    assert_eq!(status(wc2.ship_logs(&w2.id, &lines(Some(run), &["x"])).await), 400, "another worker's run");
    assert_eq!(status(wc2.ship_logs(&w.id, &lines(None, &["x"])).await), 403, "another worker's stream");
    assert_eq!(status(human.ship_logs(&w.id, &lines(None, &["x"])).await), 403);

    wc.report_usage(&w.id, &usage(t, 1, 1, 0.0)).await.unwrap();
    assert!(human.get_ticket(t).await.unwrap().runs[0].ended_at.is_some());
    wc.ship_logs(&w.id, &lines(None, &["idle"])).await.unwrap();

    let all = human.worker_logs(&w.id, None).await.unwrap();
    let view: Vec<_> = all.iter().map(|l| (l.line.as_str(), l.run_id)).collect();
    assert_eq!(view, vec![("starting", None), ("a", Some(run)), ("b", Some(run)), ("idle", None)]);
    assert!(all.windows(2).all(|w| w[0].id < w[1].id));
    let of_run = human.run_logs(run, None).await.unwrap();
    assert_eq!(of_run.iter().map(|l| l.line.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    assert_eq!(human.run_logs(run, Some(of_run[0].id)).await.unwrap().iter().map(|l| l.line.as_str()).collect::<Vec<_>>(), ["b"]);
    assert!(human.worker_logs(&w.id, Some(all.last().unwrap().id)).await.unwrap().is_empty());
    assert!(wc2.run_logs(run, None).await.is_ok(), "any authenticated caller may read");
    assert_eq!(status(human.run_logs(9999, None).await), 404);
    assert_eq!(status(human.worker_logs("w-nope", None).await), 404);

    // A second hand-out opens a second run, listed newest first.
    human.update_ticket(t, &UpdateTicket { state: Some("ready".into()), assignee: Some(None), ..Default::default() }).await.unwrap();
    let job2 = wc2.poll(&w2.id, None).await.unwrap().unwrap();
    let runs = human.get_ticket(t).await.unwrap().runs;
    assert_eq!(runs.iter().map(|r| r.id).collect::<Vec<_>>(), vec![job2.run, run]);
}

#[tokio::test]
async fn poll_skips_ready_tickets_with_unfinished_dependencies() {
    let (url, _dir) = serve().await;
    let human = Client::new(&url, TOKEN);
    let (w, worker) = worker(&url, &human, "claude-code").await;
    let dep = ticket(&human, "dep", "todo").await;
    let t = ticket(&human, "blocked", "ready").await;
    human.add_relation(t, &CreateRelation { r#type: "depends_on".into(), ticket: dep }).await.unwrap();
    assert!(human.get_ticket(t).await.unwrap().blocked);
    assert!(human.list_tickets(&Default::default()).await.unwrap().iter().any(|x| x.id == t && x.blocked));

    // The dependency is held by someone else, so the blocked ticket is the only candidate.
    for state in ["todo", "ready", "in_progress", "failed"] {
        human.update_ticket(dep, &UpdateTicket { state: Some(state.into()), assignee: Some(Some("w-other".into())), ..Default::default() }).await.unwrap();
        assert!(worker.poll(&w.id, None).await.unwrap().is_none(), "dependency in {state} should block");
    }
    // A later, unblocked ticket is handed out ahead of the blocked one.
    let free = ticket(&human, "free", "ready").await;
    assert_eq!(worker.poll(&w.id, None).await.unwrap().unwrap().ticket.id, free);
    human.update_ticket(free, &set_state("done")).await.unwrap();

    human.update_ticket(dep, &set_state("in_review")).await.unwrap();
    assert!(!human.get_ticket(t).await.unwrap().blocked);
    let got = worker.poll(&w.id, None).await.unwrap().unwrap().ticket;
    assert_eq!(got.id, t);
    // Once in_progress, a dependency moving back does not stop the resume.
    human.update_ticket(t, &UpdateTicket { state: Some("in_progress".into()), assignee: Some(None), ..Default::default() }).await.unwrap();
    human.update_ticket(dep, &set_state("todo")).await.unwrap();
    assert_eq!(worker.poll(&w.id, None).await.unwrap().unwrap().ticket.id, t);
}

#[tokio::test]
async fn poll_exclude_is_last_in_line() {
    let (url, _dir) = serve().await;
    let human = Client::new(&url, TOKEN);
    let (w, wc) = worker(&url, &human, "claude-code").await;
    let first = ticket(&human, "first", "ready").await;
    let second = ticket(&human, "second", "ready").await;
    // Other work available: the excluded ticket is skipped.
    assert_eq!(wc.poll(&w.id, Some(first)).await.unwrap().unwrap().ticket.id, second);
    // Only the excluded ticket left: handed out anyway.
    assert_eq!(wc.poll(&w.id, Some(first)).await.unwrap().unwrap().ticket.id, first);
    assert!(wc.poll(&w.id, Some(first)).await.unwrap().is_none());
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
        let (w, wc) = worker(&url, &human, "claude-code").await;
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            wc.poll(&w.id, None).await.unwrap().map(|p| (w.id, p.ticket.id))
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
    let (w, wc) = worker(&url, &human, "claude-code").await;
    let (other, oc) = worker(&url, &human, "claude-code").await;
    let mine = ticket(&human, "mine", "ready").await;
    let theirs = ticket(&human, "theirs", "todo").await;
    assert_eq!(wc.poll(&w.id, None).await.unwrap().unwrap().ticket.id, mine);

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
    let created = oc.create_ticket(&CreateTicket { title: "from worker".into(), ..Default::default() }).await.unwrap();
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
    let (w, wc) = worker(&url, &human, "claude-code").await;
    let id = ticket(&human, "a", "ready").await;
    wc.poll(&w.id, None).await.unwrap().unwrap();

    // Same state: still held.
    let t = wc.update_ticket(id, &set_state("ready")).await.unwrap();
    assert_eq!(t.assignee.as_deref(), Some(w.id.as_str()));
    // Other fields: still held.
    let t = wc.update_ticket(id, &UpdateTicket { description: Some("d".into()), ..Default::default() }).await.unwrap();
    assert_eq!(t.assignee.as_deref(), Some(w.id.as_str()));
    // State change: cleared, and the worker no longer holds it.
    let t = wc.update_ticket(id, &set_state("in_review")).await.unwrap();
    assert_eq!((t.state.as_str(), t.assignee), ("in_review", None));
    assert_eq!(status(wc.update_ticket(id, &set_state("done")).await), 403);
    assert_eq!(human.list_workers().await.unwrap()[0].ticket, None);
    // Explicit assignee in the same patch wins.
    let t = human.update_ticket(id, &UpdateTicket { state: Some("in_review".into()), assignee: Some(Some("bob".into())), ..Default::default() }).await.unwrap();
    assert_eq!((t.state.as_str(), t.assignee.as_deref()), ("in_review", Some("bob")));
    let t = human.update_ticket(id, &set_state("done")).await.unwrap();
    assert_eq!(t.assignee, None);
}

#[tokio::test]
async fn worker_keeps_ticket_it_moves_to_in_progress() {
    let (url, _dir) = serve().await;
    let human = Client::new(&url, TOKEN);
    let (w, wc) = worker(&url, &human, "claude-code").await;
    let (other, oc) = worker(&url, &human, "claude-code").await;
    let id = ticket(&human, "a", "ready").await;
    assert_eq!(wc.poll(&w.id, None).await.unwrap().unwrap().ticket.id, id);

    let t = wc.update_ticket(id, &set_state("in_progress")).await.unwrap();
    assert_eq!((t.state.as_str(), t.assignee.as_deref()), ("in_progress", Some(w.id.as_str())));
    assert!(oc.poll(&other.id, None).await.unwrap().is_none(), "held in_progress ticket is not handed out");
    assert_eq!(status(oc.update_ticket(id, &set_state("done")).await), 403);
    // A human moving it to in_progress still clears it.
    human.update_ticket(id, &set_state("ready")).await.unwrap();
    human.update_ticket(id, &UpdateTicket { assignee: Some(Some(w.id.clone())), ..Default::default() }).await.unwrap();
    assert_eq!(human.update_ticket(id, &set_state("in_progress")).await.unwrap().assignee, None);
    human.update_ticket(id, &UpdateTicket { assignee: Some(Some(w.id.clone())), ..Default::default() }).await.unwrap();
    // Any other state change clears it.
    let t = wc.update_ticket(id, &set_state("in_review")).await.unwrap();
    assert_eq!((t.state.as_str(), t.assignee), ("in_review", None));
    assert_eq!(status(wc.update_ticket(id, &set_state("done")).await), 403);
}

#[tokio::test]
async fn reaper_marks_silent_workers_dead_and_frees_tickets() {
    let timeout = std::time::Duration::from_millis(200);
    let (url, dir) = common::serve(&CONFIG.replace("\"60s\"", "\"200ms\"")).await;
    let pool = orchestrator::db::open(&dir.path().join("test.db")).await.unwrap();
    let human = Client::new(&url, TOKEN);
    let (w, wc) = worker(&url, &human, "claude-code").await;
    wc.register(&w.id).await.unwrap();
    let id = ticket(&human, "a", "in_progress").await;
    assert_eq!(wc.poll(&w.id, None).await.unwrap().unwrap().ticket.id, id);
    // Never registers: reaped from its creation time.
    let (silent, sc) = worker(&url, &human, "claude-code").await;
    let (live, lc) = worker(&url, &human, "claude-code").await;
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
    assert!(t.runs[0].ended_at.is_some(), "reaping ends the worker's open run");

    // Dead tokens no longer authenticate.
    assert_eq!(status(wc.heartbeat(&w.id).await), 401);
    assert_eq!(status(wc.register(&w.id).await), 401);
    assert_eq!(status(wc.poll(&w.id, None).await), 401);
    assert_eq!(status(wc.update_ticket(id, &set_state("done")).await), 401);
    assert_eq!(status(sc.register(&silent.id).await), 401);
    lc.heartbeat(&live.id).await.unwrap();

    // The next poll resumes the ticket.
    let (f, fc) = worker(&url, &human, "claude-code").await;
    fc.register(&f.id).await.unwrap();
    let p = fc.poll(&f.id, None).await.unwrap().expect("the freed ticket");
    assert_eq!((p.ticket.id, p.ticket.assignee.as_deref(), p.prompt.as_str()), (id, Some(f.id.as_str()), format!("Resume #{id}").as_str()));
}

fn usage(ticket_id: i64, tokens_in: i64, tokens_out: i64, cost: f64) -> ReportUsage {
    ReportUsage { ticket_id, tokens_in, tokens_out, cost, model: None }
}

fn totals(t: &Totals) -> (i64, i64, f64, i64, i64) {
    (t.tokens_in, t.tokens_out, t.cost, t.tickets_completed, t.tickets_failed)
}

#[tokio::test]
async fn usage_is_attributed_and_scoped_to_the_reporting_worker() {
    let (url, _dir) = serve().await;
    let human = Client::new(&url, TOKEN);
    let (w, wc) = worker(&url, &human, "codex").await;
    let (other, oc) = worker(&url, &human, "claude-code").await;
    let id = ticket(&human, "a", "ready").await;

    // No need to hold the ticket: usage is reported after the run, which may have released it.
    let u = wc.report_usage(&w.id, &usage(id, 100, 20, 0.25)).await.unwrap();
    assert_eq!((u.ticket_id, u.worker_id.as_str(), u.agent.as_str()), (id, w.id.as_str(), "codex"));
    assert_eq!((u.tokens_in, u.tokens_out, u.cost), (100, 20, 0.25));
    assert!(u.id > 0 && !u.created_at.is_empty());

    assert_eq!(status(oc.report_usage(&w.id, &usage(id, 1, 1, 0.0)).await), 403);
    assert_eq!(status(wc.report_usage(&other.id, &usage(id, 1, 1, 0.0)).await), 403);
    assert_eq!(status(human.report_usage(&w.id, &usage(id, 1, 1, 0.0)).await), 403);
    assert_eq!(status(wc.report_usage(&w.id, &usage(9999, 1, 1, 0.0)).await), 404);

    let m = human.metrics().await.unwrap();
    assert_eq!(totals(&m.totals), (100, 20, 0.25, 0, 0));
    assert_eq!(m.per_ticket.len(), 1);
    assert_eq!((m.per_ticket[0].key.ticket_id, m.per_ticket[0].totals.tokens_in), (id, 100));
    assert_eq!(m.per_worker[0].key.worker_id, w.id);
    assert_eq!(m.per_agent[0].key.agent, "codex");
    // Workers may read metrics too.
    assert_eq!(oc.metrics().await.unwrap().totals.tokens_in, 100);
}

#[tokio::test]
async fn metrics_aggregate_and_count_ticket_states() {
    let (url, _dir) = serve().await;
    let human = Client::new(&url, TOKEN);
    assert_eq!(totals(&human.metrics().await.unwrap().totals), (0, 0, 0.0, 0, 0));
    let (a, ac) = worker(&url, &human, "claude-code").await;
    let (b, bc) = worker(&url, &human, "claude-code").await;
    let t1 = ticket(&human, "t1", "ready").await;
    let t2 = ticket(&human, "t2", "ready").await;
    // Done without any usage: counted in totals only.
    ticket(&human, "t3", "done").await;

    ac.report_usage(&a.id, &usage(t1, 100, 10, 1.0)).await.unwrap();
    ac.report_usage(&a.id, &usage(t1, 200, 20, 2.0)).await.unwrap();
    bc.report_usage(&b.id, &usage(t1, 50, 5, 0.5)).await.unwrap();
    bc.report_usage(&b.id, &usage(t2, 1000, 100, 4.0)).await.unwrap();

    let m = human.metrics().await.unwrap();
    assert_eq!(totals(&m.totals), (1350, 135, 7.5, 1, 0));
    let per_ticket: Vec<_> = m.per_ticket.iter().map(|x| (x.key.ticket_id, totals(&x.totals))).collect();
    assert_eq!(per_ticket, [(t1, (350, 35, 3.5, 0, 0)), (t2, (1000, 100, 4.0, 0, 0))]);
    let per_worker: Vec<_> = m.per_worker.iter().map(|x| (x.key.worker_id.as_str(), totals(&x.totals))).collect();
    let mut expected = vec![(a.id.as_str(), (300, 30, 3.0, 0, 0)), (b.id.as_str(), (1050, 105, 4.5, 0, 0))];
    expected.sort_by(|x, y| x.0.cmp(y.0));
    assert_eq!(per_worker, expected, "ordered by worker id");
    assert_eq!(m.per_agent.len(), 1);
    assert_eq!((m.per_agent[0].key.agent.as_str(), totals(&m.per_agent[0].totals)), ("claude-code", (1350, 135, 7.5, 0, 0)));

    // States decide completed/failed: t1 done, t2 failed. A and B both touched t1; only B touched t2.
    human.update_ticket(t1, &set_state("done")).await.unwrap();
    human.update_ticket(t2, &set_state("failed")).await.unwrap();
    let m = human.metrics().await.unwrap();
    assert_eq!((m.totals.tickets_completed, m.totals.tickets_failed), (2, 1));
    let per_ticket: Vec<_> = m.per_ticket.iter().map(|x| (x.key.ticket_id, x.totals.tickets_completed, x.totals.tickets_failed)).collect();
    assert_eq!(per_ticket, [(t1, 1, 0), (t2, 0, 1)]);
    let by_worker: std::collections::HashMap<_, _> =
        m.per_worker.iter().map(|x| (x.key.worker_id.as_str(), (x.totals.tickets_completed, x.totals.tickets_failed))).collect();
    assert_eq!((by_worker[a.id.as_str()], by_worker[b.id.as_str()]), ((1, 0), (1, 1)));
    assert_eq!((m.per_agent[0].totals.tickets_completed, m.per_agent[0].totals.tickets_failed), (1, 1));
    // Reverting a state reverts the count.
    human.update_ticket(t2, &set_state("in_review")).await.unwrap();
    assert_eq!(human.metrics().await.unwrap().totals.tickets_failed, 0);
}
