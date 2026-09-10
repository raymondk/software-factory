---
name: factory
description: Work a Software Factory ticket with the factory CLI. Use when FACTORY_TICKET is set or the task mentions a factory ticket.
---

# Factory ticket workflow

Your ticket id is `$FACTORY_TICKET`. `factory` is pre-configured (`FACTORY_URL`, `FACTORY_TOKEN`); `git` and `gh` are authenticated.

1. Read the ticket, its links, and its comments: `factory ticket view $FACTORY_TICKET`. Unresolved human comments are guidance; follow them.
2. Start: `factory ticket edit $FACTORY_TICKET --state in_progress`
3. Repos live under the current directory. Reuse an existing clone (`git fetch`, check for an existing branch or PR with `gh pr list --head <branch>`); otherwise `git clone <url>`.
4. Work on a branch, push it, open a PR: `gh pr create --title "..." --body "..."`
5. Record the PR: `factory ticket edit $FACTORY_TICKET --add-link <pr-url>`
6. Finish: `factory ticket edit $FACTORY_TICKET --state in_review`

Other commands:

- Question or blocker: `factory ticket comment $FACTORY_TICKET --body "..."`. Leave the ticket `in_progress` only if another run should resume it; otherwise move it.
- Stuck: comment why, then `factory ticket edit $FACTORY_TICKET --state failed`
- Follow-up work: `factory ticket create --title "..." --description "..."`
- Mark a comment handled: `factory ticket resolve $FACTORY_TICKET <comment-id>`

States: `todo`, `ready`, `in_progress`, `in_review`, `failed`, `done`. Never leave your ticket `in_progress` unless resuming is intended: an unmoved ticket is marked `failed` when you exit.
