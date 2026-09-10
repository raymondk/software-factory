# Software Factory: Specification

Draft.

## 1. Overview

Software Factory takes tickets from a team of developers and has agents carry them through the delivery lifecycle: implementing, opening pull requests, and later reviewing, merging, releasing, and deploying.

It is made of three components:

- **Orchestrator**: owns tickets, exposes a REST API, web UI, and CLI, and decides when workers are needed.
- **Worker Provider**: a separate process that starts and stops workers somewhere (Docker, Kubernetes, VMs) and holds the credentials they need.
- **Worker**: a runtime around an agent. Pulls tickets from the orchestrator, does the work, reports back.

One orchestrator serves one project. A project may span several repositories.

```
Web UI / CLI ──▶ Orchestrator REST API ◀── Worker (polls for tickets, reports back)
                       │
                       │ start / stop / status (REST)
                       ▼
        Worker Provider (separate process) ──▶ Worker container (agent + git + factory CLI)
```

## 2. Concepts

- **Project**: the unit of deployment. One orchestrator, one config file, one or more repositories.
- **Ticket**: the unit of work. Has a state, a rank, an optional assignee, a description, and a comment thread.
- **Assignee**: who currently holds the ticket. A worker or a human. Acts as the lease.
- **Worker type**: a named configuration of agent plus state-to-prompt map. Workers are started with a type and only pick up tickets whose state that type handles.
- **Workable state**: a ticket state that some worker type has a prompt for. Other states are human-only.

## 3. Ticket model

### 3.1 States

| State | Meaning | Assignee |
|---|---|---|
| `todo` | Being refined | None |
| `ready` | Ready to be worked on | None |
| `in_progress` | Being worked on. Workable, so an unassigned one is resumed by the next worker | Worker doing the work, or none if the previous worker died |
| `in_review` | Resulting work is under review | Worker reviewing, if any |
| `failed` | Needs human intervention | None |
| `done` | Acceptance criteria met | None |

Transitions are not restricted by the orchestrator beyond the ACL. Prompts tell workers which state to move a ticket to.

### 3.2 Assignee

- Set to the worker's id when it picks up a ticket.
- Cleared when the worker moves the ticket to another state, except to `in_progress`, which it keeps holding; also cleared when the orchestrator reaps the worker.
- A ticket in a workable state with no assignee is available to be picked up.

### 3.3 Ordering

- Each ticket has a `rank`, a float. Lower rank is served first.
- New tickets get max rank plus one, so they join the back of the queue.
- Reordering is relative: move a ticket before or after another. The orchestrator computes the new rank as the midpoint and renormalizes ranks when a gap gets too small. Clients never set the raw value.
- Poll returns the available ticket with the lowest rank.

### 3.4 Access control

- A ticket may be modified by its assignee, the orchestrator, or a human.
- Any worker may create tickets.
- Any worker or human may comment on any ticket.

### 3.5 Comments

- Each ticket has a comment thread.
- A comment has an author, body, timestamp, and a resolved flag.
- Anyone who can comment may resolve a comment. Resolution marks the comment as no longer relevant.

### 3.6 Fields

- `id`, `title`, `description`, `state`, `rank`, `assignee`, `created_at`, `updated_at`
- `links`: list of URLs such as pull requests, added by workers
- `comments`

## 4. Orchestrator

### 4.1 Responsibilities

- Store tickets, comments, worker registry, and usage records.
- Serve the REST API, web UI, and the endpoints the CLI and workers use.
- Schedule workers.
- Reap dead workers.
- Aggregate metrics.

### 4.2 REST API

All endpoints require a bearer token. MVP: one shared token for humans and the CLI, one token per worker issued at start.

Tickets:
- `GET /tickets` with optional `state` and `assignee` filters, ordered by rank
- `POST /tickets`
- `GET /tickets/{id}`
- `PATCH /tickets/{id}`: title, description, state, assignee, links. Subject to ACL.
- `POST /tickets/{id}/move`: body `{ before: id }` or `{ after: id }`. Reorders the ticket.
- `GET /tickets/{id}/comments`
- `POST /tickets/{id}/comments`
- `POST /tickets/{id}/comments/{cid}/resolve`

Workers:
- `POST /workers/{id}/register`: worker confirms it is alive. The id and token were assigned by the orchestrator before start.
- `POST /workers/{id}/heartbeat`
- `POST /workers/{id}/poll`: returns the lowest-ranked available ticket for this worker's type, with the prompt for its current state and the project's repos, or nothing. Sets assignee atomically. An optional body `{"exclude": <ticket id>}` (the ticket the worker just timed out on) makes that ticket last in line: it is returned only when nothing else is available.
- `POST /workers/{id}/usage`: report token and cost usage for a ticket.
- `GET /workers`: list workers and their status.

Metrics:
- `GET /metrics`: totals and breakdowns per ticket, per worker, and per worker type. Tokens in, tokens out, cost, tickets completed, tickets failed.

### 4.3 Scheduler

Periodic loop:
1. Count available tickets per worker type.
2. Count idle and busy workers per type.
3. For each worker needed, create a worker record with an id and token, then ask the provider to start it. Start workers until each type has one worker per available ticket, capped by `max_workers` and by the provider's remaining capacity from `/status`.
4. When a type has no available tickets, ask the provider to stop its idle workers.

Later: dependencies between tickets and worker affinity.

### 4.4 Reaper

A worker that misses heartbeats for longer than `heartbeat_timeout` is marked dead. Any ticket it holds keeps its state and has its assignee cleared. Since `in_progress` is workable, the next worker resumes it. The provider is asked to stop the dead worker.

There is no retry cap. A ticket that keeps killing workers is caught by humans watching the UI.

### 4.5 Web UI

Static HTML and JavaScript embedded in the orchestrator binary. Lists tickets in rank order, shows one ticket with comments, allows creating, editing, reordering tickets and changing state, shows workers and metrics.

Auth: the orchestrator injects the shared token into the page when serving it, and the UI sends it as a bearer header. Anyone who can load the page has the token, so the network decides who can use the UI.

**TODO**: replace token injection with real login once per-user identity exists.

### 4.6 CLI

`factory` command that wraps the REST API with the same capabilities as the UI. Configured with the orchestrator URL and a token via environment. Intended to be run by an agent, both inside a worker and by a developer working with an agent locally.

## 5. Worker Provider

A separate process. The orchestrator connects to it at a configured URL. It holds the credentials workers need and runs where the workers run, so the orchestrator never sees credentials and can be hosted anywhere.

REST API the orchestrator calls:

- `POST /workers`: body `{ worker_id, worker_type, orchestrator_url, worker_token }`. Starts a worker. The provider maps `worker_id` to its own handle (container id, pod name) internally.
- `DELETE /workers/{id}`: stops a worker.
- `GET /status`: returns the list of workers the provider believes are running with their status, plus provider-level information: total capacity (maximum workers it can run), capacity in use, and anything provider-specific.

The provider starts a worker with these environment variables: orchestrator URL, worker id, worker token, worker type, and the credentials it is configured with (git token, agent credentials). The provider knows nothing about tickets or repos.

The scheduler uses capacity from `/status` as an upper bound alongside `max_workers`.

MVP implementation: a Docker provider binary that runs the worker image on the local Docker daemon. Its own config holds the image name and credentials. Later: Kubernetes, cloud VMs.

## 6. Worker

### 6.1 Loop

1. Register with the orchestrator using the id and token from its environment. Start heartbeating.
2. Poll for a ticket.
3. Receive ticket, prompt, and repo list.
4. Prepare workspace: configure git and gh credentials. Repos are not cloned here; the agent clones what it needs, on demand, and reuses what is already present from earlier tickets.
5. Run the agent through the adapter with the prompt and the worker type's `run_timeout`. On timeout, kill the agent, comment on the ticket, and leave it in `in_progress` for another worker to resume.
6. Report usage. If the agent finished but left the ticket in `in_progress`, move it to `failed` with a comment explaining why.
7. Repeat from 2 until the orchestrator stops the worker.

### 6.2 Agent adapter

```
run(prompt, workspace, timeout) -> Outcome { success, summary, links }, Usage { tokens_in, tokens_out, cost }
```

The agent updates the ticket itself using the `factory` CLI, which is in the image and pre-configured with the worker's token. The adapter does not parse agent output to learn the result. It reads the ticket state afterwards.

MVP adapter: Claude Code CLI in non-interactive mode, authenticated with a Claude OAuth token rather than an API key. Later: Codex, Pi, others.

### 6.3 Image

Contains the worker binary, the `factory` CLI, git, the GitHub CLI, the agent CLI, and the factory skill.

### 6.4 Factory skill

A skill installed in the image, in the agent's skill location, that teaches the agent how to work with the orchestrator: read its ticket including comments, change state, add links, comment, create tickets, all through the `factory` CLI or the REST API directly. This is how human guidance left in comments reaches the agent.

### 6.5 Forge

MVP is GitHub only. Repos are cloned over HTTPS with the git token and pull requests are opened with the GitHub CLI.

## 7. Configuration

One TOML file per project, loaded by the orchestrator at startup.

```toml
[project]
name = "my-project"
repos = ["https://github.com/org/repo-a.git", "https://github.com/org/repo-b.git"]

[orchestrator]
listen = "0.0.0.0:8080"
token = "..."
heartbeat_timeout = "60s"
# public_url = "http://localhost:8080"   # how workers reach this orchestrator; defaults to the listen address

[scheduler]
max_workers = 4

[provider]
url = "http://localhost:8081"

[worker_types.default]
agent = "claude-code"
run_timeout = "1h"

[worker_types.default.prompts]
ready = """
You are working on ticket {{ticket.id}}: {{ticket.title}}.
Read the ticket and its comments with the factory CLI.
Move the ticket to in_progress, implement the change, open a pull request,
add its URL to the ticket, and move the ticket to in_review.
"""
in_progress = """
You are resuming ticket {{ticket.id}}: {{ticket.title}}.
A previous attempt was interrupted. Read the ticket and its comments,
check for an existing branch or pull request, and continue from there.
"""
```

The `prompts` table maps states to prompt templates. A worker type only picks up tickets in states it has a prompt for.

The Docker provider has its own config file:

```toml
[provider]
listen = "0.0.0.0:8081"
max_workers = 4

[docker]
image = "software-factory/worker:latest"

[worker_env]
GIT_TOKEN = "..."
CLAUDE_CODE_OAUTH_TOKEN = "..."
```

## 8. Metrics

Workers report usage per ticket after each agent run. The orchestrator stores raw records and aggregates by ticket, worker, and worker type. Exposed via `GET /metrics` and the UI.

## 9. MVP scope

One ticket goes end to end through one worker.

In:
- Orchestrator with SQLite, REST API, minimal web UI, CLI.
- Ticket states, assignee, ACL, comments.
- Docker provider as a separate process.
- Ticket ordering by rank with relative moves.
- Worker with Claude Code adapter. One worker type. Picks up `ready` and resumes `in_progress`. Opens a PR, moves to `in_review`. Loops until stopped.
- Run timeout per worker type.
- Factory skill in the worker image.
- GitHub only.
- Scheduler: one worker per available ticket, capped. Stops workers when no work is left.
- Metrics: tokens and cost per ticket.

Out:
- Multiple provider implementations, multiple worker types.
- Agent-driven review, merge, release, deploy. Review is human.
- Agent session refresh between tickets, affinity, ticket dependencies.
- Retry cap on failing tickets.
- Forges other than GitHub.
- Per-worker and per-worker-type metric breakdowns in the UI.
- External tracker sync.
- Per-user identity.

## 10. Post-MVP

- Agent session refresh between tickets, so a long-lived worker starts each ticket with a clean context but warm repos.
- Retry cap: attempt counter that moves a ticket to `failed` after too many interrupted runs.
- Other forges (GitLab, Gitea).
- Worker affinity as a poll parameter.
- Ticket dependencies and parallelism-aware scheduling.
- Additional workable states: `todo` for refinement, `in_review` for agent review, and states for merge, release, deploy.
- External tracker sync (GitHub Issues, Jira).
- Usage normalization across agents. Whether workers report tokens or dollars.
- Per-user auth.
- Kubernetes and cloud VM providers.

## 11. Tech stack

- **Orchestrator**: Rust, Axum, SQLite via SQLx. Single binary serving API and UI.
- **Docker provider**: Rust, Axum. Talks to the local Docker daemon.
- **Worker**: Rust binary in a Docker image with the Claude Code CLI, git, the GitHub CLI, the `factory` CLI, and the factory skill.
- **CLI**: Rust. Shares an API client crate with the worker.
- **Web UI**: plain HTML and JavaScript, embedded in the orchestrator.
- **Config**: TOML.
