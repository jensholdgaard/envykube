# Troubleshooting

Common issues and their resolutions, drawn from the end-to-end agent run retrospective
and the chaos gate.

## Budget & Quota

### Pending pods stuck after raising the quota

The vCluster `pod-syncer` attempts the host-side create ~2 times, records a `SyncError`,
and then **abandons the pod**. It does not re-reconcile when the blocker clears. After
raising the quota, delete the Pending pods so the ReplicaSet recreates them.

### Phantom CPU demand exhausting limits.cpu

If the ResourceQuota includes `limits.cpu` and the LimitRange sets a CPU default, every
container (including initContainers and sidecars) gets an invented CPU limit — that
default, multiplied across ~25 containers, is what exhausts the quota. The platform
governs CPU by `requests.cpu` only; `limits.cpu` is not in the quota.

### New values aren't being applied

The `pod-syncer` `SyncError` event is currently the only signal an agent gets when it
hits quota. It is easy to miss. Check `kubectl describe quota` (from the host, since
the quota lives in `vc-<name>` and is not synced into the vCluster).

## DNS and service discovery

### `otel-collector.observability.svc.cluster.local` does not resolve

vCluster CoreDNS knows nothing about the host cluster by default. The platform configures
`networking.replicateServices.fromHost` (for `observability/otel-collector`) and
`networking.advanced.fallbackHostCluster: true` in `values/vcluster.yaml`. These are
applied at `just provision` time.

The `observability` namespace must exist **inside the vCluster** for replication to work
— `provision` creates it and polls for the replicated Service.

**Caveat:** only fully-qualified names resolve. `mimir.observability` NXDOMAINs;
`mimir.observability.svc.cluster.local` resolves.

### `curl` against `*.localhost` fails

`curl` (8.5.0+) hard-codes RFC 6761: any host ending in `.localhost` resolves
internally to `::1`/`127.0.0.1`, bypassing `/etc/hosts` and NSS entirely. Workaround:

```bash
curl --resolve '*:80:172.17.0.1' http://app.<name>.localhost/healthz
```

Go and Python resolvers read `/etc/hosts` normally, so `GRAFANA_URL` stays correct
for the MCP server.

## MCP servers

### Grafana MCP: `mcp-grafana` not found

`mcp-grafana` is installed via `uv tool install`, which by default places tools under
`/root/.local/share/uv/tools/`. The `agent` user cannot traverse `/root` (`drwx------`).
Check with `which mcp-grafana`. The fix is to set `UV_TOOL_DIR=/opt/uv/tools` and
`UV_TOOL_BIN_DIR=/usr/local/bin` during the image build.

### Kubernetes MCP: exits 0 but installed nothing

`kubectl-mcp-server` is an npm shim that pip-installs `kubectl-mcp-tool` at first
run, which dies on Ubuntu 24.04's PEP 668 `externally-managed-environment`. The
solution is a self-contained binary that does not install anything at runtime.

## Chaos gate

### Every scenario passes suspiciously fast

The fault webhook is not being called. Check:
- `kubectl -n chaos-system get pods` — are the pods running?
- vCluster control-plane logs for `failed calling webhook`

`failurePolicy: Ignore` hides this silently. `assert-fired` is what catches it.

### `the armed fault fired 0 times`

The deploy sent no matching write. Usually `kubectl apply` short-circuits on an
unchanged object — it computes the patch client-side and sends no request at all
when the live state already matches the manifest. Scenarios that arm a fault `scale`
the live state away from the manifest first to guarantee the next apply is a real write.

### Exit 2, "no kubeconfig"

The suite runs on the host and needs the 127.0.0.1 kubeconfig variant,
`<name>.host.yaml`. The in-container variant uses `host.docker.internal` which only
resolves inside the DevContainer. Re-run `just kubeconfig <name>`.

### `/chaos` does nothing

No claimed member holds that issue, or the reconciler isn't polling. Check for
`polled new comment` in the pool-manager log. The reconciler polls comments, PRs,
and reviews every `pollInterval`.

### Agent never responds to a prompt

Check `GET localhost:3200{index}/api/session/<id>/message` for `model` and `error`
before suspecting the harness.

## Destructive operations

### `just deprovision ""` — invalid names rejected

The platform validates all agent names as DNS labels
(`^[a-z0-9]([a-z0-9-]*[a-z0-9])?$`) before any destructive step. `''`, `.`, `..`,
`/`, `a/b`, `../etc`, `-rf`, `UPPER`, `trailing-` are all rejected.

## Observability

### Mimir label schema: `service_name` missing

Mimir's OTLP ingest collapses `service.name` → `job` and drops the rest of the
resource attributes unless `-distributor.otel-promote-resource-attributes` is
configured. The platform configures this in `charts/observability/templates/mimir.yaml`
as a comma-separated string (not a YAML list — Mimir parses it as
`flagext.StringSliceCSV`).

### Datasource UIDs change between rebuilds

Without fixed UIDs, every `just observability` re-run generates new random UIDs.
The platform pins stable UIDs (`mimir`, `tempo`, `loki`) in
`charts/observability/templates/grafana-datasources.yaml`.

## Webhook TLS

### X509 error swallowed silently

The fault-webhook process reads its key and cert at startup. An upgrade that rotates
the TLS Secret without rolling the pod leaves the API server trusting a CA the
process is not serving. `failurePolicy: Ignore` swallows this silently. The Deployment
carries `checksum/tls` annotation so a genuine rotation rolls the pod.

## Quota inside the vCluster

The quota and LimitRange live in the host namespace `vc-<name>`. vCluster does not
sync them, so `kubectl describe quota` from inside the vCluster returns `not found`.
The only signal an agent gets is a `pod-syncer` event. Check quota from the host:
`kubectl --context k3d-agent-platform -n vc-<name> describe quota agent-quota`.
