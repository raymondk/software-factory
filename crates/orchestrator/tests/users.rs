use api_client::{ApproveUser, Client, CreateComment, CreateTicket, CreateToken, CreateWorker, Error};
use orchestrator::users::hash;
use sqlx::SqlitePool;

mod common;
use common::TOKEN;

async fn serve() -> (String, tempfile::TempDir) {
    common::serve(include_str!("../../../factory.example.toml")).await
}

fn status<T: std::fmt::Debug>(r: Result<T, Error>) -> u16 {
    match r {
        Err(Error::Api { status, .. }) => status,
        other => panic!("expected an API error, got {other:?}"),
    }
}

/// Signs `principal` in as the login endpoint will: a users row (pending unless already there) and a session valid
/// for `hours` from now. Returns the session token.
async fn session(dir: &tempfile::TempDir, principal: &str, hours: i64) -> String {
    let pool = SqlitePool::connect(&format!("sqlite://{}/test.db", dir.path().display())).await.unwrap();
    let now = "strftime('%Y-%m-%dT%H:%M:%fZ', 'now')";
    sqlx::query(&format!("INSERT OR IGNORE INTO users (principal, status, created_at) VALUES (?1, 'pending', {now})")).bind(principal).execute(&pool).await.unwrap();
    let token = format!("session-{principal}-{hours}");
    sqlx::query(&format!("INSERT INTO sessions (token_hash, principal, created_at, expires_at) VALUES (?1, ?2, {now}, strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?3))"))
        .bind(hash(&token))
        .bind(principal)
        .bind(format!("{hours} hours"))
        .execute(&pool)
        .await
        .unwrap();
    token
}

#[tokio::test]
async fn pending_users_see_only_me_until_approved() {
    let (url, dir) = serve().await;
    let admin = Client::new(&url, TOKEN);
    let bob = Client::new(&url, session(&dir, "bob-principal", 1).await);

    let me = bob.me().await.unwrap();
    assert_eq!((me.principal.as_str(), me.name, me.status.as_str()), ("bob-principal", None, "pending"));
    assert_eq!(status(bob.list_tickets(&Default::default()).await), 403);
    assert_eq!(status(bob.create_ticket(&CreateTicket { title: "x".into(), ..Default::default() }).await), 403);
    assert_eq!(status(bob.create_token(&CreateToken { name: "cli".into() }).await), 403);
    assert_eq!(status(bob.list_users().await), 403);

    // Admin sees the pending principal, approves it with a name, and the same session now works.
    let users = admin.list_users().await.unwrap();
    assert_eq!(users.iter().map(|u| (u.principal.as_str(), u.status.as_str())).collect::<Vec<_>>(), [("bob-principal", "pending")]);
    assert_eq!(status(admin.approve_user("bob-principal", &ApproveUser { name: "  ".into() }).await), 400);
    let u = admin.approve_user("bob-principal", &ApproveUser { name: "Bob".into() }).await.unwrap();
    assert_eq!((u.name.as_deref(), u.status.as_str()), (Some("Bob"), "approved"));
    assert!(bob.list_tickets(&Default::default()).await.unwrap().is_empty());
    assert_eq!(bob.me().await.unwrap().name.as_deref(), Some("Bob"));

    // Comments carry the name; the admin token is "admin".
    let t = bob.create_ticket(&CreateTicket { title: "from bob".into(), ..Default::default() }).await.unwrap();
    assert_eq!(bob.add_comment(t.id, &CreateComment { body: "hi".into() }).await.unwrap().author, "Bob");
    assert_eq!(admin.add_comment(t.id, &CreateComment { body: "hi".into() }).await.unwrap().author, "admin");
    // Users are humans: they may edit any ticket and create workers.
    admin.create_ticket(&CreateTicket { title: "from admin".into(), ..Default::default() }).await.unwrap();
    bob.create_worker(&CreateWorker { agent: "claude-code".into(), provider: None }).await.unwrap();
}

#[tokio::test]
async fn admin_preapproves_lists_and_revokes() {
    let (url, dir) = serve().await;
    let admin = Client::new(&url, TOKEN);
    // Approving a principal that never signed in creates it approved.
    let u = admin.approve_user("alice-principal", &ApproveUser { name: "Alice".into() }).await.unwrap();
    assert_eq!((u.name.as_deref(), u.status.as_str()), (Some("Alice"), "approved"));
    let alice = Client::new(&url, session(&dir, "alice-principal", 1).await);
    assert_eq!(alice.me().await.unwrap().status, "approved");
    session(&dir, "bob-principal", 1).await;

    // Users see approved users only; the admin sees everyone. Users may not approve or revoke.
    let names = |us: Vec<api_client::User>| us.into_iter().map(|u| u.principal).collect::<Vec<_>>();
    assert_eq!(names(alice.list_users().await.unwrap()), ["alice-principal"]);
    assert_eq!(names(admin.list_users().await.unwrap()), ["alice-principal", "bob-principal"]);
    assert_eq!(status(alice.approve_user("bob-principal", &ApproveUser { name: "Bob".into() }).await), 403);
    assert_eq!(status(alice.revoke_user("bob-principal").await), 403);
    assert_eq!(status(admin.me().await), 404);

    // Revoking kills the session and the personal tokens.
    let cli = Client::new(&url, alice.create_token(&CreateToken { name: "cli".into() }).await.unwrap().token);
    cli.list_tickets(&Default::default()).await.unwrap();
    assert_eq!(admin.revoke_user("alice-principal").await.unwrap().status, "revoked");
    assert_eq!(status(alice.list_tickets(&Default::default()).await), 401);
    assert_eq!(status(cli.list_tickets(&Default::default()).await), 401);
    assert_eq!(status(admin.revoke_user("nobody").await), 404);
    assert_eq!(admin.list_users().await.unwrap()[0].status, "revoked");
    // Signing in again lands in revoked, not pending; approving reinstates.
    let again = Client::new(&url, session(&dir, "alice-principal", 1).await);
    assert_eq!(again.me().await.unwrap().status, "revoked");
    assert_eq!(status(again.list_tickets(&Default::default()).await), 403);
    admin.approve_user("alice-principal", &ApproveUser { name: "Alice".into() }).await.unwrap();
    again.list_tickets(&Default::default()).await.unwrap();
}

#[tokio::test]
async fn personal_tokens_authenticate_as_their_user_until_deleted() {
    let (url, dir) = serve().await;
    let admin = Client::new(&url, TOKEN);
    admin.approve_user("alice-principal", &ApproveUser { name: "Alice".into() }).await.unwrap();
    admin.approve_user("bob-principal", &ApproveUser { name: "Bob".into() }).await.unwrap();
    let alice = Client::new(&url, session(&dir, "alice-principal", 1).await);
    let bob = Client::new(&url, session(&dir, "bob-principal", 1).await);

    assert_eq!(status(alice.create_token(&CreateToken { name: "".into() }).await), 400);
    let t = alice.create_token(&CreateToken { name: "laptop".into() }).await.unwrap();
    assert!(t.token.len() >= 32 && t.name == "laptop");
    let cli = Client::new(&url, &t.token);
    assert_eq!(cli.me().await.unwrap().name.as_deref(), Some("Alice"));
    let ticket = cli.create_ticket(&CreateTicket { title: "via cli".into(), ..Default::default() }).await.unwrap();
    assert_eq!(cli.add_comment(ticket.id, &CreateComment { body: "hi".into() }).await.unwrap().author, "Alice");

    // Listed without the token; only to its owner.
    let listed = alice.list_tokens().await.unwrap();
    assert_eq!(listed.iter().map(|x| (x.id, x.name.as_str())).collect::<Vec<_>>(), [(t.id, "laptop")]);
    assert!(!serde_json::to_string(&listed).unwrap().contains(&t.token));
    assert!(bob.list_tokens().await.unwrap().is_empty());
    assert_eq!(status(admin.list_tokens().await), 403);
    let (_, worker) = {
        let w = admin.create_worker(&CreateWorker { agent: "claude-code".into(), provider: None }).await.unwrap();
        (w.id.clone(), Client::new(&url, &w.token))
    };
    assert_eq!(status(worker.create_token(&CreateToken { name: "x".into() }).await), 403);

    // Only the owner deletes it; afterwards it no longer authenticates.
    assert_eq!(status(bob.delete_token(t.id).await), 404);
    alice.delete_token(t.id).await.unwrap();
    assert_eq!(status(alice.delete_token(t.id).await), 404);
    assert_eq!(status(cli.list_tickets(&Default::default()).await), 401);
    assert!(alice.list_tokens().await.unwrap().is_empty());
}

#[tokio::test]
async fn expired_sessions_are_rejected() {
    let (url, dir) = serve().await;
    Client::new(&url, TOKEN).approve_user("alice-principal", &ApproveUser { name: "Alice".into() }).await.unwrap();
    let expired = Client::new(&url, session(&dir, "alice-principal", -1).await);
    assert_eq!(status(expired.me().await), 401);
    assert_eq!(status(expired.list_tickets(&Default::default()).await), 401);
    Client::new(&url, session(&dir, "alice-principal", 1).await).me().await.unwrap();
}

#[tokio::test]
async fn owner_is_the_creating_developer_filterable_and_editable() {
    let (url, dir) = serve().await;
    let admin = Client::new(&url, TOKEN);
    admin.approve_user("alice-principal", &ApproveUser { name: "Alice".into() }).await.unwrap();
    admin.approve_user("bob-principal", &ApproveUser { name: "Bob".into() }).await.unwrap();
    let alice = Client::new(&url, session(&dir, "alice-principal", 1).await);
    let bob = Client::new(&url, session(&dir, "bob-principal", 1).await);

    let a = alice.create_ticket(&CreateTicket { title: "alice's".into(), state: Some("ready".into()), ..Default::default() }).await.unwrap();
    let b = bob.create_ticket(&CreateTicket { title: "bob's".into(), ..Default::default() }).await.unwrap();
    let none = admin.create_ticket(&CreateTicket { title: "admin's".into(), ..Default::default() }).await.unwrap();
    assert_eq!((a.owner.as_deref(), b.owner.as_deref(), none.owner), (Some("alice-principal"), Some("bob-principal"), None));
    assert_eq!(admin.get_ticket(a.id).await.unwrap().owner.as_deref(), Some("alice-principal"));

    let ids = |ts: Vec<api_client::Ticket>| ts.into_iter().map(|t| t.id).collect::<Vec<_>>();
    let by_owner = |o: &str| api_client::ListTickets { owner: Some(o.into()), ..Default::default() };
    assert_eq!(ids(bob.list_tickets(&by_owner("alice-principal")).await.unwrap()), [a.id]);
    assert_eq!(ids(admin.list_tickets(&by_owner("nobody")).await.unwrap()), Vec::<i64>::new());

    // A worker holding alice's ticket creates one: it belongs to alice too. A worker holding nothing sets no owner.
    let w = admin.create_worker(&CreateWorker { agent: "claude-code".into(), provider: None }).await.unwrap();
    let worker = Client::new(&url, &w.token);
    worker.register(&w.id).await.unwrap();
    let idle = worker.create_ticket(&CreateTicket { title: "from an idle worker".into(), ..Default::default() }).await.unwrap();
    assert_eq!(idle.owner, None);
    assert_eq!(worker.poll(&w.id, None).await.unwrap().unwrap().ticket.id, a.id);
    let spawned = worker.create_ticket(&CreateTicket { title: "follow-up".into(), ..Default::default() }).await.unwrap();
    assert_eq!(spawned.owner.as_deref(), Some("alice-principal"));

    // Anyone may reassign or clear the owner; an omitted field is untouched.
    let t = bob.update_ticket(a.id, &api_client::UpdateTicket { owner: Some(Some("bob-principal".into())), ..Default::default() }).await.unwrap();
    assert_eq!(t.owner.as_deref(), Some("bob-principal"));
    let t = bob.update_ticket(a.id, &api_client::UpdateTicket { title: Some("renamed".into()), ..Default::default() }).await.unwrap();
    assert_eq!(t.owner.as_deref(), Some("bob-principal"));
    let t = admin.update_ticket(a.id, &api_client::UpdateTicket { owner: Some(None), ..Default::default() }).await.unwrap();
    assert_eq!(t.owner, None);
}
