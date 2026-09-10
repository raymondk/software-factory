# Software Factory: Specification

Draft. Derived from `NOTES.md`.

## 1. Overview

Software Factory takes tickets from a team of developers and has agents carry them through the delivery lifecycle: implementing, opening pull requests, and later reviewing, merging, releasing, and deploying.

It is made of three components:

- **Orchestrator**: owns tickets, exposes a REST API, web UI, and CLI, and decides when workers are needed.
- **Worker Provider**: an abstraction that starts and stops workers somewhere (Docker, Kubernetes, VMs).
- **Worker**: a runtime around an agent. Pulls tickets from the orchestrator, does the work, reports back.

One orchestrator serves one project. A project may span several repositories.

```
Web UI / CLI ──▶ Orchestrator REST API ◀── Worker (polls for tickets, reports back)
                       │
                       │ start / stop
                       ▼
                Worker Provider ──▶ Worker container (agent + git + factory CLI)
```

## 2. Concepts

- **Project**: the unit of deployment. One orchestrator, one config file, one or more repositories.
- **Ticket**: the unit of work. Has a state, an optional assignee, a description, and a comment thread.
- **Assignee**: who currently holds the ticket. A worker or a human. Acts as the lease.
- **Worker type**: a named configuration of agent plus state-to-prompt map. Workers are started with a type and only pick up tickets whose state that type handles.
- **Workable state**: a ticket state that some worker type has a prompt for. Other states are human-only.

## 3. Ticket model

### 3.1 States

| State | Meaning | Assignee |
|---|---|---|
| `todo` | Being refined | None |
| `ready` | Ready to be worked on | None |
| `in_progress` | Being worked on | Worker doing the work |
| `in_review` | Resulting work is under review | Worker reviewing, if any |
| `failed` | Needs human intervention | None |
| `done` | Acceptance criteria met | None |

Transitions are not restricted by the orchestrator beyond the ACL. Prompts tell workers which state to move a ticket to.

### 3.2 Assignee

- Set to the worker's id when it picks up a ticket.
- Cleared when the worker moves the ticket to another state, or when the orchestrator reaps the worker.
- A ticket in a workable state with no assignee is available to be picked up.

### 3.3 Access control

- A ticket may be modified by its assignee, the orchestrator, or a human.
- Any worker may create tickets.
- Any worker or human may comment on any ticket.

### 3.4 Comments

- Each ticket has a comment thread.
- A comment has an author, body, timestamp, and a resolved flag.
- Anyone who can comment may resolve a comment. Resolution marks the comment as no longer relevant.

### 3.5 Fields

- `id`, `title`, `description`, `state`, `assignee`, `created_at`, `updated_at`
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
- `GET /tickets` with optional `state` and `assignee` filters
- `POST /tickets`
- `GET /tickets/{id}`
- `PATCH /tickets/{id}`: title, description, state, assignee, links. Subject to ACL.
- `GET /tickets/{id}/comments`
- `POST /tickets/{id}/comments`
- `POST /tickets/{id}/comments/{cid}/resolve`

Workers:
- `POST /workers/register`: worker announces itself with its type. Returns `worker_id`.
- `POST /workers/{id}/heartbeat`
- `POST /workers/{id}/poll`: returns the next available ticket for this worker's type, with the prompt for its current state and the project's repos, or nothing. Sets assignee atomically.
- `POST /workers/{id}/usage`: report token and cost usage for a ticket.
- `GET /workers`: list workers and their status.

Metrics:
- `GET /metrics`: totals and breakdowns per ticket, per worker, and per worker type. Tokens in, tokens out, cost, tickets completed, tickets failed.

### 4.3 Scheduler

Periodic loop:
1. Count available tickets per worker type.
2. Count idle and busy workers per type.
3. Ask the provider to start workers until each type has one worker per available ticket, capped by `max_workers`.
4. Ask the provider to stop idle workers beyond what is needed.

Later: dependencies between tickets and worker affinity.

### 4.4 Reaper

A worker that misses heartbeats for longer than `heartbeat_timeout` is marked dead. Any ticket it holds has its assignee cleared and becomes available again. The provider is asked to stop it.

### 4.5 Web UI

Static HTML and JavaScript embedded in the orchestrator binary. Lists tickets, shows one ticket with comments, allows creating and editing tickets and changing state, shows workers and metrics.

### 4.6 CLI

`factory` command that wraps the REST API with the same capabilities as the UI. Configured with the orchestrator URL and a token via environment. Intended to be run by an agent, both inside a worker and by a developer working with an agent locally.

## 5. Worker Provider

Interface the orchestrator uses:

- `start(worker_type) -> worker_id`
- `stop(worker_id)`
- `list() -> [(worker_id, status)]`

The provider starts a worker with these environment variables: orchestrator URL, worker token, worker type, and the credentials it is configured with (git token, agent credentials). The provider knows nothing about tickets or repos.

MVP implementation: Docker. Runs the worker image locally. Later: Kubernetes, cloud VMs.

## 6. Worker

### 6.1 Loop

1. Register with the orchestrator. Start heartbeating.
2. Poll for a ticket.
3. Receive ticket, prompt, and repo list.
4. Prepare workspace: clone repos not yet present, fetch and reset those that are.
5. Run the agent through the adapter with the prompt.
6. Report usage. If the agent did not move the ticket out of `in_progress`, move it to `failed` with a comment explaining why.
7. Repeat from 2, or exit if configured for one ticket.

### 6.2 Agent adapter

```
run(prompt, workspace) -> Outcome { success, summary, links }, Usage { tokens_in, tokens_out, cost }
```

The agent updates the ticket itself using the `factory` CLI, which is in the image and pre-configured with the worker's token. The adapter does not parse agent output to learn the result. It reads the ticket state afterwards.

MVP adapter: Claude Code CLI in non-interactive mode, authenticated with a Claude OAuth token rather than an API key. Later: Codex, Pi, others.

### 6.3 Image

Contains the worker binary, the `factory` CLI, git, and the agent CLI.

## 7. Configuration

One TOML file per project, loaded by the orchestrator at startup.

```toml
[project]
name = "my-project"
repos = ["git@github.com:org/repo-a.git", "git@github.com:org/repo-b.git"]

[orchestrator]
listen = "0.0.0.0:8080"
token = "..."
heartbeat_timeout = "60s"

[scheduler]
max_workers = 4

[provider]
type = "docker"
image = "software-factory/worker:latest"
env = { GIT_TOKEN = "...", CLAUDE_CODE_OAUTH_TOKEN = "..." }

[worker_types.default]
agent = "claude-code"

[worker_types.default.prompts]
ready = """
You are working on ticket {{ticket.id}}: {{ticket.title}}.
{{ticket.description}}
Move the ticket to in_progress, implement the change, open a pull request,
add its URL to the ticket, and move the ticket to in_review.
"""
```

The `prompts` table maps states to prompt templates. A worker type only picks up tickets in states it has a prompt for.

## 8. Metrics

Workers report usage per ticket after each agent run. The orchestrator stores raw records and aggregates by ticket, worker, and worker type. Exposed via `GET /metrics` and the UI.

## 9. MVP scope

One ticket goes end to end through one worker.

In:
- Orchestrator with SQLite, REST API, minimal web UI, CLI.
- Ticket states, assignee, ACL, comments.
- Docker provider.
- Worker with Claude Code adapter. One worker type. Picks up `ready`, opens a PR, moves to `in_review`. Exits after one ticket.
- Scheduler: one worker per available ticket, capped.
- Metrics: tokens and cost per ticket.

Out:
- Multiple provider implementations, multiple worker types.
- Agent-driven review, merge, release, deploy. Review is human.
- Long-lived workers, affinity, ticket dependencies.
- Per-worker and per-worker-type metric breakdowns in the UI.
- External tracker sync.
- Per-user identity.

## 10. Post-MVP

- Long-lived workers that keep repos warm and refresh the agent session between tickets.
- Worker affinity as a poll parameter.
- Ticket dependencies and parallelism-aware scheduling.
- Additional workable states: `todo` for refinement, `in_review` for agent review, and states for merge, release, deploy.
- External tracker sync (GitHub Issues, Jira).
- Usage normalization across agents. Whether workers report tokens or dollars.
- Per-user auth.
- Kubernetes and cloud VM providers.

## 11. Tech stack

- **Orchestrator**: Rust, Axum, SQLite via SQLx. Single binary serving API and UI.
- **Worker**: Rust binary in a Docker image with the Claude Code CLI, git, and the `factory` CLI.
- **CLI**: Rust. Shares an API client crate with the worker.
- **Web UI**: plain HTML and JavaScript, embedded in the orchestrator.
- **Config**: TOML.
