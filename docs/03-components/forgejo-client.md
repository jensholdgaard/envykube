# Forgejo Client

The Forgejo REST API client at `crates/pool-manager/src/forgejo.rs` (503 lines).
Provides all interaction with the local Forgejo Git server used for agent workspace
repos.

## HTTP Client

- Built on `reqwest` with `rustls-tls` (no `native-tls` dependency).
- Auth: Basic auth with credentials from `FORGEJO_USER`/`FORGEJO_PASSWORD` env vars
  (default: `adminadmin`/`adminadmin`).
- Base URL: `http://forgejo.localhost:3000/api/v1` (configurable via `forgejoApi` in
  `pool-config.json`).
- Repo: `adminadmin/board-integration-test` (configurable).

## Label Methods

| Method | Description |
|--------|-------------|
| `load_labels()` | Load all repository labels |
| `ensure_labels()` | Create labels if they don't exist yet |
| `set_issue_labels()` | Replace the entire label set on an issue (PUT) |
| `add_issue_labels()` | Add labels without removing existing ones (POST) |
| `remove_issue_label()` | Remove a single label by name (DELETE by label ID) |
| `issue_has_label()` | Check whether an issue currently carries a label — used as the chaos gate's read side |

`add_issue_labels` vs `set_issue_labels` matters: `set_issue_labels` replaces the
whole set, which would drop `in-progress` every time the chaos gate records a verdict.
The gate needs add/remove semantics, not replace.

## Issue Methods

| Method | Description |
|--------|-------------|
| `get_issue()` | Fetch a single issue by number — returns `None` on non-200 |
| `get_issues()` | List issues with `state` filter (`open`/`closed`) and optional `labels` filter |

## Pull Request Methods

| Method | Description |
|--------|-------------|
| `get_pull_requests()` | List PRs with `state` filter, sorted desc |
| `get_reviews()` | List reviews for a PR |
| `post_review()` | Create a review with state (`APPROVED`/`REQUEST_CHANGES`/`COMMENT`) and body — used by the chaos enforcer for the blocking review |

## Comment Methods

| Method | Description |
|--------|-------------|
| `get_issue_comments()` | List comments for an issue (paginated by Forgejo) |
| `post_issue_comment()` | Post a comment as the admin operator |
| `post_comment_as_agent()` | Post a comment with a specific agent's Basic auth |

Agent comment auth uses username `pool-{index}` with password
`pool-agent-{index}-pass`, authenticating via `auth_header_for()`.

## Agent Identity Management

| Method | Description |
|--------|-------------|
| `create_agent_user()` | Creates a Forgejo user `pool-{index}` + grants write access to the repo + creates an API token. Idempotent — deletes the stale user first. |
| `delete_agent_user()` | Deletes the Forgejo user via admin API (expects 204) |
| `write_agent_creds()` | Writes `.forgejo` file (with `FORGEJO_USER`, `FORGEJO_PASSWORD`, `FORGEJO_URL`) and a `.gitignore` to the workspace |

The `.gitignore` written by `write_workspace_gitignore()` excludes `.forgejo`, `.env`,
`.agent-name`, `.devcontainer/`, `.opencode/`, and `AGENTS.md` — so agents never commit
dev-environment files.

## API Types

The client defines Rust types for `Label`, `Issue`, `PullRequest`, `BranchRef`,
`Review`, `Comment`, and `User`, all derived from `serde::Deserialize` against the
Forgejo REST API JSON responses.
