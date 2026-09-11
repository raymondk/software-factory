//! Spec 4.3: starts a worker per available (unassigned, unblocked) ticket and stops idle ones when their type's queue is empty.

use std::collections::BTreeMap;

use sqlx::SqlitePool;

use crate::api;
use crate::config::Config;
use crate::provider::{Provider, StartWorker};

/// One scheduling pass. Fails only when the provider or the database is unreachable; the next pass retries.
pub async fn tick(pool: &SqlitePool, config: &Config, provider: &Provider) -> anyhow::Result<()> {
    let status = provider.status().await?;
    let workers: Vec<(String, String, String)> = sqlx::query_as("SELECT id, worker_type, status FROM workers").fetch_all(pool).await?;
    let alive = workers.iter().filter(|(_, _, s)| s != "dead").count() as u32;
    let mut slots = config.scheduler.max_workers.saturating_sub(alive).min(status.capacity.saturating_sub(status.in_use));
    let mut available = BTreeMap::new();
    for (name, wt) in &config.worker_types {
        let states = serde_json::to_string(&wt.prompts.keys().collect::<Vec<_>>()).unwrap();
        let (n,): (i64,) = sqlx::query_as(&format!(
            "SELECT COUNT(*) FROM tickets WHERE assignee IS NULL AND state IN (SELECT value FROM json_each(?1)) AND NOT {}",
            api::BLOCKED
        ))
        .bind(states)
            .fetch_one(pool)
            .await?;
        available.insert(name.as_str(), n as u32);
    }

    // Stop idle workers of types with nothing to do. Marking dead first invalidates the token even if the stop fails;
    // the dead-worker sweep below then retries the stop next tick.
    for (id, wt, s) in &workers {
        if s == "idle" && available.get(wt.as_str()).copied().unwrap_or(0) == 0 {
            let marked = sqlx::query("UPDATE workers SET status = 'dead' WHERE id = ?1 AND status = 'idle'").bind(id).execute(pool).await?;
            if marked.rows_affected() == 0 {
                continue;
            }
            eprintln!("scheduler: stopping idle {wt} worker {id}");
            if let Err(e) = provider.stop(id).await {
                eprintln!("scheduler: stop {id}: {e:#}");
            }
        }
    }

    // Dead workers (reaped, or whose stop failed) the provider still runs.
    for (id, _, _) in workers.iter().filter(|(_, _, s)| s == "dead") {
        if status.workers.iter().any(|w| &w.worker_id == id) {
            eprintln!("scheduler: stopping dead worker {id}");
            if let Err(e) = provider.stop(id).await {
                eprintln!("scheduler: stop {id}: {e:#}");
            }
        }
    }

    // Provider workers the orchestrator has no record of (restart with a fresh database): they can never register.
    for w in status.workers.iter().filter(|w| !workers.iter().any(|(id, _, _)| id == &w.worker_id)) {
        eprintln!("scheduler: stopping unknown worker {}", w.worker_id);
        if let Err(e) = provider.stop(&w.worker_id).await {
            eprintln!("scheduler: stop {}: {e:#}", w.worker_id);
        }
    }

    for (name, n) in available {
        let pending = workers.iter().filter(|(_, wt, s)| wt == name && (s == "starting" || s == "idle")).count() as u32;
        for _ in 0..n.saturating_sub(pending).min(slots) {
            let w = api::new_worker(pool, config, name).await.map_err(|e| anyhow::anyhow!("creating worker: {e:?}"))?;
            let req = StartWorker {
                worker_id: w.id.clone(),
                worker_type: w.worker_type,
                orchestrator_url: config.orchestrator.public_url.clone().unwrap(),
                worker_token: w.token,
            };
            eprintln!("scheduler: starting {name} worker {}", w.id);
            if let Err(e) = provider.start(&req).await {
                sqlx::query("UPDATE workers SET status = 'dead' WHERE id = ?1").bind(&w.id).execute(pool).await?;
                anyhow::bail!("start {}: {e:#}", w.id);
            }
            slots -= 1;
        }
    }
    Ok(())
}

pub async fn run(pool: SqlitePool, config: std::sync::Arc<Config>, provider: Provider) {
    let mut interval = tokio::time::interval(config.scheduler.interval);
    loop {
        interval.tick().await;
        if let Err(e) = tick(&pool, &config, &provider).await {
            eprintln!("scheduler: {e:#}");
        }
    }
}
