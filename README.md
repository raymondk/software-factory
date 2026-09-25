# Software Factory

Developers feed it tickets. Agents carry them through the delivery lifecycle: implementing, opening pull requests,
and later reviewing, merging, releasing, and deploying. Design in `SPEC.md`.

## Goals

- A team's ticket queue is the only input. Humans refine tickets, set their order, and review the results.
- Workers are disposable. Any worker can pick up any available ticket, and a ticket survives its worker dying.
- Humans steer through comments on the ticket. The agent reads them; nothing else is needed to redirect it.
- Credentials stay where workers run. The orchestrator never sees a git or agent token.
- Cost is visible per ticket.

## Components

- **Orchestrator**: owns tickets, comments, workers, and usage. Serves the REST API and the web UI. Decides when
  workers are needed and reaps ones that stop heartbeating.
- **Worker Provider**: a separate process that runs where the workers run. Starts and stops workers on request and
  holds the credentials they need, so the orchestrator never sees them.
- **Worker**: a runtime around an agent. Polls the orchestrator for a ticket, runs the agent with the prompt for the
  ticket's state, reports usage, and repeats until stopped.
- **CLI**: `factory`, which wraps the REST API. Used by the agent inside a worker to read and update its ticket, and by
  developers to feed the factory.

## Feeding it work

A developer works with an agent locally, with the CLI available and the factory skill loaded (`skills/factory`). The
agent creates the tickets as the conversation produces them, with `factory ticket create`, and the developer refines
them, reorders them, and marks them `ready`:

    factory ticket edit 12 --state ready

The factory picks up `ready` tickets in rank order. Comments left on a ticket are read by the agent working it, so
that is how a developer redirects work in flight.

Flow: a ticket moves to `ready`. The scheduler asks the provider for a worker. The worker polls, gets the ticket and
its prompt, and the agent implements the change, opens a pull request, links it, and moves the ticket to `in_review`.
A human reviews. If the worker dies mid-run, the ticket keeps its state and the next worker resumes it.


## Demo

`demo/demo.sh` runs one ticket end to end: it builds the binaries and the worker image, starts the orchestrator and
the Docker provider, creates a ticket, marks it `ready`, and prints the ticket's changes until it leaves the queue.
The scheduler then stops the worker.

    DEMO_AGENT=command demo/demo.sh "Try the factory"

runs a shell stand-in for the agent and needs no credentials. `DEMO_KILL=1` kills the worker container mid-run to
show the ticket staying `in_progress` and a fresh worker resuming it. For the real thing, edit `repos` in
`demo/factory.toml`, export `GIT_TOKEN` and `CLAUDE_CODE_OAUTH_TOKEN`, and drop `DEMO_AGENT`.

The orchestrator and the provider log to stderr with timestamps and levels. `RUST_LOG` sets verbosity: `info` (the
default) covers requests that change something, worker and run lifecycle, ticket state changes, and provider
start/stop; `RUST_LOG=debug` adds every request, heartbeats, polls, scheduler ticks, and each docker command.

## Running a provider

`docs/provider.md`: prerequisites, install, configuration, running, registering it with the orchestrator, and troubleshooting.

## Releases

Pushing a tag `vX.Y.Z` (matching `version` in `Cargo.toml`, with a section in `CHANGELOG.md`) runs `.github/workflows/release.yml`:
binaries for linux x86_64 and aarch64 on a GitHub release, and the worker image at `ghcr.io/raymondk/software-factory/worker:vX.Y.Z`
(also `latest`). The image copies the released binaries rather than building from source; `scripts/worker-image.sh` builds it locally,
compiling `worker` and `factory` in a bookworm container so they match the image's glibc.

## UI

The UI is a Vite + Preact project in `crates/orchestrator/ui/`; `cargo build` runs `npm run build` and embeds the result, so run
`npm ci` there once. `npm run dev` serves it against an orchestrator on `localhost:8080` (set `VITE_TOKEN`). `npm test` runs unit
tests; `npm run e2e` runs the Playwright tests, which start their own orchestrator (once: `npx playwright install chromium`).
