use api_client::{Client, CreateTicket, Error};
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

    let list = client.list_tickets().await.unwrap();
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
    let ids: Vec<_> = client.list_tickets().await.unwrap().into_iter().map(|t| t.id).collect();
    assert_eq!(ids, vec![b.id, a.id]);
}

#[tokio::test]
async fn rejects_bad_token() {
    let (url, _dir) = serve().await;
    for token in ["", "wrong"] {
        let client = Client::new(&url, token);
        match client.list_tickets().await {
            Err(Error::Api { status: 401, .. }) => {}
            other => panic!("expected 401 for token {token:?}, got {other:?}"),
        }
        match client.create_ticket(&new("x")).await {
            Err(Error::Api { status: 401, .. }) => {}
            other => panic!("expected 401 for token {token:?}, got {other:?}"),
        }
    }
    assert!(Client::new(&url, TOKEN).list_tickets().await.unwrap().is_empty());
}
