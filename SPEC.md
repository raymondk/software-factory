# Software Factory: Specification

Draft.

## 1. Overview

Software Factory takes tickets from a team of developers and has agents carry them through the delivery lifecycle: implementing, opening pull requests, and later reviewing, merging, releasing, and deploying.

It is made of three components:

- **Orchestrator**: owns tickets, exposes a REST API, web UI, and CLI, and decides when workers are needed.
- **Worker Provider**: a separate process that starts and stops workers somewhere (Docker, Kubernetes, VMs) and holds the credentials they need. A developer adds one through the UI or CLI; it belongs to them, and its workers act as them and work only their tickets.
- **Worker**: a runtime around an agent. Pulls tickets from the orchestrator, does the work, reports back.

One orchestrator serves one project. A project may span several repositories.

```
Web UI / CLI ──▶ Orchestrator REST API ◀── Worker (polls for tickets, reports back)
                       │
                       │ start / stop / status (REST)
                       ▼
        Worker Providers (separate processes) ──▶ Worker container (agent + git + factory CLI)
```

## 2. Concepts

- **Project**: the unit of deployment. One orchestrator, one config file, one or more repositories.
- **Ticket**: the unit of work. Has a state, a rank, an optional assignee, a description, and a comment thread.
- **Assignee**: who currently holds the ticket. A worker or a human. Acts as the lease.
- **Agent**: the program doing the work (Claude Code, Codex, ...). Every worker runs one agent. Providers advertise the agents they can run; the image behind an agent is the provider's business.
- **Model**: the model an agent runs with. Each provider advertises the models it supports per agent and a default; a ticket may pick one.
- **Owner**: the developer a ticket belongs to. Only their workers pick it up. Nobody works an unowned ticket.
- **Workable state**: a ticket state that has a prompt. Prompts are shared by all agents. Other states are human-only.

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
- Poll returns the available, unblocked ticket with the lowest rank.

### 3.4 Access control

- A ticket may be modified by its assignee, the orchestrator, or a human.
- Any worker may create tickets.
- Any worker or human may comment on any ticket.
- Owner: a caller may set it to their own principal, and the current owner may clear it. Nobody can make someone else the owner. A worker's principal is its user's (5). Refused while the ticket has an assignee.

### 3.5 Comments

- Each ticket has a comment thread.
- A comment has an author, body, timestamp, and a resolved flag.
- Anyone who can comment may resolve a comment. Resolution marks the comment as no longer relevant.

### 3.6 Fields

- `id`, `title`, `description`, `state`, `rank`, `assignee`, `created_at`, `updated_at`
- `owner`: a developer, by principal. On create: the creating developer, a worker's user, none for the admin. Only the owner's workers pick the ticket up; an unowned one is never handed out. Changing it: 3.4.
- `links`: list of URLs such as pull requests, added by workers
- `comments`
- `relations`: see 3.7
- `blocked`: a `ready` ticket with an unfinished dependency
- `agent`, `model`: optional. `agent` restricts which workers may pick the ticket up; unset means any. `model` overrides the provider's default for that agent. Both must be advertised by one of the owner's providers (5); an unowned ticket accepts neither.
- `runs`: agent runs on the ticket, newest first (4.7)

### 3.7 Relations

Tickets relate to each other; `links` (URLs) is a separate concept.

- `depends_on`: directed. The ticket cannot be worked until the other is finished, meaning `in_review` or `done`.
- `related_to`: symmetric, informational. Stored once, shown on both tickets.

A relation is refused when it duplicates an existing one, points at the ticket itself, or would make `depends_on` cyclic.

A `ready` ticket with an unfinished dependency is blocked: poll never hands it out and the scheduler does not count it as available work. It stays `ready`; nothing changes its state. Only `ready` is blocked; a resumable `in_progress` ticket is handed out regardless.

Ticket JSON lists each relation with the other ticket's id, title and state, as `depends_on`, `blocks` (the other ticket depends on this one) or `related_to`; `depends_on` carries `satisfied`.

## 4. Orchestrator

### 4.1 Responsibilities

- Store tickets, comments, worker registry, usage records, runs, and worker logs.
- Serve the REST API, web UI, and the endpoints the CLI and workers use.
- Schedule workers.
- Reap dead workers.
- Aggregate metrics.

### 4.2 REST API

All endpoints require a bearer token: the admin's `orchestrator.token`, a developer's session or personal token, or a worker's token issued at start. Developers are Internet Identity principals; one who signs in is `pending` until the admin approves them under a name (`factory user approve`), and may only call `GET /me` until then. Only token hashes are stored.

Tickets:
- `GET /tickets` with optional `state`, `assignee` and `owner` filters, ordered by rank
- `POST /tickets`
- `GET /tickets/{id}`
- `PATCH /tickets/{id}`: title, description, state, assignee, owner, links, agent, model. Subject to ACL; `owner` per 3.4. `agent` and `model` are rejected unless one of the owner's providers advertised them in its last status.
- `POST /tickets/{id}/move`: body `{ before: id }` or `{ after: id }`. Reorders the ticket.
- `GET /tickets/{id}/comments`
- `POST /tickets/{id}/comments`
- `POST /tickets/{id}/comments/{cid}/resolve`, `POST /tickets/{id}/comments/{cid}/unresolve`
- `POST /tickets/{id}/relations`: body `{ type, ticket }` with `type` `depends_on` or `related_to`. Same ACL as `PATCH`.
- `DELETE /tickets/{id}/relations/{type}/{ticket}`

Workers:
- `POST /workers/{id}/register` body `{ version }` (optional): worker confirms it is alive and reports its binary's version, shown on the worker and its runs. The id and token were assigned by the orchestrator before start.
- `POST /workers/{id}/heartbeat`
- `POST /workers/{id}/poll`: returns the lowest-ranked available, unblocked ticket owned by this worker's user whose `agent` is unset or matches this worker's, and whose `model` is unset or supported by this worker's provider, with the prompt for its current state, the ticket's `model` (may be null), the project's repos, and the id of the run it opens, or nothing. Sets assignee atomically. An optional body `{"exclude": <ticket id>}` (the ticket the worker just timed out on) makes that ticket last in line: it is returned only when nothing else is available.
- `POST /workers/{id}/usage`: report token and cost usage for a ticket, and the model the agent ran with. Ends the worker's open run and records the model on it.
- `POST /workers/{id}/logs`: body `{ run, lines }`; `run` is one of the worker's runs or null.
- `GET /workers/{id}/logs?after=<line id>`: the worker's whole stream, oldest first, at most 1000 lines per call.
- `GET /runs/{id}/logs?after=<line id>`: one run's lines, same shape. `after` supports polling for live output.
- `GET /workers`: list workers with their agent, provider and status.
- `GET /config`: the running configuration, read-only, without the token, plus the orchestrator's `version`. Shown in the UI.
- `GET /version`: `{ version }`, open. Every binary also prints its version with `--version`: the crate version, plus `+<commit>` when built off a tag or with local changes.
- `GET /agents`: the agents the caller's providers advertised in their last status (a worker's: its user's), each with the union of the models advertised for it. What the UI offers when setting agent and model on a ticket.

Providers (5):
- `GET /providers`: every provider with id, owner, name, url, last status and health: `reachable` (it answered the scheduler's last check), `last_seen` (when it last answered) and `last_error` (why it last did not). The last status is kept while a provider is unreachable; health tells the two apart. Never the token.
- `GET /providers/{id}/token`: the token, so the owner can share it with the provider's operator. Owner only; 403 to everyone else, the admin included.
- `POST /providers` body `{ name, url, token }`: adds a provider owned by the calling developer; `name` is unique per owner. Not for the admin or workers.
- `PATCH /providers/{id}`: url, token. Owner only.
- `DELETE /providers/{id}`: owner or admin. Stops the provider's workers.

Login (no token): developers sign in with Internet Identity. The browser obtains a delegation chain from `https://id.ai/authorize`; the orchestrator verifies the IC-Auth envelope off-chain against the IC mainnet root key (`ic_auth_verifier`) and mints its own session token. The principal derives from the origin the UI is served from (`orchestrator.public_url`): changing that origin changes every user's principal.
- `GET /auth/challenge` → `{ challenge }`: a one-time nonce, five minutes, kept in memory.
- `POST /auth/login` body `{ envelope }`: the envelope `signMessage(identity, challenge)` produced, base64url. Verifies it, consumes the challenge, rejects the anonymous principal, creates the user as `pending` if unknown, and returns `{ token, principal, status }`; the session lasts 8 hours.
- `POST /auth/logout` (under auth): ends the calling session.

Users:
- `GET /me`: the calling developer with their `status`.
- `GET /users`: every user for the admin, approved ones for everyone else.
- `POST /users/{principal}/approve` body `{ name }`, `POST /users/{principal}/revoke`: admin only. Revoking deletes the user's sessions, personal tokens and providers, and stops the providers' workers.
- `GET /tokens`, `POST /tokens` body `{ name }`, `DELETE /tokens/{id}`: a developer's personal tokens for the CLI. The token is returned once, at creation.

Metrics:
- `GET /metrics`: totals and breakdowns per ticket, per worker, per agent and per model. Tokens in, tokens out, cost, tickets completed, tickets failed.

### 4.3 Scheduler

Periodic loop:
1. Load the providers from the database and fetch `/status` from each: capacity, workers, advertised agents. A provider that does not answer is skipped this pass.
2. Count available tickets per owner, agent and model: owned, unassigned, in a workable state, not blocked (3.7). Tickets with no `agent` form, per owner, a shared pool that any of that owner's idle workers drains.
3. Count idle and busy workers per owner and agent, and which models each can serve from its provider's status.
4. For each worker needed, create a worker record with an id, token, agent and provider, then ask that provider to start it. The provider is the one among the owner's with the most free capacity among those advertising the agent and, when the ticket sets one, the model. For an owner's shared pool, when none of their workers is idle, start on their provider with the most free capacity using the first agent it advertises. Start until each owner's agent has one worker per available ticket, capped by `max_workers` and by each provider's remaining capacity.
5. When an owner's agent has no available tickets and their shared pool is empty, ask the providers to stop its idle workers.

Later: worker affinity.

### 4.4 Reaper

A worker that misses heartbeats for longer than `heartbeat_timeout` is marked dead. Any ticket it holds keeps its state and has its assignee cleared, its open run is ended, and a comment on the ticket says the worker disappeared, linking the run's log. Since `in_progress` is workable, the next worker resumes it. The worker's provider is asked to stop it.

There is no retry cap. A ticket that keeps killing workers is caught by humans watching the UI.

### 4.5 Web UI

Static HTML and JavaScript embedded in the orchestrator binary. Lists tickets in rank order with blocked ones marked, shows one ticket with comments, relations and runs, allows creating, editing, relating, reordering tickets, setting agent and model, and changing state, shows workers with their agent and provider, and metrics. A ticket's page lets the signed-in developer make themselves its owner unless a worker holds it. A Providers view on the board lists every developer's providers read-only with owner, URL, health (reachable or not, when it last answered), workers and the agents and models each advertises with the default marked, refreshed with the board's poll. The cog at the top right opens a modal with the running configuration, read-only, and the providers the caller manages: a developer adds their own with a URL and token, edits its URL and token, reveals its token behind an eye icon and removes it; the admin removes any. A run's log opens from the ticket at `#/tickets/{id}/runs/{run}` and a worker's at `#/workers/{id}`, both following live while open. When the UI knows the agent behind a run or a worker (Claude Code today), its log opens in a pretty view that renders every event as structure, with a toggle to the raw lines; lines that are not events, such as the worker's own output, stay raw in place. The mapping from agent to renderer lives in the UI.

Auth: the page carries no token. A developer signs in with Internet Identity (4.2, Login); the UI keeps the session token in `localStorage` and sends it as a bearer header, and a 401 sends it back to the sign-in screen. A pending or revoked user sees a page with their principal and the `factory user approve` command, polling until approved. The header shows the user, a Tokens modal (personal tokens for the CLI, each shown once at creation, revocable) and logout.

### 4.6 CLI

`factory` command that wraps the REST API with the same capabilities as the UI. Configured with the orchestrator URL and a token via environment. Intended to be run by an agent, both inside a worker and by a developer working with an agent locally. `factory ticket logs <id> [--run <n>] [-f]` and `factory worker logs <id> [-f]` print logs, `-f` following until the run ends or the worker dies. `factory provider add <name> <url> <token>`, `factory provider list` and `factory provider remove <id>` manage the caller's providers.

### 4.7 Runs and logs

A **run** is one hand-out of a ticket to a worker: opened by poll, ended by the worker's usage report or by the reaper. A ticket lists its runs. Runs carry the worker's agent and, once ended by a usage report, the model it ran with.

A worker ships every line it prints and every line its agent prints to the orchestrator, tagged with the current run or with none (startup, polling, a crash before the first poll). Lines are raw text, truncated at 16 KiB, sent in batches every second or every 100 lines, whichever comes first, so a crash loses at most one batch. Interval, batch size and line limit are worker configuration (`FACTORY_LOG_INTERVAL`, `FACTORY_LOG_BATCH`, `FACTORY_LOG_MAX_LINE`).

Retention: `orchestrator.log_retention` (default 7 days). Hourly, the orchestrator deletes a run's lines once the run ended that long ago, and run-less lines that old by their own timestamp. Run rows are kept, so a ticket still lists every run; only the text goes.

## 5. Worker Provider

A separate process. A developer adds it to the orchestrator with its URL and token (4.2, Providers); the orchestrator stores both, and the owner can read and change them later, since the token is shared with whoever runs the provider. Every scheduler pass checks each provider and records whether it answered, when it last did and the last error, which the UI shows as its health. A provider belongs to the developer who added it: its workers act as that developer and work only their tickets. A provider holds one set of agent credentials, so one provider is one account; it runs where the workers run, so the orchestrator never sees those credentials and can be hosted anywhere.

A provider is configured with the agents it can run: for each, an image, the models it supports and the default among them. It advertises agents and models in `/status`; the orchestrator validates tickets and routes work against the union of what providers advertise. To pin work to an account, give that account's provider an agent name no other provider advertises.

REST API the orchestrator calls. `POST` and `DELETE` require the provider's token as a bearer header; the orchestrator holds it from when the provider was added. `GET /status` is open.

- `POST /workers`: body `{ worker_id, agent, orchestrator_url, worker_token }`. Starts a worker of that agent; 400 for an agent the provider does not advertise. The provider maps `worker_id` to its own handle (container id, pod name) internally.
- `DELETE /workers/{id}`: stops a worker.
- `GET /status`: returns the list of workers the provider believes are running with their agent and status, plus provider-level information: the provider's `version`, total capacity (maximum workers it can run), capacity in use, the advertised agents with their models and default, and anything provider-specific.

The provider starts a worker with these environment variables: orchestrator URL, worker id, worker token, agent, the agent's default model, and the credentials it is configured with (git token, agent credentials). The provider knows nothing about tickets or repos.

The scheduler uses capacity from `/status` as an upper bound alongside `max_workers`.

MVP implementation: a Docker provider binary that runs worker images on the local Docker daemon. Its own config holds the agents and credentials. Later: Kubernetes, cloud VMs.

## 6. Worker

### 6.1 Loop

1. Register with the orchestrator using the id and token from its environment. Start heartbeating.
2. Poll for a ticket.
3. Receive ticket, prompt, and repo list.
4. Prepare workspace: configure git and gh credentials. Repos are not cloned here; the agent clones what it needs, on demand, and reuses what is already present from earlier tickets.
5. Run the agent through the adapter with the prompt, the ticket's model or the default from its environment, and the agent's `run_timeout`, shipping its output as it arrives (4.7). On timeout, kill the agent, comment on the ticket, and leave it in `in_progress` for another worker to resume.
6. Report usage and the model used. If the agent finished but left the ticket in `in_progress`, move it to `failed` with a comment explaining why. Both comments link to the run's log in the UI.
7. Repeat from 2 until the orchestrator stops the worker.

### 6.2 Agent adapter

```
run(prompt, model, workspace, timeout) -> Outcome { success, summary, links }, Usage { tokens_in, tokens_out, cost, model }
```

The agent updates the ticket itself using the `factory` CLI, which is in the image and pre-configured with the worker's token. The adapter does not parse agent output to learn the result. It reads the ticket state afterwards.

MVP adapter: Claude Code CLI in non-interactive mode with `--model <model> --output-format stream-json --verbose`, one JSON event per line, usage from the final `result` event; authenticated with a Claude OAuth token rather than an API key. Later: Codex, Pi, others.

### 6.3 Image

Contains the worker binary, the `factory` CLI, git, the GitHub CLI, the agent CLI, and the factory skill.

### 6.4 Factory skill

A skill installed in the image, in the agent's skill location, that teaches the agent how to work with the orchestrator: read its ticket including comments, change state, add links, relate tickets, comment, create tickets, all through the `factory` CLI or the REST API directly. This is how human guidance left in comments reaches the agent.

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
# log_retention = "7d"                   # worker log lines older than this are purged; runs are kept
# public_url = "http://localhost:8080"   # how workers reach this orchestrator; defaults to the listen address

[scheduler]
max_workers = 4

[agents.claude-code]
run_timeout = "1h"

[prompts]
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

Providers are not configured here; developers add them at runtime (4.2). The `prompts` table maps states to prompt templates, shared by all agents. Workers only pick up tickets in states that have a prompt. An agent a provider advertises but `agents` does not list is never scheduled.

Each provider has its own config file. The Docker provider's:

```toml
[provider]
listen = "0.0.0.0:8081"
max_workers = 4
token = "change-me"

[agents.claude-code]
image = "software-factory/worker:latest"
models = ["sonnet", "opus"]
default_model = "sonnet"

[worker_env]
GIT_TOKEN = "..."
CLAUDE_CODE_OAUTH_TOKEN = "..."
```

Secrets stay out of source control the same way for both: next to `factory.toml` or `provider.toml`, an optional
`factory.secrets.toml` or `provider.secrets.toml` with the same layout is merged over it at load, table by table, and its
values win. The committed file keeps the non-secret keys (or placeholders); `*.secrets.toml` is gitignored.

## 8. Metrics

Workers report usage per ticket after each agent run. The orchestrator stores raw records and aggregates by ticket, worker, agent and model. Exposed via `GET /metrics` and the UI.

## 9. MVP scope

One ticket goes end to end through one worker.

In:
- Orchestrator with SQLite, REST API, minimal web UI, CLI.
- Ticket states, assignee, ACL, comments.
- Docker provider as a separate process.
- Ticket ordering by rank with relative moves.
- Ticket relations: `depends_on` blocks scheduling, `related_to` is informational.
- Worker with Claude Code adapter. One agent. Picks up `ready` and resumes `in_progress`. Opens a PR, moves to `in_review`. Loops until stopped.
- Run timeout per agent.
- Factory skill in the worker image.
- GitHub only.
- Scheduler: one worker per available ticket, capped. Stops workers when no work is left.
- Metrics: tokens and cost per ticket.

Out:
- Multiple providers, multiple agents, agent and model on tickets.
- Agent-driven review, merge, release, deploy. Review is human.
- Agent session refresh between tickets, affinity.
- Retry cap on failing tickets.
- Forges other than GitHub.
- Per-worker and per-agent metric breakdowns in the UI.
- External tracker sync.
- Per-user identity.

## 10. Post-MVP

- Agent session refresh between tickets, so a long-lived worker starts each ticket with a clean context but warm repos.
- Retry cap: attempt counter that moves a ticket to `failed` after too many interrupted runs.
- Other forges (GitLab, Gitea).
- Worker affinity as a poll parameter.
- Additional workable states: `todo` for refinement, `in_review` for agent review, and states for merge, release, deploy.
- External tracker sync (GitHub Issues, Jira).
- Usage normalization across agents. Whether workers report tokens or dollars.
- Multiple providers and agents as specified in 2, 4.3, 5 and 7.
- Per-user auth.
- Kubernetes and cloud VM providers.

## 11. Tech stack

- **Orchestrator**: Rust, Axum, SQLite via SQLx. Single binary serving API and UI.
- **Docker provider**: Rust, Axum. Talks to the local Docker daemon.
- **Worker**: Rust binary in a Docker image with the Claude Code CLI, git, the GitHub CLI, the `factory` CLI, and the factory skill.
- **CLI**: Rust. Shares an API client crate with the worker.
- **Web UI**: plain HTML and JavaScript, embedded in the orchestrator.
- **Config**: TOML.
