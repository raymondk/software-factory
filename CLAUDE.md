# CLAUDE.md

Software Factory: developers feed it work; it modifies tickets, makes pull requests, reviews, merges, releases, and potentially deploys. Built from separate components.

Status: design in `SPEC.md`, broken into GitHub issues. Implementation in progress as a Cargo workspace under `crates/`.

- Write as little as possible. Do not add docs, structure, or conventions that were not asked for.
- Use `gh-axi` for GitHub operations.
- Commit and push only when asked.
- UI changes follow the ICP brand guidelines at https://jgwns-tqaaa-aaaao-ba5ua-cai.icp0.io/ (tokens in `crates/orchestrator/ui/src/style.css`).

## Agent skills

### Issue tracker

GitHub Issues on `raymondk/software-factory-2` via `gh-axi`. See `docs/agents/issue-tracker.md`.

