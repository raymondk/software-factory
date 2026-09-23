use std::time::Duration;

use sqlx::{Connection, SqlitePool};
use tracing::{error, info, warn};

/// Marks every worker silent for longer than `timeout` dead (a `starting` worker counts from its creation), frees
/// the tickets they hold, keeping their state, and ends their open runs, commenting on each such ticket that its
/// worker disappeared, with a link to the run's log (mirroring the worker's own timeout path, spec 6.1). Returns the
/// ids of the workers reaped.
pub async fn reap(pool: &SqlitePool, timeout: Duration) -> sqlx::Result<Vec<String>> {
    let mut conn = pool.acquire().await?;
    let mut tx = conn.begin_with("BEGIN IMMEDIATE").await?;
    let ids: Vec<(String,)> = sqlx::query_as(
        "UPDATE workers SET status = 'dead' WHERE status != 'dead' \
         AND COALESCE(last_heartbeat, created_at) < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?1) RETURNING id",
    )
    .bind(format!("-{} seconds", timeout.as_secs_f64()))
    .fetch_all(&mut *tx)
    .await?;
    let ids: Vec<String> = ids.into_iter().map(|(id,)| id).collect();
    if !ids.is_empty() {
        let open: Vec<(i64, i64, String, String)> = sqlx::query_as(
            "SELECT r.id, r.ticket_id, r.worker_id, t.state FROM runs r JOIN tickets t ON t.id = r.ticket_id \
             WHERE r.ended_at IS NULL AND r.worker_id IN (SELECT value FROM json_each(?1))",
        )
        .bind(serde_json::to_string(&ids).unwrap())
        .fetch_all(&mut *tx)
        .await?;
        for (run, ticket, worker, state) in open {
            warn!(worker = %worker, ticket, run, state = %state, "worker disappeared mid-run; ticket released");
            let body = format!(
                "Worker {worker} disappeared (no heartbeat for {}); leaving {state} for another worker. Log: [run {run}](#/tickets/{ticket}/runs/{run})",
                humantime_serde::re::humantime::format_duration(timeout)
            );
            sqlx::query("INSERT INTO comments (ticket_id, author, body, created_at) VALUES (?1, 'orchestrator', ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))")
                .bind(ticket)
                .bind(body)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query(
            "UPDATE tickets SET assignee = NULL, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
             WHERE assignee IN (SELECT value FROM json_each(?1))",
        )
        .bind(serde_json::to_string(&ids).unwrap())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE runs SET ended_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
             WHERE ended_at IS NULL AND worker_id IN (SELECT value FROM json_each(?1))",
        )
        .bind(serde_json::to_string(&ids).unwrap())
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(ids)
}

/// Spec 4.7: drops log lines past `retention`: a run's lines once the run ended that long ago, and run-less lines
/// (startup, polling) by their own age. Run rows stay. Returns the number of lines deleted.
pub async fn purge_logs(pool: &SqlitePool, retention: Duration) -> sqlx::Result<u64> {
    let cutoff = format!("-{} seconds", retention.as_secs_f64());
    let deleted = sqlx::query(
        "DELETE FROM log_lines WHERE CASE WHEN run_id IS NULL THEN created_at \
         ELSE (SELECT ended_at FROM runs WHERE id = log_lines.run_id) END < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?1)",
    )
    .bind(cutoff)
    .execute(pool)
    .await?;
    Ok(deleted.rows_affected())
}

/// Runs `purge_logs` at startup and then hourly.
pub async fn run_purge(pool: SqlitePool, retention: Duration) {
    let mut interval = tokio::time::interval(Duration::from_secs(3600));
    loop {
        interval.tick().await;
        match purge_logs(&pool, retention).await {
            Ok(0) => {}
            Ok(n) => info!(lines = n, retention = %humantime_serde::re::humantime::format_duration(retention), "purged old log lines"),
            Err(e) => error!("log purge failed: {e}"),
        }
    }
}

/// Runs `reap` forever, every quarter of `timeout` (at least once a second).
pub async fn run(pool: SqlitePool, timeout: Duration) {
    let mut interval = tokio::time::interval((timeout / 4).max(Duration::from_secs(1)));
    loop {
        interval.tick().await;
        match reap(&pool, timeout).await {
            Ok(ids) => ids.iter().for_each(|id| warn!(worker = %id, timeout = %humantime_serde::re::humantime::format_duration(timeout), "no heartbeat; worker marked dead")),
            Err(e) => error!("reaper failed: {e}"),
        }
    }
}
