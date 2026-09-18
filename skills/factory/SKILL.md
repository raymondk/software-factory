---
name: factory
description: Work a Software Factory ticket with the factory CLI. Use when FACTORY_TICKET is set or the task mentions a factory ticket.
---

# Factory ticket workflow

Your ticket id is `$FACTORY_TICKET`. `factory` is pre-configured (`FACTORY_URL`, `FACTORY_TOKEN`); `git` and `gh` are authenticated.

1. Read the ticket, its links, and its comments: `factory ticket view $FACTORY_TICKET`. Unresolved human comments are guidance; follow them.
2. Start: `factory ticket edit $FACTORY_TICKET --state in_progress`
3. Repo URLs are in `$FACTORY_REPOS` (space-separated). Each repo lives at `/workspace/<name>`, where `<name>` is the last path segment of its URL without `.git` (`https://github.com/org/app.git` → `/workspace/app`). If that directory exists, reuse it (`git fetch`, check for an existing branch or PR with `gh pr list --head <branch>`); otherwise `git clone <url> /workspace/<name>`. Never clone anywhere else: earlier and later runs must find the same paths.
   The Bash cwd persists between calls. Run `pwd` before `cd`; a `cd <name>` from inside the clone fails. Prefer absolute paths.
4. Work on a branch, push it, open a PR: `gh pr create --title "..." --body "..."`
5. Record the PR: `factory ticket edit $FACTORY_TICKET --add-link <pr-url>`
6. Finish, last: `factory ticket edit $FACTORY_TICKET --state in_review`. A state change releases the ticket: after it you no longer hold it, and further edits are refused with `403 … does not hold ticket`. Make every comment and link before the final state change.

Other commands:

- Question or blocker: `factory ticket comment $FACTORY_TICKET --body "..."`. Leave the ticket `in_progress` only if another run should resume it; otherwise move it.
- Stuck: comment why, then `factory ticket edit $FACTORY_TICKET --state failed`
- Follow-up work: `factory ticket create --title "..." --description "..."`
- Relate tickets: `factory ticket link <id> --depends-on <id>` or `--related-to <id>`; undo with `factory ticket unlink`. A `ready` ticket that depends on another is not worked until that one is `in_review` or `done`.
- Mark a comment handled: `factory ticket resolve $FACTORY_TICKET <comment-id>`

States: `todo`, `ready`, `in_progress`, `in_review`, `failed`, `done`. Never leave your ticket `in_progress` unless resuming is intended: an unmoved ticket is marked `failed` when you exit.
