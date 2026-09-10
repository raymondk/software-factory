# Software Factory

Developers feed it work. It modifies tickets, makes pull requests, reviews code, merges, releases, and potentially deploys.

Built from separate components. Design in `SPEC.md`.

Run: `cargo run -p orchestrator -- factory.example.toml`, then `FACTORY_TOKEN=change-me cargo run -p factory -- ticket list`.
