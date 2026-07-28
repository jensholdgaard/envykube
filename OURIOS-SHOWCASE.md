# Ourios showcase — tenant-isolated agent logs + an MCP query surface

This branch adds [Ourios](https://github.com/jensholdgaard/ourios) — a
single-binary log store (OTLP → Parquet → DataFusion, Apache-2.0) — to the
shared observability stack, **alongside** Loki. Nothing existing changes:
the OTel Collector dual-writes the logs pipeline, Grafana/Loki/Tempo/Mimir
behave exactly as before. Delete the branch and the platform is untouched.

## Why this platform specifically

**1. The telemetry plane currently contradicts the isolation model.**
envykube's core constraint is *no cross-agent visibility* — NetworkPolicy,
per-agent vClusters, ResourceQuota. But the observability stack is
explicitly untenanted (Mimir requires `multitenancy_enabled: false`, Loki
is shared single-binary), and every agent's DevContainer holds a Grafana
service-account token that can query **all** telemetry. Agent isolation
stops at the workload plane.

Ourios derives a tenant from each `ResourceLogs`' `service.name` at ingest
and scopes every query to a tenant. Each agent's logs land in their own
partition with zero collector configuration. In open mode (this branch,
matching the stack's posture) that's organisational isolation; adding
per-agent static tokens (commented in `templates/ourios.yaml`) makes it
adversarial — an agent's credential physically cannot read another
agent's logs (401/403 enforced server-side).

**2. Agents already query telemetry over MCP — this makes that tenant-bound.**
Each DevContainer runs `mcp-grafana`. Ourios's querier exposes its own MCP
surface (`/mcp`, streamable HTTP: `query_logs`, `list_templates`,
`template_drift`, plus a `ourios://query-schema` resource that tells the
agent exactly which fields, severity bands, and promoted attributes it can
query — designed for a coding agent as the audience). Add to the per-agent
`opencode.json`:

```jsonc
{
  "mcp": {
    "ourios": {
      "type": "remote",
      "url": "http://host.docker.internal:30419/mcp",
      "headers": { "x-ourios-tenant": "<agent service.name>" }
    }
  }
}
```

The `k8s-debugger` agent can then diagnose its vCluster from its own logs
— and only its own.

**3. FinOps on the logs you already emit.**
`opencode-plugin-otel`'s `api_request` log event carries flat `cost_usd`,
`input_tokens`, `output_tokens`, `model` attributes (see its
`src/handlers/message.ts`). Ourios promotes chosen attributes to real
Parquet columns at ingest — with the 0.6.0 image the spend/token columns
are typed (`Float64`/`Int64`), so aggregation over money works directly.
A query without a `range(...)` stage covers a default recent window; add
one to scope in time. Grouping by an attribute implicitly filters to the
records that carry it, so `by attr.model` is the api_request family:

```
template_id > 0 | count by attr.model                       # calls per model, this agent
template_id > 0 | count by attr.decision                    # permission accept/deny mix
template_id > 0 | sum(attr.cost_usd) by attr.model, bucket(1h)   # spend over time
template_id > 0 | sum(attr.output_tokens) by attr.agent          # usage per subagent
```

(Grouping by an attribute that is not promoted fails loudly with an
error naming the fix — no silent empty results. An all-NULL group sums
to `null`, never zero.)

That turns the pool into a meterable fleet: per-agent spend is one query,
and an agent can introspect its own cost over MCP (this loop runs daily
against Ourios's own dogfood capture).

## Try it

```sh
just observability          # same command; the chart now includes Ourios
kubectl -n observability get pods -l app=ourios

# after some agent traffic, query a tenant (tenant = the agent's service.name):
curl -s -X POST http://host.docker.internal:30419/v1/query \
  -H 'content-type: application/json' -H 'x-ourios-tenant: <service.name>' \
  -d '{"query":"template_id > 0 | count by attr.model"}'
```

## Honest scope

- **Logs only.** Tempo and Mimir are untouched; this replaces nothing —
  it dual-writes next to Loki so you can compare on your own traffic.
- **No Grafana datasource yet.** Ourios's dashboard story is a Perses
  plugin set (shipped); the Grafana datasource is a roadmapped follow-up
  — envykube is a good reason to accelerate it. Meanwhile Grafana keeps
  reading logs from Loki, and agents/humans query Ourios directly or
  over MCP.
- **Pre-release** (v0.5.x), but the write path is WAL-before-ack with a
  SIGKILL crash-recovery test in CI — it should hold up fine under
  `chaos/scenarios/restart-cold.yaml` and friends.
