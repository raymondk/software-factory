# Design notes

Working notes. Will be cleaned up into a proper spec later.

## Components

### Orchestrator
- What users interact with.
- Web front end: list of tickets, modify tickets, change their state, see what work is queued.
- Everything available through a REST API.
- Schedules workers: decides when to ask a provider for a new worker based on queue size, workers available, worker types needed, and how parallelizable the work is.
- Exposes metrics to users: worker activity, token consumption, dollars billed, and similar. Granularity: per ticket, per worker, and per agent type.

### Worker Provider
- Abstraction. The orchestrator asks it for a new worker; it provides one.
- Implementations differ in where workers run.

### Worker
- Instantiated by a provider. Runtime doesn't matter: container, VM, Kubernetes pod.
- Contains an agent. Agent doesn't matter: Claude, Codex, Pi, etc.
- Receives all of its work from the orchestrator queue. Executes it, then updates ticket status, pushes a PR, reviews a PR, etc.
- Long-lived: picks up multiple tasks in sequence, refreshing the agent session between tasks so repos and tooling stay cloned and warm rather than rebuilding from scratch each time.

## Customization
- The factory is generic. Each project tailors it to their needs.
- One orchestrator per project. A project may span multiple repositories; which repos a work item touches is for the agent to figure out, not the orchestrator.
- Per project: agent type, the prompts given to the agent, and similar.
- Configuration is easy to customize: which worker provider types to use and how to connect to them.

## MVP
One ticket goes end to end through one worker.

In:
- Orchestrator: REST API + minimal web UI. List tickets, create one, change state, see the queue.
- Tickets owned by the orchestrator. No external tracker.
- One provider implementation (local process or container).
- One worker, one agent. Takes a task, does the work, opens a PR, updates ticket state.
- Fixed per-project config file: provider, agent, prompt.
- Metrics: tokens and cost per ticket, reported by the worker, shown in the UI.

Out:
- Multiple providers, worker types, affinity, parallelism-aware scheduling.
- Review, merge, release, deploy. Worker stops at "PR opened".
- Long-lived workers. One task per worker.
- Per-worker and per-agent-type metrics.
- External tracker integration.

## Architecture

### Decided: pull model
- Worker registers with the orchestrator, then repeatedly asks for the next task matching its type.
- Worker reports progress, results, and usage back over the same REST API.
- Orchestrator reclaims a task if the worker stops heartbeating.
- Why: workers only need outbound connectivity, so the provider contract is just "start a worker with this orchestrator URL and token". Long-lived workers and affinity fall out naturally. One API surface for UI, users, and workers.

```
Web UI ──▶ Orchestrator REST API ◀── Worker (polls for tasks, reports back)
                  │
                  │ "start a worker"
                  ▼
           Worker Provider ──▶ starts Worker with orchestrator URL + token
```

### Decided: the ticket is the unit of work
- A worker is told "work on ticket N". What it does depends on the ticket's state.
- The orchestrator picks the prompt based on state. Per-project config is, at its core, a map from state to prompt plus which worker type handles that state.
- Some states are human-only (e.g. awaiting approval, done). Workers never pick those up. This is where human gates live.
- A worker leases the ticket while working. Lease is released when it moves the ticket to a new state or its heartbeat stops.
- Adding a stage later (review, release, deploy) is a new state and a new prompt in config. No new concept.
- Gives up parallel work within one ticket. States are sequential anyway.

### Decided: ticket states and assignee
States:
- `todo`: being refined. No assignee.
- `ready`: ready to be worked on. No assignee.
- `in progress`: being worked on. Assignee is the worker doing it.
- `in review`: resulting work is being reviewed. Assignee is the worker reviewing it, if one is.
- `failed`: needs human intervention.
- `done`: acceptance criteria met.

Assignee:
- A worker or a human. Replaces the lease.
- Set when a worker picks up the ticket, cleared when it moves the ticket on or its heartbeat stops.
- A ticket in `in progress` or `in review` with no assignee is available for a worker to pick up.

MVP: workers pick up `ready`, move to `in progress`, then to `in review` once the PR is open. Review is human. Later, `in review` becomes workable and `todo` can get a refine prompt.

### Decided: Worker Provider interface
- `start(worker_type) -> worker_id`: launch a worker. Worker receives orchestrator URL, auth token, and worker type as environment.
- `stop(worker_id)`: tear it down.
- `list() -> [worker_id, status]`: what the provider believes is running, so the orchestrator can reconcile against heartbeats.
- Everything else (repos, agent, prompt) the worker fetches from the orchestrator after registering. The provider stays ignorant of the work.
- MVP implementation: Docker container provider. Later: Kubernetes, cloud VMs.

### Decided: Worker internals
Worker is a thin runtime around an agent adapter.

Loop:
1. Register with the orchestrator, send heartbeats.
2. Poll for a ticket matching its worker type.
3. Fetch the ticket, the prompt for its state, and repo details.
4. Prepare the workspace: clone or reuse repos.
5. Run the agent through the adapter.
6. Report usage and outcome, move the ticket to the next state or to `failed`.
7. Go to 2.

Agent adapter:
- `run(prompt, workspace) -> outcome, usage`
- Outcome: success or failure, summary, links (e.g. PR URL).
- Usage: whatever the agent exposes, normalized to at least input tokens, output tokens, cost if known.
- MVP adapter: Claude Code CLI, non-interactive. Later: Codex, Pi, others.

Credentials:
- The provider is configured with the credentials and passes them to the worker at start. Not the orchestrator.
- Git token for push and PRs, plus agent credentials.
- For Claude: use the OAuth token (Claude subscription), not an API key, to save on costs.

### Decided: Orchestrator internals
- **REST API**: tickets, workers, metrics, plus worker-facing endpoints for register, heartbeat, poll, report.
- **Store**: tickets, assignees, worker registry, usage records. MVP: SQLite.
- **Scheduler**: periodic loop. Counts workable tickets per worker type and idle/busy workers, asks the provider to start or stop workers to close the gap. MVP rule: one worker per workable ticket, up to a configured max.
- **Reaper**: marks workers dead after missed heartbeats, clears their assignee so the ticket becomes available again.
- **Web UI**: static front end on the REST API.
- **CLI**: talks to the REST API. Same capabilities as the UI. Meant to be used by an agent, so a developer working with an agent can have it read and update tickets in the orchestrator. Workers can use it too.

Tickets:
- **ACL**: a ticket may be modified only by its assignee, the orchestrator, or a human. Any worker may create new tickets.
- **Comments**: tickets carry a comment thread. Any worker or human can comment on any ticket. Comments can be resolved, so it is clear what is still relevant (e.g. a comment asks for a change, a user makes it and resolves the comment).
- **Identity**: MVP uses a single shared token for humans. Worker tokens identify workers.

Per-project config file, loaded at startup:
- Repos the project spans.
- Provider type and settings.
- Worker types, each with agent and the state-to-prompt map.
- Scheduler limits (max workers).

### Decided: tech stack
- **Orchestrator**: Rust, Axum for the REST API, SQLite via SQLx. Single binary that also serves the web UI.
- **Worker**: Rust binary in a Docker image with the Claude Code CLI and git.
- **CLI**: Rust, sharing an API client crate with the worker.
- **Web UI**: minimal. Plain HTML and a little JavaScript, embedded in the orchestrator binary.
- **Config**: one TOML file per project.

## Open items
Post-MVP:
- Worker affinity: prefer a worker that already has the relevant repo warm. Pull model makes this a poll parameter.
- Parallelizability: ticket dependencies, so the scheduler knows what can run at once.
- External tracker sync (GitHub Issues, Jira). MVP: orchestrator owns tickets.
- Source of truth for cost: what usage data each agent exposes, and whether workers report tokens or dollars.
- Real identity and per-user auth. MVP: single shared token.
