//! Spec 4.3: starts a worker per available (owned, unassigned, unblocked) ticket on one of the owner's providers that
//! advertises what the ticket needs, and stops an owner's idle workers when their agent has no work and the owner's
//! shared pool is empty. A worker's user is the owner of its provider.

use std::collections::BTreeMap;

use sqlx::SqlitePool;

use crate::api;
use crate::config::Config;
use crate::provider::{Providers, StartWorker};

/// (owner, agent, provider) of a worker that is starting or idle, so it will take work.
type Pending = (String, String, i64);

/// One scheduling pass. Nothing to do without providers. Providers that do not answer are skipped; fails only when
/// none does or the database is unreachable. The next pass retries either way.
pub async fn tick(pool: &SqlitePool, config: &Config, providers: &Providers) -> anyhow::Result<()> {
    let (clients, statuses) = providers.refresh().await?;
    if clients.is_empty() {
        return Ok(());
    }
    if statuses.is_empty() {
        anyhow::bail!("no provider answered");
    }
    let owners: BTreeMap<i64, String> = sqlx::query_as("SELECT id, owner FROM providers").fetch_all(pool).await?.into_iter().collect();
    let workers: Vec<(String, String, i64, String)> = sqlx::query_as("SELECT id, agent, provider, status FROM workers").fetch_all(pool).await?;
    let alive = workers.iter().filter(|(.., s)| s != "dead").count() as u32;
    let mut slots = config.scheduler.max_workers.saturating_sub(alive);
    let mut free: BTreeMap<i64, u32> = statuses.iter().map(|(n, s)| (*n, s.capacity.saturating_sub(s.in_use))).collect();

    // Available tickets per (owner, agent, model). Unowned tickets have no worker to take them. Those with no agent form
    // the owner's pool, which any of their workers drains.
    let states = serde_json::to_string(&config.prompts.keys().collect::<Vec<_>>()).unwrap();
    let mut demand: Vec<(String, Option<String>, Option<String>, i64)> = sqlx::query_as(&format!(
        "SELECT owner, agent, model, COUNT(*) FROM tickets WHERE owner IS NOT NULL AND assignee IS NULL \
         AND state IN (SELECT value FROM json_each(?1)) AND NOT {} \
         GROUP BY owner, agent, model ORDER BY owner, agent IS NULL, agent, model",
        api::BLOCKED
    ))
    .bind(states)
    .fetch_all(pool)
    .await?;
    let pool_empty = |owner: &str| demand.iter().filter(|(o, ..)| o == owner).all(|(_, a, ..)| a.is_some());
    let has_work = |owner: &str, agent: &str| demand.iter().any(|(o, a, ..)| o == owner && a.as_deref() == Some(agent));

    // Stop an owner's idle workers of an agent with no work when their pool is empty. Marking dead first invalidates
    // the token even if the stop fails; the dead-worker sweep below then retries the stop next tick.
    let mut pending: Vec<Pending> = vec![];
    for (id, agent, provider, status) in &workers {
        let Some(owner) = owners.get(provider) else { continue };
        if status == "idle" && pool_empty(owner) && !has_work(owner, agent) && statuses.contains_key(provider) {
            let marked = sqlx::query("UPDATE workers SET status = 'dead' WHERE id = ?1 AND status = 'idle'").bind(id).execute(pool).await?;
            if marked.rows_affected() == 0 {
                continue;
            }
            eprintln!("scheduler: stopping idle {agent} worker {id} on {provider}");
            if let Err(e) = clients[provider].stop(id).await {
                eprintln!("scheduler: stop {id}: {e:#}");
            }
        } else if status == "idle" || status == "starting" {
            pending.push((owner.clone(), agent.clone(), *provider));
        }
    }

    for (name, status) in &statuses {
        let client = &clients[name];
        // Dead workers (reaped, or whose stop failed) the provider still runs.
        for (id, ..) in workers.iter().filter(|(id, .., s)| s == "dead" && status.workers.iter().any(|w| &w.worker_id == id)) {
            eprintln!("scheduler: stopping dead worker {id} on {name}");
            if let Err(e) = client.stop(id).await {
                eprintln!("scheduler: stop {id}: {e:#}");
            }
        }
        // Provider workers the orchestrator has no record of (restart with a fresh database): they can never register.
        for w in status.workers.iter().filter(|w| !workers.iter().any(|(id, ..)| id == &w.worker_id)) {
            eprintln!("scheduler: stopping unknown worker {} on {name}", w.worker_id);
            if let Err(e) = client.stop(&w.worker_id).await {
                eprintln!("scheduler: stop {}: {e:#}", w.worker_id);
            }
        }
    }

    // A pending worker serves a ticket when it is the owner's, runs the ticket's agent (any, for the pool) and its
    // provider advertises the ticket's model for that agent. Pinned tickets go first so the pool does not take their
    // workers.
    let supports = |provider: i64, agent: &str, model: Option<&str>| {
        model.is_none_or(|m| statuses.get(&provider).and_then(|s| s.agents.get(agent)).is_some_and(|a| a.models.iter().any(|x| x == m)))
    };
    for (owner, agent, model, n) in demand.iter_mut() {
        pending.retain(|(o, a, p)| {
            let serves = *n > 0 && o == owner && agent.as_deref().is_none_or(|x| x == a) && supports(*p, a, model.as_deref());
            if serves {
                *n -= 1;
            }
            !serves
        });
        // Start the rest on the owner's provider with the most free capacity among those advertising the agent (for the
        // pool, the first agent it advertises that is configured) and the model. Never an agent absent from [agents].
        while *n > 0 && slots > 0 {
            let mut best: Option<(u32, i64, &str)> = None;
            for (name, status) in statuses.iter().filter(|(name, _)| owners.get(name) == Some(owner)) {
                let advertised = |a: &String| config.agents.contains_key(a) && status.agents.contains_key(a) && supports(*name, a, model.as_deref());
                let chosen = match agent.as_ref() {
                    Some(a) => advertised(a).then_some(a),
                    None => status.agents.keys().find(|a| advertised(a)),
                };
                if let Some(a) = chosen {
                    let capacity = free[name];
                    if capacity > 0 && best.is_none_or(|(c, ..)| capacity > c) {
                        best = Some((capacity, *name, a));
                    }
                }
            }
            let Some((_, provider, chosen)) = best else { break };
            let w = api::new_worker(pool, config, chosen, provider).await.map_err(|e| anyhow::anyhow!("creating worker: {e:?}"))?;
            let req = StartWorker {
                worker_id: w.id.clone(),
                agent: w.agent,
                orchestrator_url: config.orchestrator.public_url.clone().unwrap(),
                worker_token: w.token,
            };
            eprintln!("scheduler: starting {chosen} worker {} on {provider}", w.id);
            *free.get_mut(&provider).unwrap() -= 1;
            slots -= 1;
            *n -= 1;
            if let Err(e) = clients[&provider].start(&req).await {
                sqlx::query("UPDATE workers SET status = 'dead' WHERE id = ?1").bind(&w.id).execute(pool).await?;
                eprintln!("scheduler: start {} on {provider}: {e:#}", w.id);
            }
        }
    }
    Ok(())
}

pub async fn run(pool: SqlitePool, config: std::sync::Arc<Config>, providers: std::sync::Arc<Providers>) {
    let mut interval = tokio::time::interval(config.scheduler.interval);
    loop {
        interval.tick().await;
        if let Err(e) = tick(&pool, &config, &providers).await {
            eprintln!("scheduler: {e:#}");
        }
    }
}
