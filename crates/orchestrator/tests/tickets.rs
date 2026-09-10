use std::time::Duration;

use api_client::{Client, CreateTicket, Error, ListTickets, MoveTicket, UpdateTicket};
use orchestrator::{api, db, AppState};

const TOKEN: &str = "secret";

async fn serve() -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::open(&dir.path().join("test.db")).await.unwrap();
    let router = api::router(AppState { pool, token: TOKEN.into() });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, dir)
}

fn new(title: &str) -> CreateTicket {
    CreateTicket { title: title.into(), description: format!("about {title}") }
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
    let by = |state: Option<&str>, assignee: Option<&str>| ListTickets { state: state.map(Into::into), assignee: assignee.map(Into::into) };
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
