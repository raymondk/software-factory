//! Spec 4.3: starts a worker per available (unassigned, unblocked) ticket and stops idle ones when the queue is empty.

use std::collections::BTreeMap;

use sqlx::SqlitePool;

use crate::api;
use crate::config::Config;
use crate::provider::{Provider, StartWorker};

/// One scheduling pass. Fails only when the provider or the database is unreachable; the next pass retries.
pub async fn tick(pool: &SqlitePool, config: &Config, provider: &Provider) -> anyhow::Result<()> {
    let status = provider.status().await?;
    let workers: Vec<(String, String, String)> = sqlx::query_as("SELECT id, agent, status FROM workers").fetch_all(pool).await?;
    let alive = workers.iter().filter(|(_, _, s)| s != "dead").count() as u32;
    let slots = config.scheduler.max_workers.saturating_sub(alive).min(status.capacity.saturating_sub(status.in_use));
    let states = serde_json::to_string(&config.prompts.keys().collect::<Vec<_>>()).unwrap();
    // Available tickets per agent; those with none form a pool any worker drains.
    let counts: Vec<(Option<String>, i64)> = sqlx::query_as(&format!(
        "SELECT agent, COUNT(*) FROM tickets WHERE assignee IS NULL AND state IN (SELECT value FROM json_each(?1)) AND NOT {} GROUP BY agent",
        api::BLOCKED
    ))
    .bind(states)
    .fetch_all(pool)
    .await?;
    let mut pool_size = 0u32;
    let mut pinned: BTreeMap<&str, u32> = BTreeMap::new();
    for (agent, n) in &counts {
        match agent {
            Some(a) => *pinned.entry(a).or_default() += *n as u32,
            None => pool_size += *n as u32,
        }
    }

    // Stop idle workers of an agent with no work when the pool is empty. Marking dead first invalidates the token even
    // if the stop fails; the dead-worker sweep below then retries the stop next tick.
    if pool_size == 0 {
        for (id, agent, _) in workers.iter().filter(|(_, a, s)| s == "idle" && !pinned.contains_key(a.as_str())) {
            let marked = sqlx::query("UPDATE workers SET status = 'dead' WHERE id = ?1 AND status = 'idle'").bind(id).execute(pool).await?;
            if marked.rows_affected() == 0 {
                continue;
            }
            eprintln!("scheduler: stopping idle {agent} worker {id}");
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

    // Each configured agent gets a worker per ticket pinned to it; starting and idle workers count. Spare ones drain
    // the pool, which otherwise starts workers of the first configured agent. An agent not in config is never scheduled.
    let mut spare = 0u32;
    let mut starts: Vec<(&str, u32)> = vec![];
    for agent in config.agents.keys() {
        let pending = workers.iter().filter(|(_, a, s)| a == agent && (s == "starting" || s == "idle")).count() as u32;
        let need = pinned.get(agent.as_str()).copied().unwrap_or(0);
        spare += pending.saturating_sub(need);
        starts.push((agent, need.saturating_sub(pending)));
    }
    if let Some((_, n)) = starts.first_mut() {
        *n += pool_size.saturating_sub(spare);
    }
    let mut slots = slots;
    for (agent, n) in starts {
        for _ in 0..n.min(slots) {
            let w = api::new_worker(pool, config, agent).await.map_err(|e| anyhow::anyhow!("creating worker: {e:?}"))?;
            let req = StartWorker {
                worker_id: w.id.clone(),
                agent: w.agent,
                orchestrator_url: config.orchestrator.public_url.clone().unwrap(),
                worker_token: w.token,
            };
            eprintln!("scheduler: starting {agent} worker {}", w.id);
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
