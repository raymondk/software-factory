# Issue tracker: GitHub

Issues and specs live as GitHub issues on `raymondk/software-factory-2`. Use `gh-axi` (wrapper around `gh`) for all operations; it infers the repo from the clone.

## Conventions

- Create: `gh-axi issue create --title "..." --body "..."` (heredoc for multi-line bodies)
- Read: `gh-axi issue view <number>` including comments and labels
- List: `gh-axi issue list` with `--label` / `--state` filters
- Comment: `gh-axi issue comment <number> --body "..."`
- Labels: `gh-axi issue edit <number> --add-label "..."` / `--remove-label "..."`
- Close: `gh-axi issue close <number> --comment "..."`

## Pull requests as a triage surface

**PRs as a request surface: no.** _(Set to `yes` to treat external PRs as feature requests.)_

GitHub shares one number space across issues and PRs: resolve a bare `#42` with `gh-axi pr view 42`, falling back to `gh-axi issue view 42`.

## When a skill says "publish to the issue tracker"

Create a GitHub issue.

## When a skill says "fetch the relevant ticket"

Run `gh-axi issue view <number>`.
