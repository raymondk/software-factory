# Changelog

Notable changes, for people running the factory. Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

To release: rename `Unreleased` to the version and date, set the same version in `Cargo.toml`, commit, tag `vX.Y.Z`,
push the tag. The release workflow takes that version's section as the release notes and refuses a tag without one.

## [Unreleased]

### Added

- Orchestrator: tickets with states, rank, owner, agent and model, dependencies and relations, comment threads with
  resolve and unresolve, usage records and metrics per agent and model.
- Orchestrator: worker registry with heartbeats, per-owner poll, dead-worker reaping, runs with streamed logs and
  retention, and a scheduler that starts and stops workers through providers by the agents they advertise.
- Orchestrator: sign-in with Internet Identity, admin approval of users, personal tokens, and a read-only view of the
  running configuration.
- Providers: added per user through the API, the CLI and the UI, with health shown on the board. Docker provider that
  advertises agents and models, starts workers as containers, recovers them on restart, and requires a bearer token.
- Worker: polls for a ticket, runs the agent with the prompt for the ticket's state, ships logs and usage, and resumes
  interrupted tickets. Claude Code adapter, worker image with the `factory` skill, and a command adapter for tests.
- CLI `factory`: tickets, comments, links, relations, logs, providers and users.
- UI: board by state with drag-and-drop, ticket dialog with comments and runs, pretty Claude Code logs, workers,
  metrics, providers, and configuration under a cog.
- Config: `factory.toml` and `provider.toml`, with a gitignored `<name>.secrets.toml` merged over each.
- Demo script that runs one ticket end to end, with a kill-and-resume mode.
