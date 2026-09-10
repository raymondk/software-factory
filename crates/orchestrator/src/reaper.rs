use std::time::Duration;

use sqlx::{Connection, SqlitePool};

/// Marks every worker silent for longer than `timeout` dead (a `starting` worker counts from its creation) and
/// frees the tickets they hold, keeping their state. Returns the ids of the workers reaped.
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
        sqlx::query(
            "UPDATE tickets SET assignee = NULL, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
             WHERE assignee IN (SELECT value FROM json_each(?1))",
        )
        .bind(serde_json::to_string(&ids).unwrap())
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(ids)
}

/// Runs `reap` forever, every quarter of `timeout` (at least once a second).
pub async fn run(pool: SqlitePool, timeout: Duration) {
    let mut interval = tokio::time::interval((timeout / 4).max(Duration::from_secs(1)));
    loop {
        interval.tick().await;
        match reap(&pool, timeout).await {
            Ok(ids) => ids.iter().for_each(|id| eprintln!("reaped worker {id}")),
            Err(e) => eprintln!("reaper: {e}"),
        }
    }
}
