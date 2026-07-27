# Charts

All four Helm charts in `charts/`, inspected via `just render` and validated via `just lint`.

## charts/agent

Per-agent guardrails applied automatically during `just provision`.

- Deployed to the agent's host namespace (`vc-<name>`).
- Contains ResourceQuota, LimitRange, and optional NetworkPolicy.
- Values at `charts/agent/values.yaml` — no `--set` needed.
- CPU governed by `requests.cpu: 4` only (no `limits.cpu` quota — avoids the LimitRange
  manufacturing phantom CPU demand via its default).
- Memory governed by `requests.memory: 8Gi` + `limits.memory: 24Gi` (3× requests, sized
  for the overcommit the LimitRange defaults inevitably produce).
- LimitRange: `defaultMemory: 512Mi`, `defaultRequestCpu: 50m`,
  `defaultRequestMemory: 128Mi`, `maxMemory: 4Gi`. No `max.cpu` — would silently
  re-create the invented CPU limit.
- Quota caps: 50 pods, 10 PVCs.
- NetworkPolicy is **off by default** (`networkPolicy.enabled: false`) — when on it
  permits egress only to kube-system, the agent's own namespace, and DNS, which blocks
  OTLP telemetry to the collector.

## charts/chaos

The fault injection kit — the only chart that targets the agent's vCluster rather than
the host cluster.

- Installed into `chaos-system` namespace inside the vCluster by `just chaos-install`
  and `scripts/chaos-run.py`.
- Components: fault-webhook (`oven/bun:1`), toxiproxy
  (`ghcr.io/shopify/toxiproxy:2.12.0`), RBAC (ServiceAccount + ClusterRole).
- Cost: 2 pods, ~192Mi.
- Registered in `just render` and `just lint`.

## charts/observability

The shared LGTM observability stack, deployed to the `observability` namespace.

- Applied via `just observability`.
- Contains: Mimir (monolithic, `grafana/mimir:3.1.4`, filesystem storage 10Gi),
  OTel Collector (`otel/opentelemetry-collector-contrib:0.157.0`), Grafana datasources
  (with stable UIDs: `mimir`, `tempo`, `loki`), and the Grafana Ingress
  (`grafana.platform.localhost`).
- OTel Collector published on host NodePorts: gRPC on 30417, HTTP on 30418.
- Values at `charts/observability/values.yaml`.
- Mimir's `promote_otel_resource_attributes` is configured as a comma-separated string
  (not a YAML list — Mimir parses it as `flagext.StringSliceCSV`).

## charts/platform

Routes the out-of-cluster Forgejo container through host Traefik.

- Applied during `just forgejo`.
- Deployed to the `platform` namespace.
- Forgejo container IP is the one value only known at runtime — re-run `just forgejo`
  if it changes.
- Uses `--set forgejo.ip=<ip>` at install time.

## Inspecting and validating

```bash
just render                  # every chart to stdout
just render agent            # one chart
just lint                    # lint all 4 charts via `helm lint`
```
