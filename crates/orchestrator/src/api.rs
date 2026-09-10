use api_client::{CreateTicket, ListTickets, MoveTicket, Ticket, UpdateTicket};
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use sqlx::Connection;

use crate::AppState;

const UI: &str = include_str!("../ui/index.html");
/// Neighbouring ranks closer than this trigger renormalization.
const MIN_GAP: f64 = 1e-6;
const STATES: [&str; 6] = ["todo", "ready", "in_progress", "in_review", "failed", "done"];

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/tickets", get(list_tickets).post(create_ticket))
        .route("/tickets/{id}", get(get_ticket).patch(update_ticket))
        .route("/tickets/{id}/move", post(move_ticket))
        .layer(middleware::from_fn_with_state(state.clone(), auth));
    Router::new().route("/", get(ui)).merge(api).with_state(state)
}

async fn auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if token == Some(state.token.as_str()) {
        next.run(req).await
    } else {
        ApiError::Unauthorized.into_response()
    }
}

async fn ui(State(state): State<AppState>) -> Html<String> {
    let token = serde_json::to_string(&state.token).unwrap().replace("</", "<\\/");
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

const COLUMNS: &str = "id, title, description, state, rank, assignee, created_at, updated_at, links";
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
    row.map(|r| Json(r.into())).ok_or(ApiError::NotFound)
}

async fn update_ticket(State(state): State<AppState>, Path(id): Path<i64>, Json(req): Json<UpdateTicket>) -> Result<Json<Ticket>, ApiError> {
    if req.title.as_deref().is_some_and(|t| t.trim().is_empty()) {
        return Err(ApiError::BadRequest("title must not be empty"));
    }
    if req.state.as_deref().is_some_and(|s| !STATES.contains(&s)) {
        return Err(ApiError::BadRequest("invalid state"));
    }
    let row: Option<Row> = sqlx::query_as(&format!(
        "UPDATE tickets SET title = COALESCE(?2, title), description = COALESCE(?3, description), \
         state = COALESCE(?4, state), assignee = CASE WHEN ?5 THEN ?6 ELSE assignee END, \
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

async fn move_ticket(State(state): State<AppState>, Path(id): Path<i64>, Json(req): Json<MoveTicket>) -> Result<Json<Ticket>, ApiError> {
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

pub enum ApiError {
    Unauthorized,
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
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m.to_string()),
            ApiError::Db(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}
