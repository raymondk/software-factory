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

