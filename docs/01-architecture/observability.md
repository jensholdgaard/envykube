# Observability

The platform runs a shared LGTM stack (Loki + Tempo + Mimir + Grafana) in the
host cluster's `observability` namespace. Every agent shares this single stack.
Telemetry flows from agent workloads through an OTel Collector that is
replicated into each vCluster.

## Stack Components

| Component | Source | Version / Image | Values File |
|---|---|---|---|
| **Loki** (logs) | `grafana/loki` upstream chart | 7.1.0 | `values/loki.yaml` |
| **Tempo** (traces) | `grafana/tempo` upstream chart | 1.24.4 | `values/tempo.yaml` |
| **Grafana** (dashboards) | `grafana/grafana` upstream chart | 10.5.15 | `values/grafana.yaml` |
| **Mimir** (metrics) | `charts/observability` custom chart | `grafana/mimir:3.1.4` monolithic (`-target=all`) | `charts/observability/values.yaml` |
| **OTel Collector** | `charts/observability` custom chart | `otel/opentelemetry-collector-contrib:0.157.0` | `charts/observability/values.yaml` |

### Loki

Single-binary, filesystem-backed. The upstream chart defaults to
`SimpleScalable` (requires object storage), so the values file explicitly sets
`SingleBinary` mode and zeros out the scalable replica counts. Uses Loki's
built-in throwaway test schema — the log store is disposable along with the
cluster.

### Tempo

Single-binary, filesystem-backed (the chart defaults are already correct).
The values file exists so every upstream release is configured the same
declarative way.

### Mimir — Monolithic

One deployment, no pod sprawl. The full `grafana/mimir-distributed` chart deploys
25+ pods (ingester zones, store-gateway zones, Kafka, MinIO) — too heavy for a
dev platform. Instead, the `charts/observability` custom chart runs Mimir as a
single process with `-target=all`, filesystem storage, and multitenancy disabled.

Multitenancy MUST be off (`multitenancy_enabled: false`) — otherwise every push
is rejected for a missing `X-Scope-OrgID` header. All data lands under the
implicit "anonymous" tenant.

**Metric label promotion**: the Mimir configuration promotes these OTel resource
attributes into Prometheus labels so they can be filtered in Grafana:

| OTel Resource Attribute | Prometheus Label |
|---|---|
| `service.name` | `service_name` |
| `service.namespace` | `service_namespace` |
| `service.version` | `service_version` |
| `k8s.namespace.name` | `k8s_namespace_name` |
| `k8s.pod.name` | `k8s_pod_name` |
| `deployment.environment` | `deployment_environment` |

Configured via `limits.promote_otel_resource_attributes` (Mimir requires a
comma-separated string, not a YAML list).

### OTel Collector

Deployed in the host `observability` namespace. Three pipelines:

```
traces   → otlp → Tempo  (tempo.observability.svc.cluster.local:4317)
metrics  → otlp + kube-state-metrics scrape → Mimir  (mimir.observability.svc.cluster.local:8080/otlp)
logs     → otlp → Loki   (loki.observability.svc.cluster.local:3100/otlp)
```

Exposed as a NodePort Service on port `30417` (gRPC) so DevContainers can
reach it at `host.docker.internal:30417`. Also replicated into each vCluster's
`observability` namespace via `networking.replicateServices.fromHost` so that
`otel-collector.observability.svc.cluster.local:4317` resolves natively inside
the vCluster.

### Grafana

Deployed from the upstream chart with sidecar-based datasource and dashboard
provisioning enabled. Accessible at `http://grafana.platform.localhost`.

## How Agents Send Telemetry

Agent workloads send OTLP to:

```
otel-collector.observability.svc.cluster.local:4317
```

This FQDN works inside the vCluster because the OTel Collector Service is
replicated into each vCluster's `observability` namespace during `just provision`
(`networking.replicateServices.fromHost` in `values/vcluster.yaml`).

The opencode agent itself exports telemetry via the `@devtheops/opencode-plugin-otel`
plugin, configured in the per-agent `opencode.json` to send OTLP to
`http://host.docker.internal:30417` (gRPC, from the DevContainer to the host).

## Dashboard Sync

Agents do **not** create dashboards through the Grafana API. Instead:

1. The agent creates a ConfigMap in its vCluster with the annotation
   `platform.agentic-k8s.dev/dashboard: "true"`. The ConfigMap contains
   a JSON dashboard payload.
2. `just watch-dashboards` (or a one-shot `just sync-dashboards`) runs on the
   host, polls every agent's vCluster for annotated ConfigMaps, and pushes them
   to Grafana using the **host-side admin credentials**.
3. Dashboards are tagged `agent:<name>` and `managed-by:agentic-platform` for
   lifecycle tracking. Stale dashboards (present in Grafana but absent from the
   vCluster) are deleted automatically.

This design keeps the Grafana MCP server read-only — the token the agent uses
inside its DevContainer only needs to query telemetry, not create or modify
dashboards. Dashboard writes happen on the operator plane.

## Grafana MCP Server

The `mcp-grafana` binary runs inside each agent's DevContainer. It authenticates
with a Grafana service account token (`GRAFANA_SERVICE_ACCOUNT_TOKEN` from `.env`).
The token is provisioned by `just grafana-token` (Editor role, re-minted on
every observability deploy). The MCP server performs only read operations
(querying logs, traces, metrics) — all dashboard mutations are handled by the
host-side ConfigMap sync.

## Deploying the Stack

```bash
just observability
```

This one command:
1. Creates the `observability` namespace
2. Installs Loki, Tempo, and Grafana from upstream charts with pinned versions
   and custom values files (`values/*.yaml`)
3. Installs the custom `charts/observability` Helm chart: Mimir (monolithic),
   OTel Collector (with all pipelines), Grafana datasource configurations,
   and the Grafana Ingress
4. Mints the Grafana service account token and writes it to `.env`

Grafana is then reachable at `http://grafana.platform.localhost`.
