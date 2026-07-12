# Improvements & Known Issues

Documented during end-to-end testing of the justfile recipes and platform setup.

---

## Architecture Decisions

### Cilium dropped as default dataplane — `just bootstrap` is the primary path

Cilium was evaluated as the single dataplane (CNI + kube-proxy + Gateway API +
Hubble + NetworkPolicy per PLATFORM-SETUP.md §4.2–4.3). After extensive testing,
it was downgraded to an optional/experimental recipe. Reasoning:

**What Cilium provides vs. what the platform actually needs:**

| Cilium feature | Value for this project | Better alternative |
|---|---|---|
| eBPF kube-proxy replacement | No perf difference for 2–5 dev agents | Default kube-proxy (iptables) |
| Gateway API (Cilium) | Redundant — k3d routes host ports through Traefik's loadbalancer | Traefik (k3d default) handles ingress |
| Hubble L3–L7 flows | Nice but OTel + LGTM already gives per-agent traces/metrics/logs | Grafana + Tempo + Loki + Mimir |
| CiliumNetworkPolicy L7 rules | `just netpol` only uses L3/L4 (namespace isolation) | Kubernetes NetworkPolicy |
| Cilium CNI | Basic pod networking | Flannel (k3d default, zero config) |

**What Cilium costs:**

| Cost | Detail |
|---|---|
| TLSRoute CRD patch | Gateway API v1.5.1+ sets `served:false` on v1alpha2; Cilium 1.19 hardcodes v1alpha2. Requires `kubectl patch crd`. Breaks on version bumps. |
| Manual Traefik install | k3d auto-deploy jobs conflict with pre-installed Gateway CRDs. Must delete jobs + helm install manually. |
| Two bootstrap paths | Diverging recipes to maintain, different bugs, different troubleshooting |
| Recipe complexity | `bootstrap-cilium` is ~40 lines of bash; `bootstrap` is 1 line |

**Verdict:** Dropped Cilium. Kept `bootstrap-cilium` as an optional/experimental
recipe for users who want it. `just bootstrap` (flannel + Traefik) is the
recommended default.

### Kubernetes NetworkPolicy replaces CiliumNetworkPolicy for agent isolation

Since Cilium is no longer the default, agent isolation uses native
`NetworkPolicy` (available with both flannel and any CNI). Added a
`network-policy` recipe to the justfile. The existing `just netpol` recipe
remains (Cilium-only).

---

## Remaining Recipe Issues

### `just observability` — k8s-monitoring
The `grafana/k8s-monitoring` (Alloy) chart fails without `cluster.name` and at
least one enabled collector + destination. The recipe expects a
`k8s-monitoring-values.yaml` file that doesn't exist. Added `--set cluster.name`
so the chart installs, but full collection needs a proper values file.

### `just provision` — timeout risk
`vcluster connect --print` occasionally hangs for >2 minutes. Works reliably
on the default flannel cluster; intermittent on Cilium.

---

## Chart Version Drift

### Loki chart v7.0.0
The default `deploymentMode: SimpleScalable` requires object storage. Recipe must
now explicitly set:
- `singleBinary.replicas=1`
- `backend.replicas=0`, `read.replicas=0`, `write.replicas=0`
- `loki.useTestSchema=true` (no schema config provided)

### Tempo chart — deprecated
The `grafana/tempo` chart prints a deprecation warning on every install. Monitor
for replacement chart.

### Grafana chart — deprecated
Same as Tempo. The `grafana/grafana` chart is also deprecated. Both still work
but will stop receiving updates.

---

## Design / Architecture

### `/srv/` paths require root
`kubeconfigs` and `opencodes` were hardcoded to `/srv/agent-kubeconfigs` and
`/srv/agent-opencode`. These require root to create. Changed to
`$HOME/.local/share/` for user-writable paths, but this diverges from the
PLATFORM-SETUP.md design (which assumes a harness service running as root).

### `host.docker.internal` only works inside containers
Static kubeconfigs use `host.docker.internal` as the server address — this only
resolves inside Docker containers (or with `--add-host`). On the host machine,
MCP servers and local tooling need `127.0.0.1` variants of the kubeconfig.

### Mimir-distributed is too heavy for dev
The full `grafana/mimir-distributed` chart deploys 25+ pods (ingester zones,
store-gateway zones, Kafka, MinIO, etc.). For a dev platform, consider
`mimir/single-binary` mode or plain Prometheus. The 48GB Docker cap (from
`just harden`) is too tight with Mimir + Astroshop running simultaneously.

### No DevContainer built
Agents currently run on the bare-metal host. The DevContainer (PLATFORM-SETUP.md
§8) with `workspaceMount` scoping and no Docker socket hasn't been built or
tested.

### Grafana LBAC not configured
The Grafana service account token is a basic `Viewer` role — no label-based
access control (LBAC) scoping per agent namespace. Any agent with the token
can query all telemetry.

### k8sgpt needs separate AI backend
`k8sgpt serve --mcp` requires its own AI provider configuration (OpenAI key,
Ollama, etc.) — it doesn't piggyback on OpenCode's model. The Kubernetes MCP
server (`kubectl-mcp-server`) is a better fit since OpenCode provides the
reasoning.

### Agent isolation — NetworkPolicy vs CiliumNetworkPolicy
`CiliumNetworkPolicy` was tested end-to-end on the Cilium cluster (VALID=True).
With the migration off Cilium, agent isolation uses native `NetworkPolicy`. The
`just netpol` recipe is Cilium-only; a `network-policy` recipe needs to be added
for the default bootstrap path.

### `just harden` kills the cluster
`systemctl restart docker` destroys the running cluster. k3d containers
auto-restart and workloads mostly recover (~2 minutes), but this isn't a
graceful operation. Consider a separate Docker daemon or VM-level cgroups
for the cap.

---

## Quick Fixes Applied (already in justfile)

| Issue | Fix |
|---|---|
| `/srv/` permission denied | Changed to `$HOME/.local/share/` |
| Loki chart v7.0.0 strict validation | Added singleBinary.replicas, zeroed scalable replicas, useTestSchema |
| k8s-monitoring requires cluster.name | Added `--set cluster.name=agent-platform` |

---

## End-to-End Test Results (2026-07-11)

Full system walkthrough: bootstrap → observability → provision 2 agents → deployment → ingress → deprovision.

### What works

| Capability | Status | Notes |
|---|---|---|
| `just bootstrap` | Pass | 3/3 Ready in ~30s, Traefik auto-installs |
| `just observability` | Pass* | LGTM deployed; k8s-monitoring fails (known); Grafana ingress must be applied manually if recipe fails mid-way |
| OTel Collector deployment | Pass | All 3 pipelines (traces/metrics/logs) configured |
| Grafana datasources (Tempo/Loki/Mimir) | Pass | Configured via API |
| `just provision` x2 agents | Pass | NodePorts 30001/30002 assigned |
| `just network-policy` | Pass | NetworkPolicy applied to both agent namespaces |
| Ingress sync through vCluster | Pass | `Ingress` in vCluster → surfaced in host Traefik |
| App accessible via hostname | Pass | `test.agent-alpha.localhost` → HTTP 200 |
| `just deprovision` | Pass | vCluster + namespace + kubeconfig cleanly removed |
| `just prune` | Pass | Cleanup works |
| `just list` | Pass | Shows all active agents |
| vCluster connect | Pass | Proxy-based access works reliably |
| MCP servers | Pass | mcp-grafana v0.17.0, kubectl-mcp-server v1.24.0 ready |

### Faults & Inefficiencies Found

1. **`just observability` is brittle** — k8s-monitoring failure (no collectors config) exits the bash script with `set -e`, so the Grafana ingress at the end of the recipe never gets created. The LGTM backends deploy fine but ingress is missing. Fix: move ingress apply before k8s-monitoring, or make k8s-monitoring conditional.

2. **Static kubeconfig only works in containers** — uses `host.docker.internal` as server address. On the host machine, `vcluster connect` is required. This is by design but means host-side MCP servers can't use the static kubeconfig — they need a `127.0.0.1` variant or `vcluster connect`.

3. **127.0.0.1 kubeconfig fails with EOF** — even though port 30001 listens and `127.0.0.1` is in the vCluster cert SANs, the kubeconfig with `https://127.0.0.1:30001` gets EOF/timeout from kubectl. Root cause unclear — possibly k3s API server rejecting SNI or network policy interaction.

4. **ResourceQuota too tight for demo workloads** — default `limits.cpu: 6` blocks the Astroshop demo (22 pods × 500m = 11 cores). The quota is correctly protective but the default is restrictive. Agents would need to request a larger quota for real workloads.

5. **No `k8s-monitoring-values.yaml`** — the observability recipe is incomplete without it. The OTel Collector is a manual workaround that works but isn't in the recipe. Either add the collector deployment to `just observability` or create a proper Alloy values file.

6. **`just harden` untested in this run** — the Docker resource cap was applied in a previous run but `systemctl restart docker` didn't fully destroy the cluster (k3d containers auto-restarted). This is a risk: the cap may not survive a Docker daemon upgrade or host reboot if the drop-in isn't persistent.

7. **No observability for vCluster workloads** — the OTel Collector is in the host cluster's `observability` namespace. Agents deploying inside a vCluster need to configure their apps to send OTLP to `otel-collector.observability.svc.cluster.local` (host cluster DNS). This works but is fragile — it requires hardcoding the host cluster namespace. A better approach: the collector could be deployed inside each vCluster via the provision recipe, or a dedicated collector per vCluster namespace.

8. **vcluster CLI version warning** — v0.35.1 installed, v0.35.2 available. Minor but repeated warnings clutter output.

9. **Grafana + Tempo charts deprecated** — both print deprecation warnings on every install. Monitor for replacement charts.

10. **k8s-monitoring chart blocks observability recipe** — because `set -euo pipefail` exits the whole recipe when k8s-monitoring fails, the post-install Grafana ingress never runs. The recipe works for LGTM deployment but leaves the system in an incomplete state.
