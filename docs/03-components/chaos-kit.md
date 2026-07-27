# Chaos Kit

The zero-privilege fault injection kit deployed inside agent vClusters (`charts/chaos/`).
It is the only chart in this repo that targets the vCluster rather than the host cluster.

## Components

### fault-webhook

A `ValidatingAdmissionWebhook` that rejects a deterministic ordinal subset of writes.
Runs on `oven/bun:1`, handler mounted from a ConfigMap (no container build required —
agents have no Docker socket and the platform has no in-cluster registry).

- **Control API** on port 8080 — plain HTTP, reachable through the API server's service proxy.
- **Admission** on port 8443 — TLS (Kubernetes requires it).
- **Modes:** `conflict` (409), `error` (500), `throttle` (429).
- **Ordinals:** "the 1st write", "the 3rd write", etc. The counter lives in process memory,
  so `replicas: 1` is load-bearing — a second replica splits the count and the ordinal
  guarantee breaks.
- **Operations:** `CREATE`, `UPDATE`, `DELETE`, `CONNECT`. No `PATCH` in the webhook rules —
  the admission layer only surfaces those four; both `kubectl patch` and `kubectl apply`
  arrive as `UPDATE`.
- Scoped by `namespaceSelector`: `kube-system`, `kube-public`, `kube-node-lease`, and
  `chaos-system` are excluded so arming a fault can never wedge the platform itself.

### toxiproxy

`ghcr.io/shopify/toxiproxy:2.12.0` — the only zero-privilege way to inject latency.
`tc`/`netem` needs `NET_ADMIN` in the target's network namespace; a userspace TCP proxy
does not.

- API on port 8474.
- Pre-opened proxy listener ports: 21000, 21001, 21002.
- Used for the `dependency-slow` scenario (extended suite only, since the app must address
  its dependency through toxiproxy — declared via `dependency.proxy: true` in `.chaos.yaml`).

### RBAC

`chaos-runner` ServiceAccount + ClusterRole for API-only faults (kill, scale, stall,
squeeze, partition). Cluster-scoped because the chart cannot know at install time which
namespace the agent deploys into. This is still nothing the agent doesn't already have —
it is cluster-admin of its own vCluster.

## Resource Cost

2 pods, ~192Mi against the agent's 8Gi/50-pod quota. Sized deliberately small so the
kit is never the reason a chart doesn't fit.

## Key Design Decisions

### `failurePolicy: Ignore`, not `Fail`

A webhook that fails closed and then crashes would reject every write in the agent's
cluster — including the writes that would fix it. That is an unrecoverable state in a
sandbox whose whole purpose is producing _recoverable_ ones. The cost is that a dead
webhook silently injects nothing, which is why every fault-injecting scenario ends with
`assert-fired`.

### CA reuse via `lookup`

The TLS CA is generated once and reused through `lookup` in the Secret. An upgrade
that rotates the Secret without rolling the pod would leave the API server trusting a
CA the process is not serving — `x509: certificate signed by unknown authority`,
swallowed silently by `failurePolicy: Ignore`. Observed for real.

The Deployment carries `checksum/tls` and `checksum/handler` annotations so a genuine
rotation or handler change rolls the pod.

### Control APIs through service proxy

All control APIs go through the API server's service proxy:

```
/api/v1/namespaces/chaos-system/services/http:fault-webhook:8080/proxy/...
```

Not port-forward, not Ingress. Both of those are forbidden to agents by `AGENTS.md`,
and a tool the agent is not allowed to imitate is a tool that teaches it the wrong
pattern.

### Pinned version & upstream images

Both `oven/bun:1` and `ghcr.io/shopify/toxiproxy:2.12.0` are upstream images. Nothing
needs a container build — there is no Dockerfile in the kit's directory structure.
