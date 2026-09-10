# Software Factory

Developers feed it work. It modifies tickets, makes pull requests, reviews code, merges, releases, and potentially deploys.

Built from separate components. Design in `SPEC.md`.

## Demo

One ticket end to end through one worker: orchestrator, Docker provider, one container, a pull request.

Prerequisites: Docker, Rust, a GitHub repo you can push to, a git token with `repo` scope, and
`CLAUDE_CODE_OAUTH_TOKEN` from `claude setup-token`.

Stand-in agent (a shell script; no tokens or repo needed):

    DEMO_AGENT=command ./demo/demo.sh "Add a greeting" "Print hello from main"

Claude Code: set `repos` in `demo/factory.toml`, then

    export GIT_TOKEN=... CLAUDE_CODE_OAUTH_TOKEN=...
    ./demo/demo.sh "Add a greeting" "Print hello from main"

The script builds the binaries and the worker image, starts the provider and the orchestrator, creates the ticket,
moves it to `ready`, and prints state changes, comments, and links until the ticket leaves `in_progress`; then it
prints `factory metrics` and `factory worker list` and stops everything. Generated configs and the
`orchestrator.log` / `provider.log` files live in a temp dir printed on the first line.

To manually drive the demo instead: `cargo run -p orchestrator -- factory.example.toml`, then
`FACTORY_TOKEN=change-me cargo run -p factory -- ticket list`.
