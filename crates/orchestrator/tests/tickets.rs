use std::time::Duration;

use api_client::{Client, CreateComment, CreateRelation, CreateTicket, Error, ListTickets, MoveTicket, UpdateTicket};

mod common;
use common::TOKEN;

async fn serve() -> (String, tempfile::TempDir) {
    common::serve(include_str!("../../../factory.example.toml")).await
}

fn new(title: &str) -> CreateTicket {
    CreateTicket { title: title.into(), description: format!("about {title}"), ..Default::default() }
}

#[tokio::test]
async fn agents_lists_what_providers_advertise() {
    let (url, _dir) = serve().await;
    let agents = Client::new(&url, TOKEN).agents().await.unwrap();
    let expect = |a: &str, ms: &[&str]| (a.to_string(), ms.iter().map(|m| m.to_string()).collect::<Vec<_>>());
    assert_eq!(agents.into_iter().collect::<Vec<_>>(), vec![expect("claude-code", &["sonnet", "opus"]), expect("codex", &["o3"])]);
}

#[tokio::test]
async fn config_is_served_without_the_token() {
    let (url, _dir) = serve().await;
    let body = reqwest::Client::new().get(format!("{url}/config")).bearer_auth(TOKEN).send().await.unwrap().text().await.unwrap();
    assert!(!body.contains(TOKEN), "{body}");
    let c: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(c["orchestrator"]["heartbeat_timeout"], "1m");
    assert!(c["orchestrator"].get("token").is_none());
    assert!(c.get("providers").is_none(), "providers live in the database");
}

#[tokio::test]
async fn agent_and_model_must_be_advertised_together_by_a_provider() {
    let (url, _dir) = serve().await;
    let client = Client::new(&url, TOKEN);
    bad_request(client.create_ticket(&CreateTicket { agent: Some("nope".into()), ..new("a") }).await, "agent not advertised");
    bad_request(client.create_ticket(&CreateTicket { model: Some("gpt-9".into()), ..new("a") }).await, "model not advertised");
    bad_request(client.create_ticket(&CreateTicket { agent: Some("codex".into()), model: Some("opus".into()), ..new("a") }).await, "not advertised together");
    // Any agent may run opus somewhere; codex only runs o3.
    client.create_ticket(&CreateTicket { model: Some("opus".into()), ..new("a") }).await.unwrap();
    let t = client.create_ticket(&CreateTicket { agent: Some("codex".into()), ..new("b") }).await.unwrap();

    // PATCH validates the merged result: the new model against the existing agent, and the other way round.
    bad_request(client.update_ticket(t.id, &UpdateTicket { model: Some(Some("opus".into())), ..Default::default() }).await, "not advertised together");
    client.update_ticket(t.id, &UpdateTicket { model: Some(Some("o3".into())), ..Default::default() }).await.unwrap();
    bad_request(client.update_ticket(t.id, &UpdateTicket { agent: Some(Some("claude-code".into())), ..Default::default() }).await, "not advertised together");
    let u = client.update_ticket(t.id, &UpdateTicket { agent: Some(Some("claude-code".into())), model: Some(Some("opus".into())), ..Default::default() }).await.unwrap();
    assert_eq!((u.agent.as_deref(), u.model.as_deref()), (Some("claude-code"), Some("opus")));
    // Clearing is always fine, and other fields never trigger the check.
    client.update_ticket(t.id, &UpdateTicket { agent: Some(None), model: Some(None), title: Some("c".into()), ..Default::default() }).await.unwrap();
}

#[tokio::test]
async fn agent_and_model_are_optional_settable_and_clearable() {
    let (url, _dir) = serve().await;
    let client = Client::new(&url, TOKEN);
    let t = client.create_ticket(&new("a")).await.unwrap();
    assert_eq!((t.agent, t.model), (None, None));
    let t = client.create_ticket(&CreateTicket { agent: Some("codex".into()), model: Some("o3".into()), ..new("b") }).await.unwrap();
    assert_eq!((t.agent.as_deref(), t.model.as_deref()), (Some("codex"), Some("o3")));
    let got = client.get_ticket(t.id).await.unwrap();
    assert_eq!((got.agent.as_deref(), got.model.as_deref()), (Some("codex"), Some("o3")));
    assert!(client.list_tickets(&Default::default()).await.unwrap().iter().any(|x| x.id == t.id && x.model.as_deref() == Some("o3")));

    // Omitted: untouched. Set: replaced. Null: cleared.
    let u = client.update_ticket(t.id, &UpdateTicket { title: Some("b2".into()), ..Default::default() }).await.unwrap();
    assert_eq!((u.agent.as_deref(), u.model.as_deref()), (Some("codex"), Some("o3")));
    let u = client.update_ticket(t.id, &UpdateTicket { agent: Some(Some("claude-code".into())), model: Some(None), ..Default::default() }).await.unwrap();
    assert_eq!((u.agent.as_deref(), u.model), (Some("claude-code"), None));
    let u = client.update_ticket(t.id, &UpdateTicket { agent: Some(None), model: Some(Some("opus".into())), ..Default::default() }).await.unwrap();
    assert_eq!((u.agent, u.model.as_deref()), (None, Some("opus")));
}

#[tokio::test]
async fn create_list_get() {
    let (url, _dir) = serve().await;
    let client = Client::new(&url, TOKEN);

    let a = client.create_ticket(&new("a")).await.unwrap();
    assert_eq!(a.state, "todo");
    assert_eq!(a.rank, 1.0);
    assert_eq!(a.description, "about a");
    assert_eq!(a.assignee, None);
    assert!(a.links.is_empty() && a.comments.is_empty());

    let b = client.create_ticket(&new("b")).await.unwrap();
    let c = client.create_ticket(&new("c")).await.unwrap();
    assert_eq!((b.rank, c.rank), (2.0, 3.0));

    let list = client.list_tickets(&Default::default()).await.unwrap();
    assert_eq!(list.iter().map(|t| t.id).collect::<Vec<_>>(), vec![a.id, b.id, c.id]);
    assert!(list.windows(2).all(|w| w[0].rank < w[1].rank));

    let got = client.get_ticket(b.id).await.unwrap();
    assert_eq!(got.title, "b");

    let ready = client.create_ticket(&CreateTicket { state: Some("ready".into()), ..new("d") }).await.unwrap();
    assert_eq!(ready.state, "ready");
    match client.create_ticket(&CreateTicket { state: Some("bogus".into()), ..new("e") }).await {
        Err(Error::Api { status: 400, .. }) => {}
        other => panic!("expected 400, got {other:?}"),
    }

    match client.get_ticket(9999).await {
        Err(Error::Api { status: 404, .. }) => {}
        other => panic!("expected 404, got {other:?}"),
    }
}

#[tokio::test]
async fn list_orders_by_rank_not_id() {
    let (url, _dir) = serve().await;
    let client = Client::new(&url, TOKEN);
    let a = client.create_ticket(&new("a")).await.unwrap();
    let b = client.create_ticket(&new("b")).await.unwrap();
    // Rank of the first ticket pushed past the second (as a later move endpoint would do).
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}/test.db", _dir.path().display())).await.unwrap();
    sqlx::query("UPDATE tickets SET rank = 5.0 WHERE id = ?1").bind(a.id).execute(&pool).await.unwrap();
    let ids: Vec<_> = client.list_tickets(&Default::default()).await.unwrap().into_iter().map(|t| t.id).collect();
    assert_eq!(ids, vec![b.id, a.id]);
}

#[tokio::test]
async fn rejects_bad_token() {
    let (url, _dir) = serve().await;
    for token in ["", "wrong"] {
        let client = Client::new(&url, token);
        match client.list_tickets(&Default::default()).await {
            Err(Error::Api { status: 401, .. }) => {}
            other => panic!("expected 401 for token {token:?}, got {other:?}"),
        }
        match client.create_ticket(&new("x")).await {
            Err(Error::Api { status: 401, .. }) => {}
            other => panic!("expected 401 for token {token:?}, got {other:?}"),
        }
    }
    assert!(Client::new(&url, TOKEN).list_tickets(&Default::default()).await.unwrap().is_empty());
}

fn assignee(a: Option<&str>) -> UpdateTicket {
    UpdateTicket { assignee: Some(a.map(String::from)), ..Default::default() }
}

/// `updated_at` has millisecond resolution; make sure a bump is observable.
async fn tick() {
    tokio::time::sleep(Duration::from_millis(5)).await;
}

#[tokio::test]
async fn update_each_field() {
    let (url, _dir) = serve().await;
    let client = Client::new(&url, TOKEN);
    let mut t = client.create_ticket(&new("a")).await.unwrap();

    let cases = [
        UpdateTicket { title: Some("b".into()), ..Default::default() },
        UpdateTicket { description: Some("desc".into()), ..Default::default() },
        UpdateTicket { state: Some("in_progress".into()), ..Default::default() },
        assignee(Some("w1")),
        UpdateTicket { links: Some(vec!["https://x/pr/1".into()]), ..Default::default() },
    ];
    for req in cases {
        tick().await;
        let u = client.update_ticket(t.id, &req).await.unwrap();
        assert!(u.updated_at > t.updated_at, "{req:?}");
        assert_eq!(u.created_at, t.created_at);
        assert_eq!(u.rank, t.rank);
        t = u;
    }
    assert_eq!(t.title, "b");
    assert_eq!(t.description, "desc");
    assert_eq!(t.state, "in_progress");
    assert_eq!(t.assignee.as_deref(), Some("w1"));
    assert_eq!(t.links, ["https://x/pr/1"]);
    // Persisted, not just echoed.
    let got = client.get_ticket(t.id).await.unwrap();
    assert_eq!(got.links, t.links);
    assert_eq!(got.description, "desc");
}

#[tokio::test]
async fn omitted_fields_untouched_and_assignee_clear() {
    let (url, _dir) = serve().await;
    let client = Client::new(&url, TOKEN);
    let t = client.create_ticket(&new("a")).await.unwrap();
    let t = client.update_ticket(t.id, &assignee(Some("w1"))).await.unwrap();

    // Empty patch: nothing but updated_at changes.
    let u = client.update_ticket(t.id, &UpdateTicket::default()).await.unwrap();
    assert_eq!((u.title.as_str(), u.description.as_str(), u.state.as_str(), u.assignee.as_deref()), ("a", "about a", "todo", Some("w1")));

    let u = client.update_ticket(t.id, &assignee(None)).await.unwrap();
    assert_eq!(u.assignee, None);
    assert_eq!(u.title, "a");
}

#[tokio::test]
async fn rejects_invalid_state_and_unknown_id() {
    let (url, _dir) = serve().await;
    let client = Client::new(&url, TOKEN);
    let t = client.create_ticket(&new("a")).await.unwrap();
    for state in ["", "bogus", "Done"] {
        match client.update_ticket(t.id, &UpdateTicket { state: Some(state.into()), ..Default::default() }).await {
            Err(Error::Api { status: 400, .. }) => {}
            other => panic!("expected 400 for {state:?}, got {other:?}"),
        }
    }
    assert_eq!(client.get_ticket(t.id).await.unwrap().state, "todo");
    for state in ["todo", "ready", "in_progress", "in_review", "failed", "done"] {
        let u = client.update_ticket(t.id, &UpdateTicket { state: Some(state.into()), ..Default::default() }).await.unwrap();
        assert_eq!(u.state, state);
    }
    match client.update_ticket(9999, &UpdateTicket::default()).await {
        Err(Error::Api { status: 404, .. }) => {}
        other => panic!("expected 404, got {other:?}"),
    }
}

#[tokio::test]
async fn list_filters() {
    let (url, _dir) = serve().await;
    let client = Client::new(&url, TOKEN);
    let a = client.create_ticket(&new("a")).await.unwrap();
    let b = client.create_ticket(&new("b")).await.unwrap();
    let c = client.create_ticket(&new("c")).await.unwrap();
    client.update_ticket(a.id, &UpdateTicket { state: Some("ready".into()), ..Default::default() }).await.unwrap();
    client.update_ticket(c.id, &UpdateTicket { state: Some("ready".into()), assignee: Some(Some("w1".into())), ..Default::default() }).await.unwrap();
    client.update_ticket(b.id, &assignee(Some("w2"))).await.unwrap();

    let ids = |list: Vec<api_client::Ticket>| list.into_iter().map(|t| t.id).collect::<Vec<_>>();
    let by = |state: Option<&str>, assignee: Option<&str>| ListTickets { state: state.map(Into::into), assignee: assignee.map(Into::into), ..Default::default() };
    assert_eq!(ids(client.list_tickets(&by(Some("ready"), None)).await.unwrap()), vec![a.id, c.id]);
    assert_eq!(ids(client.list_tickets(&by(Some("todo"), None)).await.unwrap()), vec![b.id]);
    assert_eq!(ids(client.list_tickets(&by(None, Some("w1"))).await.unwrap()), vec![c.id]);
    assert_eq!(ids(client.list_tickets(&by(Some("ready"), Some("w2"))).await.unwrap()), Vec::<i64>::new());
    assert_eq!(ids(client.list_tickets(&by(None, None)).await.unwrap()), vec![a.id, b.id, c.id]);
}

fn before(id: i64) -> MoveTicket {
    MoveTicket { before: Some(id), ..Default::default() }
}

fn after(id: i64) -> MoveTicket {
    MoveTicket { after: Some(id), ..Default::default() }
}

async fn order(client: &Client) -> Vec<i64> {
    let list = client.list_tickets(&Default::default()).await.unwrap();
    assert!(list.windows(2).all(|w| w[0].rank < w[1].rank), "ranks must be strictly increasing");
    list.into_iter().map(|t| t.id).collect()
}

#[tokio::test]
async fn move_before_after_front_back() {
    let (url, _dir) = serve().await;
    let client = Client::new(&url, TOKEN);
    let mut ids = vec![];
    for name in ["a", "b", "c", "d"] {
        ids.push(client.create_ticket(&new(name)).await.unwrap().id);
    }
    let [a, b, c, d] = ids[..] else { unreachable!() };

    let moved = client.move_ticket(d, &before(b)).await.unwrap();
    assert_eq!(moved.id, d);
    assert_eq!(moved.rank, 1.5);
    assert_eq!(order(&client).await, vec![a, d, b, c]);

    assert_eq!(client.move_ticket(a, &after(b)).await.unwrap().rank, 2.5);
    assert_eq!(order(&client).await, vec![d, b, a, c]);

    // To the front and to the back extend past the boundary.
    assert_eq!(client.move_ticket(c, &before(d)).await.unwrap().rank, 0.5);
    assert_eq!(order(&client).await, vec![c, d, b, a]);
    assert_eq!(client.move_ticket(c, &after(a)).await.unwrap().rank, 3.5);
    assert_eq!(order(&client).await, vec![d, b, a, c]);

    // Moving next to an immediate neighbour keeps the order.
    client.move_ticket(b, &after(d)).await.unwrap();
    assert_eq!(order(&client).await, vec![d, b, a, c]);

    // Other fields survive; updated_at is bumped.
    let t = client.get_ticket(c).await.unwrap();
    assert_eq!((t.title.as_str(), t.state.as_str()), ("c", "todo"));
    assert!(t.updated_at > t.created_at);
}

#[tokio::test]
async fn repeated_moves_renormalize() {
    let (url, _dir) = serve().await;
    let client = Client::new(&url, TOKEN);
    let a = client.create_ticket(&new("a")).await.unwrap().id;
    let b = client.create_ticket(&new("b")).await.unwrap().id;
    let c = client.create_ticket(&new("c")).await.unwrap().id;
    let d = client.create_ticket(&new("d")).await.unwrap().id;
    client.move_ticket(b, &after(d)).await.unwrap();
    // Alternately squeeze c and d into the gap right after a; each move halves it until renormalization resets it.
    let mut min_rank_diff = f64::MAX;
    for i in 0..60 {
        let (mover, other) = if i % 2 == 0 { (c, d) } else { (d, c) };
        client.move_ticket(mover, &after(a)).await.unwrap();
        assert_eq!(order(&client).await, vec![a, mover, other, b], "iteration {i}");
        let list = client.list_tickets(&Default::default()).await.unwrap();
        min_rank_diff = min_rank_diff.min(list[1].rank - list[0].rank);
    }
    assert!(min_rank_diff >= 5e-7, "gap collapsed to {min_rank_diff}");
    let list = client.list_tickets(&Default::default()).await.unwrap();
    assert!(list.iter().all(|t| t.rank < 10.0), "renormalization keeps ranks small: {:?}", list.iter().map(|t| t.rank).collect::<Vec<_>>());
}

#[tokio::test]
async fn move_errors() {
    let (url, _dir) = serve().await;
    let client = Client::new(&url, TOKEN);
    let a = client.create_ticket(&new("a")).await.unwrap().id;
    let b = client.create_ticket(&new("b")).await.unwrap().id;
    let bad = [
        (a, MoveTicket::default(), 400),
        (a, MoveTicket { before: Some(b), after: Some(b) }, 400),
        (a, before(a), 400),
        (a, after(a), 400),
        (a, before(9999), 404),
        (9999, after(a), 404),
    ];
    for (id, req, status) in bad {
        match client.move_ticket(id, &req).await {
            Err(Error::Api { status: s, .. }) if s == status => {}
            other => panic!("expected {status} for {id} {req:?}, got {other:?}"),
        }
    }
    assert_eq!(order(&client).await, vec![a, b]);
}

fn comment(body: &str) -> CreateComment {
    CreateComment { body: body.into() }
}

#[tokio::test]
async fn comments_add_list_resolve() {
    let (url, _dir) = serve().await;
    let client = Client::new(&url, TOKEN);
    let t = client.create_ticket(&new("a")).await.unwrap();
    let other = client.create_ticket(&new("b")).await.unwrap();
    assert!(client.list_comments(t.id).await.unwrap().is_empty());

    tick().await;
    let first = client.add_comment(t.id, &comment("first")).await.unwrap();
    assert_eq!((first.ticket_id, first.author.as_str(), first.body.as_str(), first.resolved), (t.id, "admin", "first", false));
    let commented = client.get_ticket(t.id).await.unwrap().updated_at;
    assert!(commented > t.updated_at, "a comment bumps updated_at");
    tick().await;
    let second = client.add_comment(t.id, &comment("second")).await.unwrap();
    client.add_comment(other.id, &comment("elsewhere")).await.unwrap();
    assert!(second.created_at > first.created_at);

    let ids = |list: Vec<api_client::Comment>| list.into_iter().map(|c| c.id).collect::<Vec<_>>();
    assert_eq!(ids(client.list_comments(t.id).await.unwrap()), vec![first.id, second.id]);

    tick().await;
    let resolved = client.resolve_comment(t.id, first.id).await.unwrap();
    assert!(resolved.resolved && resolved.id == first.id);
    // Stays in the thread, and the ticket embeds it.
    let got = client.get_ticket(t.id).await.unwrap();
    assert!(got.updated_at > commented, "resolving bumps updated_at");
    assert_eq!(got.comments.iter().map(|c| (c.id, c.resolved)).collect::<Vec<_>>(), vec![(first.id, true), (second.id, false)]);
    assert_eq!(got.comments[1].body, "second");
    tick().await;
    let back = client.unresolve_comment(t.id, first.id).await.unwrap();
    assert!(!back.resolved && back.id == first.id);
    let got2 = client.get_ticket(t.id).await.unwrap();
    assert!(!got2.comments[0].resolved);
    assert!(got2.updated_at > got.updated_at, "unresolving bumps updated_at");
    // The list endpoint does not embed threads.
    assert!(client.list_tickets(&Default::default()).await.unwrap().iter().all(|t| t.comments.is_empty()));
}

#[tokio::test]
async fn comment_errors() {
    let (url, _dir) = serve().await;
    let client = Client::new(&url, TOKEN);
    let t = client.create_ticket(&new("a")).await.unwrap();
    let other = client.create_ticket(&new("b")).await.unwrap();
    let c = client.add_comment(t.id, &comment("x")).await.unwrap();
    let status = |r: Result<api_client::Comment, Error>| match r {
        Err(Error::Api { status, .. }) => status,
        other => panic!("expected error, got {other:?}"),
    };
    assert_eq!(status(client.add_comment(t.id, &comment("")).await), 400);
    assert_eq!(status(client.add_comment(t.id, &comment("  \n")).await), 400);
    assert_eq!(status(client.add_comment(9999, &comment("x")).await), 404);
    assert_eq!(status(client.resolve_comment(t.id, 9999).await), 404);
    assert_eq!(status(client.resolve_comment(9999, c.id).await), 404);
    assert_eq!(status(client.resolve_comment(other.id, c.id).await), 404);
    assert_eq!(status(client.unresolve_comment(other.id, c.id).await), 404);
    match client.list_comments(9999).await {
        Err(Error::Api { status: 404, .. }) => {}
        other => panic!("expected 404, got {other:?}"),
    }
    assert_eq!(client.list_comments(t.id).await.unwrap().len(), 1);
    assert!(!client.get_ticket(t.id).await.unwrap().comments[0].resolved);
}

fn relation(kind: &str, ticket: i64) -> CreateRelation {
    CreateRelation { r#type: kind.into(), ticket }
}

fn bad_request<T: std::fmt::Debug>(r: Result<T, Error>, msg: &str) {
    match r {
        Err(Error::Api { status: 400, body }) => assert!(body.contains(msg), "{body}"),
        other => panic!("expected 400 {msg:?}, got {other:?}"),
    }
}

#[tokio::test]
async fn relations_are_created_listed_and_deleted() {
    let (url, _dir) = serve().await;
    let client = Client::new(&url, TOKEN);
    let a = client.create_ticket(&new("a")).await.unwrap().id;
    let bt = client.create_ticket(&CreateTicket { state: Some("done".into()), ..new("b") }).await.unwrap();
    let b = bt.id;
    let c = client.create_ticket(&new("c")).await.unwrap().id;

    tick().await;
    let linked = client.add_relation(a, &relation("depends_on", b)).await.unwrap();
    assert!(linked.updated_at > linked.created_at, "relating bumps updated_at");
    let b_linked = client.get_ticket(b).await.unwrap().updated_at;
    assert!(b_linked > bt.updated_at, "on both tickets");
    let t = client.add_relation(a, &relation("related_to", c)).await.unwrap();
    let view: Vec<_> = t.relations.iter().map(|r| (r.r#type.as_str(), r.ticket, r.state.as_str(), r.satisfied)).collect();
    assert_eq!(view, vec![("depends_on", b, "done", Some(true)), ("related_to", c, "todo", None)]);
    assert_eq!(t.relations[0].title, "b");

    // Seen from the other side: b is depended on, c is related (stored once, shown on both).
    let rb = client.get_ticket(b).await.unwrap().relations;
    assert_eq!((rb[0].r#type.as_str(), rb[0].ticket, rb[0].satisfied), ("blocks", a, None));
    let rc = client.get_ticket(c).await.unwrap().relations;
    assert_eq!((rc[0].r#type.as_str(), rc[0].ticket), ("related_to", a));
    // The list leaves relations empty, like comments.
    assert!(client.list_tickets(&Default::default()).await.unwrap().iter().all(|t| t.relations.is_empty()));

    // related_to is removable from either side.
    assert!(client.remove_relation(c, "related_to", a).await.unwrap().relations.is_empty());
    assert!(client.get_ticket(a).await.unwrap().relations.iter().all(|r| r.r#type == "depends_on"));
    tick().await;
    assert!(client.remove_relation(a, "depends_on", b).await.unwrap().relations.is_empty());
    assert!(client.get_ticket(b).await.unwrap().updated_at > b_linked, "unrelating bumps updated_at on both tickets");
    match client.remove_relation(a, "depends_on", b).await {
        Err(Error::Api { status: 404, .. }) => {}
        other => panic!("expected 404, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_duplicate_self_and_cyclic_relations() {
    let (url, _dir) = serve().await;
    let client = Client::new(&url, TOKEN);
    let a = client.create_ticket(&new("a")).await.unwrap().id;
    let b = client.create_ticket(&new("b")).await.unwrap().id;
    let c = client.create_ticket(&new("c")).await.unwrap().id;

    bad_request(client.add_relation(a, &relation("depends_on", a)).await, "itself");
    bad_request(client.add_relation(a, &relation("blocks", b)).await, "invalid relation type");
    bad_request(client.add_relation(a, &relation("depends_on", 9999)).await, "not found");

    client.add_relation(a, &relation("depends_on", b)).await.unwrap();
    bad_request(client.add_relation(a, &relation("depends_on", b)).await, "already exists");
    client.add_relation(a, &relation("related_to", b)).await.unwrap();
    bad_request(client.add_relation(b, &relation("related_to", a)).await, "already exists");

    // b -> c, then c -> a would close a -> b -> c -> a.
    client.add_relation(b, &relation("depends_on", c)).await.unwrap();
    bad_request(client.add_relation(c, &relation("depends_on", a)).await, "cycle");
    bad_request(client.add_relation(b, &relation("depends_on", a)).await, "cycle");
    // The reverse direction as related_to is fine.
    client.add_relation(c, &relation("related_to", a)).await.unwrap();
}
