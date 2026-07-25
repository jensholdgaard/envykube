# Agentic Kubernetes Platform — Host Setup & Operations

This document describes everything required to stand up the platform on a host
machine and how agents (OpenCode) use it day-to-day via the justfile and skill file.

The platform gives each agent an **isolated, ephemeral Kubernetes cluster** in
which it can freely design, implement, integrate, and validate microservices —
deploy Helm charts, install operators/CRDs, wire up services, hit them over HTTP,
and **observe them through a shared LGTM stack** — without any risk of breaking the
host machine.

---

## 1. Architecture

```
┌──────────────────────────────────────────────────────────────────────────┐
│ HOST MACHINE                                                               │
│  Docker Engine (capped by `just harden` — MemoryMax=48G, CPUQuota=1400%)  │
│      │                                                                     │
│      ▼                                                                     │
│  ┌────────────────────────────────────────────────────────────────────┐  │
│  │ k3d host cluster  "agent-platform"  (k3s in Docker)                  │  │
│  │  • shared ingress (Traefik) ── host :80 / :443                       │  │
│  │  • NodePort range 30000-30050 ── host, for per-vCluster API access   │  │
│  │  • observability: LGTM + OTel Collector (collects ALL agent workloads)│  │
│  │                                                                      │  │
│  │  ┌──────────────────────┐   ┌──────────────────────┐                │  │
│  │  │ vCluster agent-alpha  │   │ vCluster agent-beta   │   … per agent  │  │
│  │  │  ns: vc-agent-alpha   │   │  ns: vc-agent-beta    │                │  │
│  │  │  own API/CRDs/RBAC    │   │  own API/CRDs/RBAC    │                │  │
│  │  │  + ResourceQuota      │   │  + ResourceQuota      │                │  │
│  │  │  + NetworkPolicy      │   │  + NetworkPolicy      │                │  │
│  │  └──────────▲───────────┘   └──────────▲───────────┘                │  │
│  └─────────────┼──────────────────────────┼────────────────────────────┘  │
│                │ static kubeconfig         │ static kubeconfig             │
│  ┌─────────────┴──────────┐   ┌────────────┴─────────┐                    │
│  │ DevContainer (agent A)  │   │ DevContainer (agent B)│                   │
│  │  OpenCode + kubectl/helm│   │  OpenCode + kubectl   │                   │
│  │  + MCP: Grafana, K8s    │   │  + MCP: Grafana, K8s  │                   │
│  │  workspace = git clone  │   │  workspace = git clone│                   │
│  │  from local Forgejo     │   │  from local Forgejo   │                   │
│  │  NO docker socket       │   │  NO docker socket     │                   │
│  └─────────────────────────┘   └──────────────────────┘                   │
└──────────────────────────────────────────────────────────────────────────┘
```

| Decision | Rationale |
|---|---|
| **k3d** as the shared host cluster | GA/stable "k8s in Docker"; textbook host for vClusters. |
| **Standard (helm-driver) vClusters** per agent | Cheap, namespace-scoped, own API server + CRDs + RBAC. Teardown = delete namespace. |
| **One shared ingress** in the host cluster | All agent clusters route on a single `:80`/`:443` by hostname. vClusters *sync* their Ingresses up to it. |
| **Static per-agent kubeconfig** via NodePort | No live proxy process to babysit (see §7). |
| **Agents get a vCluster kubeconfig + scoped MCP tokens only** | The agent's ceiling is its own cluster + a ResourceQuota + read-only observability of its own namespace. Never Docker, never the host. |
| **One LGTM + OTel Collector in the host cluster** | Because vCluster workloads run as real pods in `vc-<cluster>` namespaces, one collector observes every agent, labelled per namespace (see §6). |
| **Prometheus by default** (Mimir distributed opt-in) | Single-binary metrics for dev scale; `just observability mimir=distributed` for learning the full LGTM stack. |
| **Forgejo** as local Git server | Agents clone from a private, local Git server. Push changes for CI/review. No public repo auth needed. |
| **Traefik ingress** (default) | Ships with k3d, zero-config. Cilium dataplane is available as an experimental option. |

**Isolation honesty:** vClusters share the host cluster's kernel and nodes — *soft*
isolation, appropriate for **your own** trusted agents. Not a hard sandbox for
untrusted third-party code (for that, add gVisor/Kata or a dedicated cluster).

---

## 2. Host prerequisites

| Requirement | Notes |
|---|---|
| Linux, **cgroup v2** + systemd | Verify: `stat -fc %T /sys/fs/cgroup` → `cgroup2fs`. |
| **Docker Engine** (rootful) | Binds `:80/:443` with no sysctl changes; blast radius bounded to the Docker daemon. |
| CPU / RAM / disk | Per agent: ~1 vCPU + 1–2 GB control plane **plus** workload usage. Add ~4 vCPU + 8 GB for the LGTM stack. Starter box for ~5 agents + observability: **12+ vCPU, 48+ GB RAM, 150+ GB SSD**. |
| Sized Docker data-root | Put `/var/lib/docker` on a dedicated filesystem. |

---

## 3. Install the toolchain (host, one-time)

Linux amd64 shown.

```bash
# Docker Engine
curl -fsSL https://get.docker.com | sh
sudo usermod -aG docker "$USER"        # log out/in afterwards

# kubectl
curl -LO "https://dl.k8s.io/release/$(curl -Ls https://dl.k8s.io/release/stable.txt)/bin/linux/amd64/kubectl"
sudo install -m 0755 kubectl /usr/local/bin/kubectl && rm kubectl

# Helm
curl -fsSL https://raw.githubusercontent.com/helm/helm/main/scripts/get-helm-3 | bash

# k3d
curl -s https://raw.githubusercontent.com/k3d-io/k3d/main/install.sh | bash

# vCluster CLI
curl -Lo vcluster "https://github.com/loft-sh/vcluster/releases/latest/download/vcluster-linux-amd64" \
  && sudo install -c -m 0755 vcluster /usr/local/bin && rm -f vcluster

# DevContainer CLI (needs Node.js)
npm install -g @devcontainers/cli

# kubectl-mcp-server (Kubernetes MCP)
npm install -g kubectl-mcp-server

# mcp-grafana (Grafana MCP)
curl -LsSf https://astral.sh/uv/install.sh | sh
uv tool install mcp-grafana

# OpenCode — install per your OpenCode distribution

# Cilium CLI (optional; only needed for `just hubble` and Cilium cluster debugging)
CILIUM_CLI_VERSION=$(curl -s https://raw.githubusercontent.com/cilium/cilium-cli/main/stable.txt)
curl -Lo cilium.tar.gz "https://github.com/cilium/cilium-cli/releases/download/${CILIUM_CLI_VERSION}/cilium-linux-amd64.tar.gz"
sudo tar xzf cilium.tar.gz -C /usr/local/bin && rm cilium.tar.gz

docker --version && kubectl version --client && helm version \
  && k3d --version && vcluster --version && devcontainer --version
```

Verify with `just versions`.

---

## 4. Create the shared host cluster (one-time, durable)

The justfile provides `just bootstrap` (recommended default: flannel CNI + Traefik)
and `just bootstrap-cilium` (experimental: Cilium dataplane). The default is:

```bash
just bootstrap
```

This runs:

```bash
k3d cluster create agent-platform \
  --servers 1 --agents 2 \
  --servers-memory 4g --agents-memory 12g \
  --port "80:80@loadbalancer" \
  --port "443:443@loadbalancer" \
  --port "30000-30050:30000-30050@server:0"
```

- `:80`/`:443@loadbalancer` → the shared ingress entry point.
- `30000-30050@server:0` pre-publishes a **NodePort range** on the host so each
  vCluster's API server can be reached on a stable port (see §7). 51 ports ≈ 51
  concurrent agents; widen as needed.
- Traefik ships enabled and is kept as the shared controller.
- The Traefik dashboard is available at `http://traefik.localhost`.

Your kube-context is now `k3d-agent-platform`. **Keep it and the Docker socket on
the harness/host only** — never hand either to an agent.

### 4.1 Guarantee the host survives anything (hard ceiling)

k3d runs its node containers privileged, so its memory *flags* don't impose a real
kernel limit. Cap the **Docker daemon itself** with a systemd slice:

```bash
just harden
```

Or manually:

```bash
sudo mkdir -p /etc/systemd/system/docker.service.d
sudo tee /etc/systemd/system/docker.service.d/resource-cap.conf >/dev/null <<'EOF'
[Service]
MemoryMax=48G
MemorySwapMax=48G
CPUQuota=1400%      # = 14 cores
EOF
sudo systemctl daemon-reload && sudo systemctl restart docker
# recreate the k3d cluster after restarting docker
```

For the strongest boundary, run this entire Docker host inside a resource-limited VM.

### 4.2 Ingress data plane: Traefik (default)

Traefik is the recommended default — ships with k3d, zero config, provides
`IngressRouteTCP` for vCluster API SNI passthrough (§7 Option B).

**Cilium dataplane** is available as an experimental option (`just bootstrap-cilium`).
It provides eBPF kube-proxy replacement, Gateway API, Hubble, and CiliumNetworkPolicy.
See `improvements.md` for the full evaluation of why it was downgraded from the
default path.

---

## 5. Wildcard DNS for `*.localhost`

Agents reach apps at `app.<cluster>.localhost` and shared services at
`grafana.platform.localhost`. These must resolve to `127.0.0.1`.

```bash
getent hosts app.cluster-alpha.localhost      # modern systemd-resolved already handles it
# If empty, add a wildcard (NetworkManager + dnsmasq shown):
echo 'address=/localhost/127.0.0.1' | \
  sudo tee /etc/NetworkManager/dnsmasq.d/localhost-wildcard.conf
sudo systemctl restart NetworkManager
```

Forgejo is also accessible at `http://forgejo.platform.localhost`.

---

## 6. Observability & agent introspection (host cluster, one-time)

This is what gives agents a real **grasp of their services**: metrics, logs, and
traces of everything they deploy, queryable via MCP.

### 6.1 Why one shared stack works

vCluster syncs each agent's workloads *down* into the host namespace `vc-<cluster>`
as real pods. So a single collector in the host cluster sees **every** agent's
workloads automatically, and everything is labelled by `namespace` — which is the
per-agent boundary.

### 6.2 Deploy LGTM + OTel Collector

```bash
just observability
```

This deploys:
- **Loki** (SingleBinary) — logs backend
- **Tempo** — traces backend
- **Prometheus** (default) — metrics backend (~1 pod, ~1 GB)
- **Grafana** — dashboards, exposed at `http://grafana.platform.localhost`
- **Grafana ingress** — auto-provisioned at `grafana.platform.localhost`

For the full Mimir distributed stack (production-grade, 25+ pods):

```bash
just observability mimir=distributed
```

The recipe also supports `k8s-monitoring` (Alloy) if a
`k8s-monitoring-values.yaml` file exists. Without it, deploy an OTel Collector
manually (see below) or use the sample configuration in the `justfile`.

#### OTel Collector (manual deployment)

If `k8s-monitoring-values.yaml` doesn't exist, deploy a collector to ingest
OTLP from agent workloads and forward to the LGTM backends:

```yaml
# ConfigMap with pipelines for traces → Tempo, metrics → Prometheus, logs → Loki
receivers:
  otlp:
    protocols:
      grpc: { endpoint: 0.0.0.0:4317 }
      http: { endpoint: 0.0.0.0:4318 }
exporters:
  otlp/tempo:    { endpoint: tempo.observability.svc.cluster.local:4317, tls: { insecure: true } }
  otlphttp/mimir: { endpoint: http://prometheus-server.observability.svc:80/api/v1/otlp, tls: { insecure: true } }
  otlphttp/loki:  { endpoint: http://loki.observability.svc.cluster.local:3100/otlp, tls: { insecure: true } }
```

Agent workloads send OTLP to `otel-collector.observability.svc.cluster.local:4317`.

### 6.3 Grafana datasources

After deploying the LGTM stack, configure Grafana datasources:

```bash
# Prometheus (metrics)
curl -u admin:<password> -X POST http://grafana.platform.localhost/api/datasources \
  -H "Content-Type: application/json" \
  -d '{"name":"Mimir","type":"prometheus","url":"http://prometheus-server.observability.svc:80","access":"proxy","isDefault":true}'

# Tempo (traces)
curl -u admin:<password> -X POST http://grafana.platform.localhost/api/datasources \
  -H "Content-Type: application/json" \
  -d '{"name":"Tempo","type":"tempo","url":"http://tempo.observability.svc.cluster.local:3200","access":"proxy"}'

# Loki (logs)
curl -u admin:<password> -X POST http://grafana.platform.localhost/api/datasources \
  -H "Content-Type: application/json" \
  -d '{"name":"Loki","type":"loki","url":"http://loki-gateway.observability.svc.cluster.local","access":"proxy"}'
```

### 6.4 MCP servers wired into each agent

These give the OpenCode agent structured, tool-shaped access to its cluster and
telemetry — far more reliable than parsing CLI text.

| MCP server | Source | Gives the agent |
|---|---|---|
| **Grafana MCP** | `mcp-grafana` (Python, via `uv`) | PromQL/LogQL queries, Tempo trace lookups, error/slow-request detection, dashboard search + PNG render, alert status |
| **Kubernetes MCP** | `kubectl-mcp-server` (npm) | Structured kubectl operations (270+ tools), resources, events, logs |

Both are baked into the agent DevContainer image. The per-agent `opencode.json`
is generated by `just workspace`. A project-level config template is at `opencode.jsonc`.

The agent also has direct access to `kubectl`, `helm`, and `vcluster` CLI.

---

## 7. Provision a per-agent cluster with a durable, static kubeconfig

**Option A (NodePort) is the default** — it's ingress-agnostic and fully deterministic.

```bash
just provision <agent-name> <index>
```

This:
1. Creates a vCluster in namespace `vc-<agent-name>`
2. Applies ResourceQuota + LimitRange
3. Pins the vCluster API service to a NodePort (`np_base + index`)
4. Exports a static kubeconfig to `$HOME/.local/share/agent-kubeconfigs/<name>.yaml`

The kubeconfig uses `host.docker.internal` as the server address — it works
from inside the DevContainer (via `--add-host`). On the host, use
`vcluster connect <name> -n vc-<name>` instead.

### 7.1 vCluster config template

Generated automatically by `just provision`:

```yaml
sync:
  toHost:
    ingresses:
      enabled: true                 # agent Ingresses surface in the shared controller
controlPlane:
  statefulSet:
    resources:
      requests: { cpu: 200m, memory: 256Mi }
      limits:   { cpu: "2",  memory: 2Gi }
  proxy:
    extraSANs:
      - host.docker.internal        # from the DevContainer
      - 127.0.0.1                   # from the host
```

### 7.2 Bound the agent's footprint

```yaml
# ResourceQuota (applied automatically by just provision)
spec:
  hard:
    requests.cpu: "4"
    requests.memory: 8Gi
    limits.cpu: "6"
    limits.memory: 12Gi
    pods: "50"
    persistentvolumeclaims: "10"
---
# LimitRange
spec:
  limits:
    - type: Container
      default:        { cpu: 500m, memory: 512Mi }
      defaultRequest: { cpu: 100m, memory: 128Mi }
```

### 7.3 Agent network isolation

Apply per-agent network isolation:

```bash
just network-policy <agent-name>
```

This applies a Kubernetes `NetworkPolicy` that restricts pods to their own
namespace + kube-system (for DNS). Works with any CNI. For Cilium-specific
L7 policies, use `just netpol <agent-name>` (experimental, requires Cilium).

### 7.4 Option B — SNI passthrough (no port management)

Multiplex every agent's API on `:443`, routed by SNI. No per-agent port allocation.

```yaml
apiVersion: traefik.io/v1alpha1
kind: IngressRouteTCP
metadata: { name: cluster-alpha-api, namespace: vc-cluster-alpha }
spec:
  entryPoints: [websecure]
  routes:
    - match: HostSNI(`api.cluster-alpha.localhost`)
      services:
        - name: cluster-alpha
          port: 443
  tls:
    passthrough: true
```

---

## 8. The agent DevContainer & workspace

Each agent runs in a DevContainer with OpenCode, CLIs, MCP servers, its
**static** kubeconfig, and a git workspace cloned from the local Forgejo instance.
**No Docker socket.**

### 8.1 Start Forgejo (local Git server)

```bash
just forgejo
```

Forgejo runs at `http://forgejo.platform.localhost`. On first visit, complete the
install form (SQLite, admin: agent / agent123). Agents clone from it, push
changes for CI/review. No external auth needed.

### 8.2 Agent Docker image

Baked into `.devcontainer/Dockerfile` (Ubuntu 24.04):

| Tool | Source |
|---|---|
| kubectl | `dl.k8s.io` |
| helm | `get-helm-3` script |
| vcluster CLI | GitHub releases |
| kubectl-mcp-server | npm |
| mcp-grafana | `uv tool install` |
| opencode | GitHub releases |
| git, curl, jq | apt |

### 8.3 devcontainer.json

Located at `.devcontainer/devcontainer.json`:

```jsonc
{
  "name": "opencode-agent",
  "build": { "dockerfile": "Dockerfile" },
  "workspaceMount": "source=${localWorkspaceFolder},target=/workspace,type=bind",
  "workspaceFolder": "/workspace",
  "runArgs": [
    "--network=k3d-agent-platform",
    "--hostname=agent",
    "--add-host=host.docker.internal:host-gateway",
    "--add-host=grafana.platform.localhost:host-gateway",
    "--add-host=forgejo.platform.localhost:host-gateway"
  ],
  "mounts": [
    "source=${localEnv:KUBECONFIG_PATH},target=/home/agent/.kube/config,type=bind,readonly",
    "source=${localEnv:OPENCODE_CONFIG_PATH},target=/home/agent/.config/opencode/opencode.json,type=bind,readonly"
  ],
  "remoteUser": "agent"
  // Deliberately NO "/var/run/docker.sock" mount.
}
```

### 8.4 Launch an agent workspace

```bash
just provision agent-alpha 1
just workspace agent-alpha http://forgejo.platform.localhost/agent/my-repo.git
```

This:
1. Clones the repo from Forgejo into `$HOME/.local/share/agent-workspaces/agent-alpha/`
2. Copies `.devcontainer/` config into the workspace
3. Generates a per-agent `opencode.json` with MCP servers pointing at the agent's vCluster
4. Builds the DevContainer image (cached after first build)
5. Launches the container with workspace mount, kubeconfig, and network access

### 8.5 Git note

Agents get a **full `git clone`**, not a bare worktree — a worktree's `.git`
only points into the main repo, so a lone bind-mount breaks git operations.

---

## 9. Agent workflow (day in the life)

1. **Provisioned** by harness: `just provision <name> <index>`
2. **Workspace launched**: `just workspace <name> <repo-url>`
3. **Build** the microservice in `/workspace`.
4. **Deploy** into its own vCluster: `helm install my-svc ./chart` (or `kubectl apply`).
5. **Expose** with a task-scoped hostname (`app.<name>.localhost`) via an
   Ingress — vCluster syncs it to the shared controller automatically.
6. **Validate the path:** `curl http://app.<name>.localhost/healthz`.
7. **Introspect via MCP:**
   - *"What's the p99 latency and error rate of `my-svc` over the last 15m?"* →
     **Grafana MCP** runs PromQL against Prometheus.
   - *"Show the error logs for `my-svc`."* → **Grafana MCP** runs LogQL against Loki.
   - *"Where is the slow span in this request?"* → **Grafana MCP** pulls the trace
     from Tempo.
   - *"Why is my pod restarting?"* → **Kubernetes MCP** reads events, pod status,
     and logs.
8. **Push changes** to Forgejo: `git push origin main`.
9. **Iterate** — full admin within its own vCluster, zero effect on anything else.
10. **Teardown** (harness) — §10.

---

## 10. Teardown & maintenance

```bash
# Per-task teardown (instant, zero drift):
just deprovision <agent-name>

# This deletes: vCluster, host namespace, static kubeconfig,
#               opencode config, git workspace clone

# Routine host maintenance:
just list                     # active agents
just status                   # host cluster health
just prune                    # docker system prune

# Destroy everything:
just nuke                     # deletes k3d cluster entirely
```

The host cluster and observability stack are disposable too — keep the
`justfile`, `.devcontainer/`, and any Helm values in version control; there is
no state worth backing up.

---

## 11. Security & blast-radius model

| Boundary | Held by | Protects against |
|---|---|---|
| Docker socket + `k3d-agent-platform` admin kubeconfig | **Harness/host only** | Agents can't create containers, mount the host FS, or admin the host cluster. |
| Per-agent static vCluster kubeconfig | Each agent | Admin *only* inside its own virtual cluster. |
| `ResourceQuota` + `LimitRange` per vCluster | Harness | One agent can't starve the shared cluster / others. |
| `NetworkPolicy` per vCluster namespace | Harness | Agent pods can't reach other agents' pods. |
| `devcontainer.json` workspace mount | Harness | Agent only sees its own git workspace. |
| Docker systemd slice (`MemoryMax`/`CPUQuota`) | Host | **The host machine always survives.** |

**An agent can freely** do anything inside its own vCluster — worst case is its
cluster becomes unusable and the harness reissues it.

**Known caveats (by design):** vCluster isolation is soft (shared kernel/nodes);
observability is shared (all agents' telemetry visible in Grafana); Ingress sync
uses standard Ingress resources (not Gateway API `HTTPRoute`).

---

## 12. Scaling path

The agent-facing workflow is **identical** whether the host cluster is this k3d box or
a real multi-node cluster. When you outgrow one machine, point the harness at a
k3s/managed Kubernetes host cluster and re-run §4–§10 unchanged — the LGTM stack,
per-agent vClusters, static kubeconfigs, and MCP wiring all carry over with no
agent-side changes.

---

## 13. Skill file & agent orientation

A skill file at `.opencode/skills/agentic-k8s-platform.md` gives agents
immediate context about the platform — their vCluster, available tools, hostname
patterns, MCP servers, resource limits, and common workflows. OpenCode
auto-loads it when the project is opened.

## 14. Known issues & improvements

See `improvements.md` for a detailed log of issues found during end-to-end
testing, architecture decisions (Cilium evaluation), and remaining work items.
