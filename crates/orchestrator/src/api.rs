use api_client::{
    Breakdown, Comment, CreateComment, CreateRelation, CreateTicket, CreateWorker, ListTickets, LogLine, LogsAfter, Metrics, MoveTicket, NewWorker, PollRequest,
    ModelKey, PollResponse, Relation, ReportUsage, Run, ShipLogs, Ticket, TicketKey, Totals, UpdateTicket, Usage, Worker, WorkerKey, AgentKey,
};
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use sqlx::{Connection, SqliteExecutor};

use crate::config::Config;
use crate::{AppState, STATES};

/// Built by build.rs.
const UI: &str = include_str!("../ui/dist/index.html");
/// Neighbouring ranks closer than this trigger renormalization.
const MIN_GAP: f64 = 1e-6;

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/tickets", get(list_tickets).post(create_ticket))
        .route("/tickets/{id}", get(get_ticket).patch(update_ticket))
        .route("/tickets/{id}/move", post(move_ticket))
        .route("/tickets/{id}/comments", get(list_comments).post(add_comment))
        .route("/tickets/{id}/comments/{cid}/resolve", post(resolve_comment))
        .route("/tickets/{id}/comments/{cid}/unresolve", post(unresolve_comment))
        .route("/tickets/{id}/relations", post(add_relation))
        .route("/tickets/{id}/relations/{kind}/{other}", delete(remove_relation))
        .route("/workers", get(list_workers).post(create_worker))
        .route("/workers/{id}/register", post(register))
        .route("/workers/{id}/heartbeat", post(heartbeat))
        .route("/workers/{id}/poll", post(poll))
        .route("/workers/{id}/usage", post(report_usage))
        .route("/workers/{id}/logs", get(worker_logs).post(ship_logs))
        .route("/runs/{id}/logs", get(run_logs))
        .route("/metrics", get(metrics))
        .route("/agents", get(agents))
        .route("/config", get(config))
        .layer(middleware::from_fn_with_state(state.clone(), auth));
    Router::new().route("/", get(ui)).route("/favicon.ico", get(|| async { StatusCode::NO_CONTENT })).merge(api).with_state(state)
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
    let token = state.config.orchestrator.token.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;");
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
    agent: Option<String>,
    model: Option<String>,
    #[sqlx(default)]
    unresolved_comments: i64,
    #[sqlx(default)]
    blocked: bool,
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
            unresolved_comments: r.unresolved_comments,
            relations: vec![],
            blocked: r.blocked,
            agent: r.agent,
            model: r.model,
            runs: vec![],
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

const COLUMNS: &str = "id, title, description, state, rank, assignee, created_at, updated_at, links, agent, model";
const COLUMNS_WITH_COMMENTS: &str = "id, title, description, state, rank, assignee, created_at, updated_at, links, agent, model, \
    (SELECT COUNT(*) FROM comments c WHERE c.ticket_id = tickets.id AND NOT c.resolved) AS unresolved_comments";
const COMMENT_COLUMNS: &str = "id, ticket_id, author, body, created_at, resolved";
/// Spec 3.7: a `ready` ticket with a dependency that is not yet `in_review` or `done`. Evaluated against `tickets`.
pub const BLOCKED: &str = "(tickets.state = 'ready' AND EXISTS (SELECT 1 FROM ticket_relations r JOIN tickets d ON d.id = r.to_id \
    WHERE r.from_id = tickets.id AND r.type = 'depends_on' AND d.state NOT IN ('in_review', 'done')))";
const RELATION_TYPES: [&str; 2] = ["depends_on", "related_to"];
const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%fZ', 'now')";

async fn create_ticket(State(state): State<AppState>, Json(req): Json<CreateTicket>) -> Result<(StatusCode, Json<Ticket>), ApiError> {
    if req.title.trim().is_empty() {
        return Err(ApiError::BadRequest("title must not be empty"));
    }
    let state_name = req.state.as_deref().unwrap_or("todo");
    if !STATES.contains(&state_name) {
        return Err(ApiError::BadRequest("invalid state"));
    }
    check_advertised(&state, req.agent.as_deref(), req.model.as_deref())?;
    let row: Row = sqlx::query_as(&format!(
        "INSERT INTO tickets (title, description, state, rank, created_at, updated_at, agent, model) \
         VALUES (?1, ?2, ?3, (SELECT COALESCE(MAX(rank), 0) + 1 FROM tickets), {NOW}, {NOW}, ?4, ?5) \
         RETURNING {COLUMNS}"
    ))
    .bind(&req.title)
    .bind(&req.description)
    .bind(state_name)
    .bind(&req.agent)
    .bind(&req.model)
    .fetch_one(&state.pool)
    .await?;
    Ok((StatusCode::CREATED, Json(row.into())))
}

/// Spec 3.6: an agent or model on a ticket must be advertised together by some provider's last status.
fn check_advertised(state: &AppState, agent: Option<&str>, model: Option<&str>) -> Result<(), ApiError> {
    if (agent.is_some() || model.is_some()) && !state.providers.advertised(agent, model) {
        return Err(ApiError::BadRequest(match (agent, model) {
            (Some(_), None) => "agent not advertised by any provider",
            (None, Some(_)) => "model not advertised by any provider",
            _ => "agent and model not advertised together by any provider",
        }));
    }
    Ok(())
}

// The list leaves `comments` empty; only GET /tickets/{id} embeds the thread, to avoid a query per ticket.
async fn list_tickets(State(state): State<AppState>, Query(filter): Query<ListTickets>) -> Result<Json<Vec<Ticket>>, ApiError> {
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT {COLUMNS_WITH_COMMENTS}, {BLOCKED} AS blocked FROM tickets WHERE (?1 IS NULL OR state = ?1) AND (?2 IS NULL OR assignee = ?2) \
         ORDER BY rank ASC, id ASC"
    ))
    .bind(&filter.state)
    .bind(&filter.assignee)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows.into_iter().map(Ticket::from).collect()))
}

async fn get_ticket(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<Ticket>, ApiError> {
    Ok(Json(full_ticket(&state, id).await?))
}

/// The ticket with its comments and relations.
async fn full_ticket(state: &AppState, id: i64) -> Result<Ticket, ApiError> {
    let row: Option<Row> = sqlx::query_as(&format!("SELECT {COLUMNS_WITH_COMMENTS}, {BLOCKED} AS blocked FROM tickets WHERE id = ?1"))
        .bind(id)
        .fetch_optional(&state.pool)
        .await?;
    let mut ticket: Ticket = row.ok_or(ApiError::NotFound)?.into();
    ticket.comments = comments_of(state, id).await?;
    ticket.relations = relations_of(state, id).await?;
    let runs: Vec<RunRow> =
        sqlx::query_as(&format!("SELECT {RUN_COLUMNS} FROM runs r JOIN workers w ON w.id = r.worker_id WHERE r.ticket_id = ?1 ORDER BY r.id DESC"))
            .bind(id)
            .fetch_all(&state.pool)
            .await?;
    ticket.runs = runs
        .into_iter()
        .map(|r| Run {
            agent: r.agent,
            model: r.model,
            id: r.id,
            ticket_id: r.ticket_id,
            worker_id: r.worker_id,
            started_at: r.started_at,
            ended_at: r.ended_at,
        })
        .collect();
    Ok(ticket)
}

#[derive(sqlx::FromRow)]
struct RunRow {
    id: i64,
    ticket_id: i64,
    worker_id: String,
    agent: String,
    model: Option<String>,
    started_at: String,
    ended_at: Option<String>,
}

const RUN_COLUMNS: &str = "r.id, r.ticket_id, r.worker_id, w.agent, r.model, r.started_at, r.ended_at";

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
    set_resolved(&state, id, cid, true).await
}

async fn unresolve_comment(State(state): State<AppState>, Path((id, cid)): Path<(i64, i64)>) -> Result<Json<Comment>, ApiError> {
    set_resolved(&state, id, cid, false).await
}

async fn set_resolved(state: &AppState, id: i64, cid: i64, resolved: bool) -> Result<Json<Comment>, ApiError> {
    let row: Option<CommentRow> =
        sqlx::query_as(&format!("UPDATE comments SET resolved = ?3 WHERE id = ?1 AND ticket_id = ?2 RETURNING {COMMENT_COLUMNS}"))
            .bind(cid)
            .bind(id)
            .bind(resolved)
            .fetch_optional(&state.pool)
            .await?;
    row.map(|r| Json(r.into())).ok_or(ApiError::NotFound)
}

#[derive(sqlx::FromRow)]
struct RelationRow {
    r#type: String,
    ticket: i64,
    title: String,
    state: String,
    satisfied: Option<bool>,
}

/// Dependencies, dependents (as `blocks`) and related tickets, each with the other ticket's title and state.
async fn relations_of(state: &AppState, id: i64) -> Result<Vec<Relation>, ApiError> {
    let rows: Vec<RelationRow> = sqlx::query_as(
        "SELECT 'depends_on' AS type, t.id AS ticket, t.title, t.state, t.state IN ('in_review', 'done') AS satisfied \
         FROM ticket_relations r JOIN tickets t ON t.id = r.to_id WHERE r.from_id = ?1 AND r.type = 'depends_on' \
         UNION ALL SELECT 'blocks', t.id, t.title, t.state, NULL \
         FROM ticket_relations r JOIN tickets t ON t.id = r.from_id WHERE r.to_id = ?1 AND r.type = 'depends_on' \
         UNION ALL SELECT 'related_to', t.id, t.title, t.state, NULL \
         FROM ticket_relations r JOIN tickets t ON t.id = CASE WHEN r.from_id = ?1 THEN r.to_id ELSE r.from_id END \
         WHERE r.type = 'related_to' AND ?1 IN (r.from_id, r.to_id) ORDER BY 1, 2",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    Ok(rows.into_iter().map(|r| Relation { r#type: r.r#type, ticket: r.ticket, title: r.title, state: r.state, satisfied: r.satisfied }).collect())
}

/// Spec 3.7: refused when duplicate (either direction for `related_to`), self-referencing, or a `depends_on` cycle.
async fn add_relation(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<i64>,
    Json(req): Json<CreateRelation>,
) -> Result<(StatusCode, Json<Ticket>), ApiError> {
    if !RELATION_TYPES.contains(&req.r#type.as_str()) {
        return Err(ApiError::BadRequest("invalid relation type"));
    }
    if req.ticket == id {
        return Err(ApiError::BadRequest("a ticket cannot relate to itself"));
    }
    let mut conn = state.pool.acquire().await?;
    // IMMEDIATE so two concurrent inserts cannot each pass the cycle check.
    let mut tx = conn.begin_with("BEGIN IMMEDIATE").await?;
    authorize(&caller, &mut *tx, id).await?;
    let other: Option<(i64,)> = sqlx::query_as("SELECT id FROM tickets WHERE id = ?1").bind(req.ticket).fetch_optional(&mut *tx).await?;
    other.ok_or(ApiError::BadRequest("related ticket not found"))?;
    let dup: Option<(i64,)> = sqlx::query_as(
        "SELECT 1 FROM ticket_relations WHERE type = ?3 AND (from_id = ?1 AND to_id = ?2 OR type = 'related_to' AND from_id = ?2 AND to_id = ?1)",
    )
    .bind(id)
    .bind(req.ticket)
    .bind(&req.r#type)
    .fetch_optional(&mut *tx)
    .await?;
    if dup.is_some() {
        return Err(ApiError::BadRequest("relation already exists"));
    }
    if req.r#type == "depends_on" {
        let cycle: Option<(i64,)> = sqlx::query_as(
            "WITH RECURSIVE reach(id) AS (SELECT to_id FROM ticket_relations WHERE from_id = ?2 AND type = 'depends_on' \
             UNION SELECT r.to_id FROM ticket_relations r JOIN reach ON r.from_id = reach.id WHERE r.type = 'depends_on') \
             SELECT 1 FROM reach WHERE id = ?1",
        )
        .bind(id)
        .bind(req.ticket)
        .fetch_optional(&mut *tx)
        .await?;
        if cycle.is_some() {
            return Err(ApiError::BadRequest("dependency would form a cycle"));
        }
    }
    sqlx::query("INSERT INTO ticket_relations (from_id, type, to_id) VALUES (?1, ?2, ?3)").bind(id).bind(&req.r#type).bind(req.ticket).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(full_ticket(&state, id).await?)))
}

async fn remove_relation(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path((id, kind, other)): Path<(i64, String, i64)>,
) -> Result<Json<Ticket>, ApiError> {
    authorize(&caller, &state.pool, id).await?;
    let deleted = sqlx::query(
        "DELETE FROM ticket_relations WHERE type = ?3 AND (from_id = ?1 AND to_id = ?2 OR type = 'related_to' AND from_id = ?2 AND to_id = ?1)",
    )
    .bind(id)
    .bind(other)
    .bind(&kind)
    .execute(&state.pool)
    .await?;
    if deleted.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(Json(full_ticket(&state, id).await?))
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
    if req.agent.is_some() || req.model.is_some() {
        let (agent, model): (Option<String>, Option<String>) =
            sqlx::query_as("SELECT agent, model FROM tickets WHERE id = ?1").bind(id).fetch_optional(&state.pool).await?.ok_or(ApiError::NotFound)?;
        let agent = req.agent.clone().unwrap_or(agent);
        let model = req.model.clone().unwrap_or(model);
        check_advertised(&state, agent.as_deref(), model.as_deref())?;
    }
    // Spec 3.2: a state change clears the assignee unless the same patch sets it, or the holding worker moves it to
    // in_progress (authorize guarantees a worker caller is the assignee).
    let keep = matches!(caller, Caller::Worker(_)) && req.state.as_deref() == Some("in_progress");
    let row: Option<Row> = sqlx::query_as(&format!(
        "UPDATE tickets SET title = COALESCE(?2, title), description = COALESCE(?3, description), \
         state = COALESCE(?4, state), \
         assignee = CASE WHEN ?5 THEN ?6 WHEN ?4 IS NOT NULL AND ?4 != state AND NOT ?8 THEN NULL ELSE assignee END, \
         links = COALESCE(?7, links), agent = CASE WHEN ?9 THEN ?10 ELSE agent END, model = CASE WHEN ?11 THEN ?12 ELSE model END, \
         updated_at = {NOW} WHERE id = ?1 RETURNING {COLUMNS}"
    ))
    .bind(id)
    .bind(&req.title)
    .bind(&req.description)
    .bind(&req.state)
    .bind(req.assignee.is_some())
    .bind(req.assignee.clone().flatten())
    .bind(req.links.as_ref().map(|l| serde_json::to_string(l).unwrap()))
    .bind(keep)
    .bind(req.agent.is_some())
    .bind(req.agent.clone().flatten())
    .bind(req.model.is_some())
    .bind(req.model.clone().flatten())
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
    agent: String,
    provider: String,
    status: String,
    created_at: String,
    last_heartbeat: Option<String>,
    ticket: Option<i64>,
}

impl From<WorkerRow> for Worker {
    fn from(r: WorkerRow) -> Worker {
        Worker { id: r.id, agent: r.agent, provider: r.provider, status: r.status, created_at: r.created_at, last_heartbeat: r.last_heartbeat, ticket: r.ticket }
    }
}

/// Worker columns for the API: everything but the token, plus the ticket it holds.
const WORKER_COLUMNS: &str = "id, agent, provider, status, created_at, last_heartbeat, \
    (SELECT t.id FROM tickets t WHERE t.assignee = workers.id ORDER BY t.rank ASC, t.id ASC LIMIT 1) AS ticket";

/// Creates a worker record with a fresh id and token, to run `agent` on `provider`. Also used by the scheduler.
pub async fn new_worker(db: impl SqliteExecutor<'_>, config: &Config, agent: &str, provider: &str) -> Result<NewWorker, ApiError> {
    if !config.agents.contains_key(agent) {
        return Err(ApiError::BadRequest("unknown agent"));
    }
    if !config.providers.contains_key(provider) {
        return Err(ApiError::BadRequest("unknown provider"));
    }
    let (id, agent, provider, token): (String, String, String, String) = sqlx::query_as(&format!(
        "INSERT INTO workers (id, agent, provider, token, status, created_at) \
         VALUES ('w-' || lower(hex(randomblob(4))), ?1, ?2, lower(hex(randomblob(24))), 'starting', {NOW}) \
         RETURNING id, agent, provider, token"
    ))
    .bind(agent)
    .bind(provider)
    .fetch_one(db)
    .await?;
    Ok(NewWorker { id, agent, provider, token })
}

async fn create_worker(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(req): Json<CreateWorker>,
) -> Result<(StatusCode, Json<NewWorker>), ApiError> {
    if !matches!(caller, Caller::Human) {
        return Err(ApiError::Forbidden);
    }
    // The first configured provider unless the request names one.
    let provider = req.provider.as_deref().unwrap_or_else(|| state.config.providers.keys().next().unwrap());
    Ok((StatusCode::CREATED, Json(new_worker(&state.pool, &state.config, &req.agent, provider).await?)))
}

async fn list_workers(State(state): State<AppState>) -> Result<Json<Vec<Worker>>, ApiError> {
    let rows: Vec<WorkerRow> =
        sqlx::query_as(&format!("SELECT {WORKER_COLUMNS} FROM workers ORDER BY created_at ASC, id ASC")).fetch_all(&state.pool).await?;
    Ok(Json(rows.into_iter().map(Worker::from).collect()))
}

/// Only the authenticated worker itself gets here, so a missing row means the reaper marked it dead since: 401, and a
/// dead worker is never resurrected.
async fn set_status(db: impl SqliteExecutor<'_>, id: &str, status: &str) -> Result<Worker, ApiError> {
    let row: Option<WorkerRow> = sqlx::query_as(&format!(
        "UPDATE workers SET status = ?2, last_heartbeat = {NOW} WHERE id = ?1 AND status != 'dead' RETURNING {WORKER_COLUMNS}"
    ))
    .bind(id)
    .bind(status)
    .fetch_optional(db)
    .await?;
    row.map(Worker::from).ok_or(ApiError::Unauthorized)
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

/// Hands the worker the lowest-ranked unassigned, unblocked ticket in a state that has a prompt, whose `agent` is unset
/// or the worker's, and whose `model` is unset or one the worker's provider advertises for that agent. 204 when there is none.
async fn poll(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    body: Option<Json<PollRequest>>,
) -> Result<Response, ApiError> {
    caller.require_worker(&id)?;
    let exclude = body.and_then(|Json(b)| b.exclude);
    let mut conn = state.pool.acquire().await?;
    // IMMEDIATE serializes polls so two workers never pick the same ticket.
    let mut tx = conn.begin_with("BEGIN IMMEDIATE").await?;
    let (agent_name, provider): (String, String) =
        sqlx::query_as("SELECT agent, provider FROM workers WHERE id = ?1").bind(&id).fetch_optional(&mut *tx).await?.ok_or(ApiError::NotFound)?;
    let agent = state.config.agents.get(&agent_name).ok_or(ApiError::BadRequest("unknown agent"))?;
    let models = serde_json::to_string(&state.providers.models(&provider, &agent_name)).unwrap();
    let prompts = &state.config.prompts;
    let states: Vec<String> = prompts.keys().map(|s| serde_json::to_string(s).unwrap()).collect();
    let row: Option<Row> = sqlx::query_as(&format!(
        "UPDATE tickets SET assignee = ?1, updated_at = {NOW} WHERE id = \
         (SELECT id FROM tickets WHERE assignee IS NULL AND state IN (SELECT value FROM json_each(?2)) AND NOT {BLOCKED} \
          AND (agent IS NULL OR agent = ?4) AND (model IS NULL OR model IN (SELECT value FROM json_each(?5))) \
          ORDER BY id IS ?3, rank ASC, id ASC LIMIT 1) \
         RETURNING {COLUMNS}"
    ))
    .bind(&id)
    .bind(format!("[{}]", states.join(",")))
    .bind(exclude)
    .bind(&agent_name)
    .bind(models)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        set_status(&mut *tx, &id, "idle").await?;
        tx.commit().await?;
        return Ok(StatusCode::NO_CONTENT.into_response());
    };
    set_status(&mut *tx, &id, "busy").await?;
    let (run,): (i64,) = sqlx::query_as(&format!("INSERT INTO runs (ticket_id, worker_id, started_at) VALUES (?1, ?2, {NOW}) RETURNING id"))
        .bind(row.id)
        .bind(&id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    let ticket: Ticket = row.into();
    let prompt = render(&prompts[&ticket.state], &ticket);
    let model = ticket.model.clone();
    Ok(Json(PollResponse { ticket, run, prompt, model, repos: state.config.project.repos.clone(), run_timeout: agent.run_timeout }).into_response())
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
    agent: String,
    model: Option<String>,
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
            agent: r.agent,
            model: r.model,
            tokens_in: r.tokens_in,
            tokens_out: r.tokens_out,
            cost: r.cost,
            created_at: r.created_at,
        }
    }
}

const USAGE_COLUMNS: &str = "id, ticket_id, worker_id, agent, model, tokens_in, tokens_out, cost, created_at";

/// Spec 8: a worker reports what an agent run on a ticket cost, and the model it ran with. Attributed to the worker
/// and its agent at report time; the model is also recorded on the run this ends.
async fn report_usage(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    Json(req): Json<ReportUsage>,
) -> Result<(StatusCode, Json<Usage>), ApiError> {
    caller.require_worker(&id)?;
    ticket_exists(&state, req.ticket_id).await?;
    sqlx::query(&format!("UPDATE runs SET ended_at = {NOW}, model = ?2 WHERE worker_id = ?1 AND ended_at IS NULL"))
        .bind(&id)
        .bind(&req.model)
        .execute(&state.pool)
        .await?;
    let row: Option<UsageRow> = sqlx::query_as(&format!(
        "INSERT INTO usage (ticket_id, worker_id, agent, model, tokens_in, tokens_out, cost, created_at) \
         SELECT ?1, id, agent, ?6, ?2, ?3, ?4, {NOW} FROM workers WHERE id = ?5 RETURNING {USAGE_COLUMNS}"
    ))
    .bind(req.ticket_id)
    .bind(req.tokens_in)
    .bind(req.tokens_out)
    .bind(req.cost)
    .bind(&id)
    .bind(&req.model)
    .fetch_optional(&state.pool)
    .await?;
    row.map(|r| (StatusCode::CREATED, Json(r.into()))).ok_or(ApiError::NotFound)
}

#[derive(sqlx::FromRow)]
struct LogRow {
    id: i64,
    run_id: Option<i64>,
    line: String,
    created_at: String,
}

impl From<LogRow> for LogLine {
    fn from(r: LogRow) -> LogLine {
        LogLine { id: r.id, run_id: r.run_id, line: r.line, created_at: r.created_at }
    }
}

const LOG_COLUMNS: &str = "id, run_id, line, created_at";
const LOG_PAGE: i64 = 1000;

/// Spec 4.7: a worker ships a batch of its output. `run` must be one of its own runs.
async fn ship_logs(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    Json(req): Json<ShipLogs>,
) -> Result<StatusCode, ApiError> {
    caller.require_worker(&id)?;
    let mut conn = state.pool.acquire().await?;
    let mut tx = conn.begin().await?;
    if let Some(run) = req.run {
        let owned: Option<(i64,)> = sqlx::query_as("SELECT id FROM runs WHERE id = ?1 AND worker_id = ?2").bind(run).bind(&id).fetch_optional(&mut *tx).await?;
        owned.ok_or(ApiError::BadRequest("run does not belong to this worker"))?;
    }
    for line in &req.lines {
        sqlx::query(&format!("INSERT INTO log_lines (worker_id, run_id, line, created_at) VALUES (?1, ?2, ?3, {NOW})"))
            .bind(&id)
            .bind(req.run)
            .bind(line)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `key` is compared as text; SQLite converts it for the integer `run_id` column.
async fn logs_where(state: &AppState, column: &str, key: &str, after: Option<i64>) -> Result<Json<Vec<LogLine>>, ApiError> {
    let rows: Vec<LogRow> = sqlx::query_as(&format!("SELECT {LOG_COLUMNS} FROM log_lines WHERE {column} = ?1 AND id > ?2 ORDER BY id ASC LIMIT {LOG_PAGE}"))
        .bind(key)
        .bind(after.unwrap_or(0))
        .fetch_all(&state.pool)
        .await?;
    Ok(Json(rows.into_iter().map(LogLine::from).collect()))
}

/// The worker's whole stream, including lines outside runs.
async fn worker_logs(State(state): State<AppState>, Path(id): Path<String>, Query(q): Query<LogsAfter>) -> Result<Json<Vec<LogLine>>, ApiError> {
    let found: Option<(String,)> = sqlx::query_as("SELECT id FROM workers WHERE id = ?1").bind(&id).fetch_optional(&state.pool).await?;
    found.ok_or(ApiError::NotFound)?;
    logs_where(&state, "worker_id", &id, q.after).await
}

async fn run_logs(State(state): State<AppState>, Path(id): Path<i64>, Query(q): Query<LogsAfter>) -> Result<Json<Vec<LogLine>>, ApiError> {
    let found: Option<(i64,)> = sqlx::query_as("SELECT id FROM runs WHERE id = ?1").bind(id).fetch_optional(&state.pool).await?;
    found.ok_or(ApiError::NotFound)?;
    logs_where(&state, "run_id", &id.to_string(), q.after).await
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

/// Spec 4.2: the running configuration, read-only, without the token.
async fn config(State(state): State<AppState>) -> Json<std::sync::Arc<Config>> {
    Json(state.config.clone())
}

/// Spec 4.2: what providers advertise, for the UI to offer when setting agent and model on a ticket.
async fn agents(State(state): State<AppState>) -> Json<std::collections::BTreeMap<String, Vec<String>>> {
    Json(state.providers.agents())
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
        per_agent: breakdown(&state.pool, "agent", |agent| AgentKey { agent }).await?,
        per_model: breakdown(&state.pool, "model", |model| ModelKey { model }).await?,
    }))
}

#[derive(Debug)]
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
