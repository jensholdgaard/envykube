# API Endpoints

Key API endpoints used across the platform.

## Forgejo REST API

Base URL: `http://forgejo:3000/api/v1`

### Labels

- `GET /repos/{repo}/labels` — list labels
- `POST /repos/{repo}/labels` — create label

### Issues

- `GET /repos/{repo}/issues` — list issues (`state=open`, optional `labels` filter)
- `GET /repos/{repo}/issues/{n}` — get issue (includes labels array)
- `POST /repos/{repo}/issues/{n}/comments` — create comment
- `PUT /repos/{repo}/issues/{n}/labels` — set labels (replaces entire set)
- `POST /repos/{repo}/issues/{n}/labels` — add labels (does not remove existing)
- `DELETE /repos/{repo}/issues/{n}/labels/{id}` — remove label by ID

### Pull Requests

- `GET /repos/{repo}/pulls` — list PRs (`state=open`, `sort=desc`)
- `POST /repos/{repo}/pulls` — create PR
- `GET /repos/{repo}/pulls/{n}/reviews` — list reviews
- `POST /repos/{repo}/pulls/{n}/reviews` — create review (`event`:
  `APPROVE`/`REQUEST_CHANGES`/`COMMENT`)
- `POST /repos/{repo}/pulls/{n}/merge` — merge PR

### Admin / User Management

- `POST /admin/users` — create user (admin only, `must_change_password: false`)
- `PUT /repos/{repo}/collaborators/{user}` — add collaborator (`permission: write`)
- `POST /users/{user}/tokens` — create API token (`scopes: ["all"]`)

## OpenCode API

Per agent, port `32000 + index`.

### Agent Endpoints

- `GET /` — health check
- `POST /api/session` — create session
- `POST /api/session/{id}/message` — send prompt/message

The pool-manager uses these via the `Agent` struct in
`crates/pool-manager/src/agent.rs`.

## OpenCode Relay

Port `30999`.

- `GET /` — list all agents with session counts
- `/agent/{name}/api/*` — proxy to that agent's OpenCode API

Used by the operator-side health monitor to check agent liveness via a health check
GET on the relay.

## Grafana

URL: `http://grafana.platform.localhost`

### Dashboards

- `POST /api/dashboards/db` — create/update dashboard (`overwrite: true`)
- `GET /api/dashboards/uid/{uid}` — get dashboard
- `DELETE /api/dashboards/uid/{uid}` — delete dashboard
- `GET /api/search` — search dashboards (with tag filters)

Used by `scripts/sync-dashboards.py` for the dashboard-as-CM sync.

### Service Accounts

- `GET /api/serviceaccounts/search` — list service accounts
- `POST /api/serviceaccounts` — create service account
- `PATCH /api/serviceaccounts/{id}` — update service account
- `GET /api/serviceaccounts/{id}/tokens` — list tokens
- `POST /api/serviceaccounts/{id}/tokens` — create token (key returned once)
- `DELETE /api/serviceaccounts/{id}/tokens/{tokenId}` — revoke token

Managed by `just grafana-token` in the justfile.

### MCP Servers

- **Grafana** — read-only telemetry queries (Prometheus, Loki, Tempo, Pyroscope,
  dashboards)
- **Kubernetes** — cluster operations (context from `$KUBECONFIG`)
