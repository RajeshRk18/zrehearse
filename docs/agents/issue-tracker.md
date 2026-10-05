# Issue tracker: GitHub

Issues and PRDs for this repo live as GitHub issues in RajeshRk18/zrehearse. Use the `gh` CLI for all operations. `gh` finds the repo from `git remote -v`.

Creating, commenting on, labelling or closing an issue publishes content. Ask the user before each one.

## Conventions

- **Create an issue.** `gh issue create --title "..." --body "..."`. Use a heredoc for multi-line bodies.
- **Read an issue.** `gh issue view <number> --comments`.
- **List issues.** `gh issue list --state open --json number,title,body,labels,comments --jq '[.[] | {number, title, body, labels: [.labels[].name], comments: [.comments[].body]}]'`. Add `--label` and `--state` filters as needed.
- **Comment on an issue.** `gh issue comment <number> --body "..."`
- **Apply or remove labels.** `gh issue edit <number> --add-label "..."` or `--remove-label "..."`
- **Close.** `gh issue close <number> --comment "..."`

## When a skill says "publish to the issue tracker"

Create a GitHub issue.

## When a skill says "fetch the relevant ticket"

Run `gh issue view <number> --comments`.
