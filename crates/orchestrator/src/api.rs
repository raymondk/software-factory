use api_client::{
    Breakdown, Comment, CreateComment, CreateTicket, CreateWorker, ListTickets, Metrics, MoveTicket, NewWorker, PollResponse, ReportUsage, Ticket, TicketKey, Totals,
    UpdateTicket, Usage, Worker, WorkerKey, WorkerTypeKey,
};
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use sqlx::{Connection, SqliteExecutor};

use crate::config::Config;
use crate::{AppState, STATES};

const UI: &str = include_str!("../ui/index.html");
/// Neighbouring ranks closer than this trigger renormalization.
const MIN_GAP: f64 = 1e-6;

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/tickets", get(list_tickets).post(create_ticket))
        .route("/tickets/{id}", get(get_ticket).patch(update_ticket))
        .route("/tickets/{id}/move", post(move_ticket))
        .route("/tickets/{id}/comments", get(list_comments).post(add_comment))
        .route("/tickets/{id}/comments/{cid}/resolve", post(resolve_comment))
        .route("/workers", get(list_workers).post(create_worker))
        .route("/workers/{id}/register", post(register))
        .route("/workers/{id}/heartbeat", post(heartbeat))
        .route("/workers/{id}/poll", post(poll))
        .route("/workers/{id}/usage", post(report_usage))
        .route("/metrics", get(metrics))
        .layer(middleware::from_fn_with_state(state.clone(), auth));
    Router::new().route("/", get(ui)).merge(api).with_state(state)
}

/// Who is making the request, as established by `auth`. Handlers read it via `Extension<Caller>`.
#[derive(Clone, Debug)]
pub enum Caller {
    Human,
    /// A worker, by id. May only act as itself and on tickets it holds.
    Worker(String),
}

impl Caller {
    fn name(&self) -> &str {
        match self {
            Caller::Human => "human",
            Caller::Worker(id) => id,
        }
    }

    /// Worker-scoped endpoints: only that worker may call them.
    fn require_worker(&self, id: &str) -> Result<(), ApiError> {
        match self {
            Caller::Worker(w) if w == id => Ok(()),
            _ => Err(ApiError::Forbidden),
        }
    }
}

async fn auth(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_string();
    let caller = if token == state.config.orchestrator.token {
        Caller::Human
    } else {
        let worker: Result<Option<(String,)>, _> = sqlx::query_as("SELECT id FROM workers WHERE token = ?1 AND status != 'dead'").bind(&token).fetch_optional(&state.pool).await;
        match worker {
            Ok(Some((id,))) => Caller::Worker(id),
            Ok(None) => return ApiError::Unauthorized.into_response(),
            Err(e) => return ApiError::Db(e).into_response(),
        }
    };
    req.extensions_mut().insert(caller);
    next.run(req).await
}

async fn ui(State(state): State<AppState>) -> Html<String> {
    let token = serde_json::to_string(&state.config.orchestrator.token).unwrap().replace("</", "<\\/");
    Html(UI.replace("__TOKEN__", &token))
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    title: String,
    description: String,
    state: String,
    rank: f64,
    assignee: Option<String>,
    created_at: String,
    updated_at: String,
    links: String,
}

impl From<Row> for Ticket {
    fn from(r: Row) -> Ticket {
        Ticket {
            id: r.id,
            title: r.title,
            description: r.description,
            state: r.state,
            rank: r.rank,
            assignee: r.assignee,
            created_at: r.created_at,
            updated_at: r.updated_at,
            links: serde_json::from_str(&r.links).unwrap_or_default(),
            comments: vec![],
        }
    }
}

#[derive(sqlx::FromRow)]
struct CommentRow {
    id: i64,
    ticket_id: i64,
    author: String,
    body: String,
    created_at: String,
    resolved: bool,
}

impl From<CommentRow> for Comment {
    fn from(r: CommentRow) -> Comment {
        Comment { id: r.id, ticket_id: r.ticket_id, author: r.author, body: r.body, created_at: r.created_at, resolved: r.resolved }
    }
}

const COLUMNS: &str = "id, title, description, state, rank, assignee, created_at, updated_at, links";
const COMMENT_COLUMNS: &str = "id, ticket_id, author, body, created_at, resolved";
const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%fZ', 'now')";

async fn create_ticket(State(state): State<AppState>, Json(req): Json<CreateTicket>) -> Result<(StatusCode, Json<Ticket>), ApiError> {
    if req.title.trim().is_empty() {
        return Err(ApiError::BadRequest("title must not be empty"));
    }
    let row: Row = sqlx::query_as(&format!(
        "INSERT INTO tickets (title, description, state, rank, created_at, updated_at) \
         VALUES (?1, ?2, 'todo', (SELECT COALESCE(MAX(rank), 0) + 1 FROM tickets), {NOW}, {NOW}) \
         RETURNING {COLUMNS}"
    ))
    .bind(&req.title)
    .bind(&req.description)
    .fetch_one(&state.pool)
    .await?;
    Ok((StatusCode::CREATED, Json(row.into())))
}

// The list leaves `comments` empty; only GET /tickets/{id} embeds the thread, to avoid a query per ticket.
async fn list_tickets(State(state): State<AppState>, Query(filter): Query<ListTickets>) -> Result<Json<Vec<Ticket>>, ApiError> {
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM tickets WHERE (?1 IS NULL OR state = ?1) AND (?2 IS NULL OR assignee = ?2) \
         ORDER BY rank ASC, id ASC"
    ))
    .bind(&filter.state)
    .bind(&filter.assignee)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows.into_iter().map(Ticket::from).collect()))
}

async fn get_ticket(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<Ticket>, ApiError> {
    let row: Option<Row> = sqlx::query_as(&format!("SELECT {COLUMNS} FROM tickets WHERE id = ?1"))
        .bind(id)
        .fetch_optional(&state.pool)
        .await?;
    let mut ticket: Ticket = row.ok_or(ApiError::NotFound)?.into();
    ticket.comments = comments_of(&state, id).await?;
    Ok(Json(ticket))
}

async fn comments_of(state: &AppState, ticket_id: i64) -> Result<Vec<Comment>, ApiError> {
    let rows: Vec<CommentRow> =
        sqlx::query_as(&format!("SELECT {COMMENT_COLUMNS} FROM comments WHERE ticket_id = ?1 ORDER BY created_at ASC, id ASC"))
            .bind(ticket_id)
            .fetch_all(&state.pool)
            .await?;
    Ok(rows.into_iter().map(Comment::from).collect())
}

async fn ticket_exists(state: &AppState, id: i64) -> Result<(), ApiError> {
    let found: Option<(i64,)> = sqlx::query_as("SELECT id FROM tickets WHERE id = ?1").bind(id).fetch_optional(&state.pool).await?;
    found.map(|_| ()).ok_or(ApiError::NotFound)
}

async fn list_comments(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<Vec<Comment>>, ApiError> {
    ticket_exists(&state, id).await?;
    Ok(Json(comments_of(&state, id).await?))
}

async fn add_comment(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<i64>,
    Json(req): Json<CreateComment>,
) -> Result<(StatusCode, Json<Comment>), ApiError> {
    if req.body.trim().is_empty() {
        return Err(ApiError::BadRequest("body must not be empty"));
    }
    ticket_exists(&state, id).await?;
    let row: CommentRow = sqlx::query_as(&format!(
        "INSERT INTO comments (ticket_id, author, body, created_at) VALUES (?1, ?2, ?3, {NOW}) RETURNING {COMMENT_COLUMNS}"
    ))
    .bind(id)
    .bind(caller.name())
    .bind(&req.body)
    .fetch_one(&state.pool)
    .await?;
    Ok((StatusCode::CREATED, Json(row.into())))
}

async fn resolve_comment(State(state): State<AppState>, Path((id, cid)): Path<(i64, i64)>) -> Result<Json<Comment>, ApiError> {
    let row: Option<CommentRow> =
        sqlx::query_as(&format!("UPDATE comments SET resolved = 1 WHERE id = ?1 AND ticket_id = ?2 RETURNING {COMMENT_COLUMNS}"))
            .bind(cid)
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    row.map(|r| Json(r.into())).ok_or(ApiError::NotFound)
}

/// Spec 3.4: a ticket may be modified by a human or by the worker holding it.
async fn authorize(caller: &Caller, db: impl SqliteExecutor<'_>, id: i64) -> Result<(), ApiError> {
    let row: Option<(Option<String>,)> = sqlx::query_as("SELECT assignee FROM tickets WHERE id = ?1").bind(id).fetch_optional(db).await?;
    let (assignee,) = row.ok_or(ApiError::NotFound)?;
    match caller {
        Caller::Human => Ok(()),
        Caller::Worker(w) if assignee.as_deref() == Some(w) => Ok(()),
        Caller::Worker(_) => Err(ApiError::Forbidden),
    }
}

async fn update_ticket(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateTicket>,
) -> Result<Json<Ticket>, ApiError> {
    if req.title.as_deref().is_some_and(|t| t.trim().is_empty()) {
        return Err(ApiError::BadRequest("title must not be empty"));
    }
    if req.state.as_deref().is_some_and(|s| !STATES.contains(&s)) {
        return Err(ApiError::BadRequest("invalid state"));
    }
    authorize(&caller, &state.pool, id).await?;
    // Spec 3.2: a state change clears the assignee unless the same patch sets it.
    let row: Option<Row> = sqlx::query_as(&format!(
        "UPDATE tickets SET title = COALESCE(?2, title), description = COALESCE(?3, description), \
         state = COALESCE(?4, state), \
         assignee = CASE WHEN ?5 THEN ?6 WHEN ?4 IS NOT NULL AND ?4 != state THEN NULL ELSE assignee END, \
         links = COALESCE(?7, links), updated_at = {NOW} WHERE id = ?1 RETURNING {COLUMNS}"
    ))
    .bind(id)
    .bind(&req.title)
    .bind(&req.description)
    .bind(&req.state)
    .bind(req.assignee.is_some())
    .bind(req.assignee.clone().flatten())
    .bind(req.links.as_ref().map(|l| serde_json::to_string(l).unwrap()))
    .fetch_optional(&state.pool)
    .await?;
    row.map(|r| Json(r.into())).ok_or(ApiError::NotFound)
}

async fn move_ticket(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<i64>,
    Json(req): Json<MoveTicket>,
) -> Result<Json<Ticket>, ApiError> {
    let (target, before) = match (req.before, req.after) {
        (Some(t), None) => (t, true),
        (None, Some(t)) => (t, false),
        _ => return Err(ApiError::BadRequest("exactly one of before or after is required")),
    };
    if target == id {
        return Err(ApiError::BadRequest("cannot move a ticket relative to itself"));
    }
    let mut conn = state.pool.acquire().await?;
    // IMMEDIATE takes the write lock up front so concurrent moves see each other's ranks.
    let mut tx = conn.begin_with("BEGIN IMMEDIATE").await?;
    authorize(&caller, &mut *tx, id).await?;
    let mut order: Vec<(i64, f64)> = sqlx::query_as("SELECT id, rank FROM tickets ORDER BY rank ASC, id ASC").fetch_all(&mut *tx).await?;
    if !order.iter().any(|(i, _)| *i == id) || !order.iter().any(|(i, _)| *i == target) {
        return Err(ApiError::NotFound);
    }
    // Ranks bounding the slot, with the moved ticket taken out of the order.
    let gap = |order: &[(i64, f64)]| {
        let rest: Vec<f64> = order.iter().filter(|(i, _)| *i != id).map(|(_, r)| *r).collect();
        let pos = order.iter().filter(|(i, _)| *i != id).position(|(i, _)| *i == target).unwrap();
        if before { (pos.checked_sub(1).map(|p| rest[p]), Some(rest[pos])) } else { (Some(rest[pos]), rest.get(pos + 1).copied()) }
    };
    let (mut lo, mut hi) = gap(&order);
    if matches!((lo, hi), (Some(lo), Some(hi)) if hi - lo < MIN_GAP) {
        sqlx::query(
            "UPDATE tickets SET rank = (SELECT rn FROM (SELECT id, ROW_NUMBER() OVER (ORDER BY rank ASC, id ASC) AS rn FROM tickets) r WHERE r.id = tickets.id)",
        )
        .execute(&mut *tx)
        .await?;
        order = sqlx::query_as("SELECT id, rank FROM tickets ORDER BY rank ASC, id ASC").fetch_all(&mut *tx).await?;
        (lo, hi) = gap(&order);
    }
    let rank = match (lo, hi) {
        (Some(lo), Some(hi)) => (lo + hi) / 2.0,
        (None, Some(hi)) => hi - 1.0,
        (Some(lo), None) => lo + 1.0,
        (None, None) => unreachable!(),
    };
    let row: Row = sqlx::query_as(&format!("UPDATE tickets SET rank = ?2, updated_at = {NOW} WHERE id = ?1 RETURNING {COLUMNS}"))
        .bind(id)
        .bind(rank)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(row.into()))
}

#[derive(sqlx::FromRow)]
struct WorkerRow {
    id: String,
    worker_type: String,
    status: String,
    created_at: String,
    last_heartbeat: Option<String>,
    ticket: Option<i64>,
}

impl From<WorkerRow> for Worker {
    fn from(r: WorkerRow) -> Worker {
        Worker { id: r.id, worker_type: r.worker_type, status: r.status, created_at: r.created_at, last_heartbeat: r.last_heartbeat, ticket: r.ticket }
    }
}

/// Worker columns for the API: everything but the token, plus the ticket it holds.
const WORKER_COLUMNS: &str = "id, worker_type, status, created_at, last_heartbeat, \
    (SELECT t.id FROM tickets t WHERE t.assignee = workers.id ORDER BY t.rank ASC, t.id ASC LIMIT 1) AS ticket";

/// Creates a worker record with a fresh id and token. Also used by the scheduler.
pub async fn new_worker(db: impl SqliteExecutor<'_>, config: &Config, worker_type: &str) -> Result<NewWorker, ApiError> {
    if !config.worker_types.contains_key(worker_type) {
        return Err(ApiError::BadRequest("unknown worker type"));
    }
    let (id, worker_type, token): (String, String, String) = sqlx::query_as(&format!(
        "INSERT INTO workers (id, worker_type, token, status, created_at) \
         VALUES ('w-' || lower(hex(randomblob(4))), ?1, lower(hex(randomblob(24))), 'starting', {NOW}) \
         RETURNING id, worker_type, token"
    ))
    .bind(worker_type)
    .fetch_one(db)
    .await?;
    Ok(NewWorker { id, worker_type, token })
}

async fn create_worker(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(req): Json<CreateWorker>,
) -> Result<(StatusCode, Json<NewWorker>), ApiError> {
    if !matches!(caller, Caller::Human) {
        return Err(ApiError::Forbidden);
    }
    Ok((StatusCode::CREATED, Json(new_worker(&state.pool, &state.config, &req.worker_type).await?)))
}

async fn list_workers(State(state): State<AppState>) -> Result<Json<Vec<Worker>>, ApiError> {
    let rows: Vec<WorkerRow> =
        sqlx::query_as(&format!("SELECT {WORKER_COLUMNS} FROM workers ORDER BY created_at ASC, id ASC")).fetch_all(&state.pool).await?;
    Ok(Json(rows.into_iter().map(Worker::from).collect()))
}

async fn set_status(db: impl SqliteExecutor<'_>, id: &str, status: &str) -> Result<Worker, ApiError> {
    let row: Option<WorkerRow> =
        sqlx::query_as(&format!("UPDATE workers SET status = ?2, last_heartbeat = {NOW} WHERE id = ?1 RETURNING {WORKER_COLUMNS}"))
            .bind(id)
            .bind(status)
            .fetch_optional(db)
            .await?;
    row.map(Worker::from).ok_or(ApiError::NotFound)
}

async fn register(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<String>) -> Result<Json<Worker>, ApiError> {
    caller.require_worker(&id)?;
    Ok(Json(set_status(&state.pool, &id, "idle").await?))
}

async fn heartbeat(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<String>) -> Result<Json<Worker>, ApiError> {
    caller.require_worker(&id)?;
    let row: Option<WorkerRow> = sqlx::query_as(&format!("UPDATE workers SET last_heartbeat = {NOW} WHERE id = ?1 RETURNING {WORKER_COLUMNS}"))
        .bind(&id)
        .fetch_optional(&state.pool)
        .await?;
    row.map(|r| Json(r.into())).ok_or(ApiError::NotFound)
}

/// Hands the worker the lowest-ranked unassigned ticket in a state its type has a prompt for. 204 when there is none.
async fn poll(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<String>) -> Result<Response, ApiError> {
    caller.require_worker(&id)?;
    let mut conn = state.pool.acquire().await?;
    // IMMEDIATE serializes polls so two workers never pick the same ticket.
    let mut tx = conn.begin_with("BEGIN IMMEDIATE").await?;
    let (worker_type,): (String,) =
        sqlx::query_as("SELECT worker_type FROM workers WHERE id = ?1").bind(&id).fetch_optional(&mut *tx).await?.ok_or(ApiError::NotFound)?;
    let prompts = state.config.worker_types.get(&worker_type).map(|w| &w.prompts).ok_or(ApiError::BadRequest("unknown worker type"))?;
    let states: Vec<String> = prompts.keys().map(|s| serde_json::to_string(s).unwrap()).collect();
    let row: Option<Row> = sqlx::query_as(&format!(
        "UPDATE tickets SET assignee = ?1, updated_at = {NOW} WHERE id = \
         (SELECT id FROM tickets WHERE assignee IS NULL AND state IN (SELECT value FROM json_each(?2)) ORDER BY rank ASC, id ASC LIMIT 1) \
         RETURNING {COLUMNS}"
    ))
    .bind(&id)
    .bind(format!("[{}]", states.join(",")))
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        set_status(&mut *tx, &id, "idle").await?;
        tx.commit().await?;
        return Ok(StatusCode::NO_CONTENT.into_response());
    };
    set_status(&mut *tx, &id, "busy").await?;
    tx.commit().await?;
    let ticket: Ticket = row.into();
    let prompt = render(&prompts[&ticket.state], &ticket);
    Ok(Json(PollResponse { ticket, prompt, repos: state.config.project.repos.clone() }).into_response())
}

fn render(template: &str, t: &Ticket) -> String {
    template
        .replace("{{ticket.id}}", &t.id.to_string())
        .replace("{{ticket.title}}", &t.title)
        .replace("{{ticket.description}}", &t.description)
        .replace("{{ticket.state}}", &t.state)
}

#[derive(sqlx::FromRow)]
struct UsageRow {
    id: i64,
    ticket_id: i64,
    worker_id: String,
    worker_type: String,
    tokens_in: i64,
    tokens_out: i64,
    cost: f64,
    created_at: String,
}

impl From<UsageRow> for Usage {
    fn from(r: UsageRow) -> Usage {
        Usage {
            id: r.id,
            ticket_id: r.ticket_id,
            worker_id: r.worker_id,
            worker_type: r.worker_type,
            tokens_in: r.tokens_in,
            tokens_out: r.tokens_out,
            cost: r.cost,
            created_at: r.created_at,
        }
    }
}

const USAGE_COLUMNS: &str = "id, ticket_id, worker_id, worker_type, tokens_in, tokens_out, cost, created_at";

/// Spec 8: a worker reports what an agent run on a ticket cost. Attributed to the worker and its type at report time.
async fn report_usage(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    Json(req): Json<ReportUsage>,
) -> Result<(StatusCode, Json<Usage>), ApiError> {
    caller.require_worker(&id)?;
    ticket_exists(&state, req.ticket_id).await?;
    let row: Option<UsageRow> = sqlx::query_as(&format!(
        "INSERT INTO usage (ticket_id, worker_id, worker_type, tokens_in, tokens_out, cost, created_at) \
         SELECT ?1, id, worker_type, ?2, ?3, ?4, {NOW} FROM workers WHERE id = ?5 RETURNING {USAGE_COLUMNS}"
    ))
    .bind(req.ticket_id)
    .bind(req.tokens_in)
    .bind(req.tokens_out)
    .bind(req.cost)
    .bind(&id)
    .fetch_optional(&state.pool)
    .await?;
    row.map(|r| (StatusCode::CREATED, Json(r.into()))).ok_or(ApiError::NotFound)
}

#[derive(sqlx::FromRow)]
struct TotalsRow {
    tokens_in: i64,
    tokens_out: i64,
    cost: f64,
    tickets_completed: i64,
    tickets_failed: i64,
}

impl From<TotalsRow> for Totals {
    fn from(r: TotalsRow) -> Totals {
        Totals { tokens_in: r.tokens_in, tokens_out: r.tokens_out, cost: r.cost, tickets_completed: r.tickets_completed, tickets_failed: r.tickets_failed }
    }
}

#[derive(sqlx::FromRow)]
struct KeyedRow<K> {
    key: K,
    #[sqlx(flatten)]
    totals: TotalsRow,
}

/// Sums usage grouped by `key`, counting the distinct done/failed tickets that have usage under that key.
async fn breakdown<K, T>(db: impl SqliteExecutor<'_>, key: &str, into: fn(K) -> T) -> Result<Vec<Breakdown<T>>, ApiError>
where
    K: Send + Unpin + for<'r> sqlx::Decode<'r, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>,
{
    let rows: Vec<KeyedRow<K>> = sqlx::query_as(&format!(
        "SELECT u.{key} AS key, SUM(u.tokens_in) AS tokens_in, SUM(u.tokens_out) AS tokens_out, SUM(u.cost) AS cost, \
         COUNT(DISTINCT CASE WHEN t.state = 'done' THEN t.id END) AS tickets_completed, \
         COUNT(DISTINCT CASE WHEN t.state = 'failed' THEN t.id END) AS tickets_failed \
         FROM usage u JOIN tickets t ON t.id = u.ticket_id GROUP BY u.{key} ORDER BY u.{key}"
    ))
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(|r| Breakdown { key: into(r.key), totals: r.totals.into() }).collect())
}

async fn metrics(State(state): State<AppState>) -> Result<Json<Metrics>, ApiError> {
    let totals: TotalsRow = sqlx::query_as(
        "SELECT COALESCE(SUM(tokens_in), 0) AS tokens_in, COALESCE(SUM(tokens_out), 0) AS tokens_out, COALESCE(SUM(cost), 0.0) AS cost, \
         (SELECT COUNT(*) FROM tickets WHERE state = 'done') AS tickets_completed, \
         (SELECT COUNT(*) FROM tickets WHERE state = 'failed') AS tickets_failed FROM usage",
    )
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(Metrics {
        totals: totals.into(),
        per_ticket: breakdown(&state.pool, "ticket_id", |ticket_id| TicketKey { ticket_id }).await?,
        per_worker: breakdown(&state.pool, "worker_id", |worker_id| WorkerKey { worker_id }).await?,
        per_worker_type: breakdown(&state.pool, "worker_type", |worker_type| WorkerTypeKey { worker_type }).await?,
    }))
}

pub enum ApiError {
    Unauthorized,
    Forbidden,
    NotFound,
    BadRequest(&'static str),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        ApiError::Db(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".to_string()),
            ApiError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m.to_string()),
            ApiError::Db(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}
