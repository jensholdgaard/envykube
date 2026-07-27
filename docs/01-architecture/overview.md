# Architecture Overview

The Agentic Kubernetes Platform is a four-layer stack that gives AI coding agents
isolated Kubernetes clusters, a shared observability pipeline, and a mandatory
chaos-testing gate before any work goes to review.

## Four-Layer Architecture

### Layer 4 — Agent DevContainer

The agent's execution sandbox. A DevContainer with:

- **opencode** CLI (the agent harness)
- **kubectl** (scoped to its vCluster via static kubeconfig)
- **helm** (for deploying workloads)
- **MCP servers**: Grafana (query telemetry) + Kubernetes (cluster ops)
- **No Docker socket** — agents push code to Forgejo, not images

The DevContainer is built per-agent by `just workspace`. Ports are deterministic:
OpenChamber UI at `31000 + index`, opencode API at `32000 + index`. The kubeconfig
is static (CA embedded, no live proxy) and targets `host.docker.internal` at the
agent's pinned NodePort.

### Layer 3 — Agent vCluster

Each agent gets a full virtual Kubernetes cluster, provisioned by
`vcluster create --driver helm`. Inside the vCluster the agent has:

- **Own API server** — real kube-apiserver, not a mock
- **Own CRDs** — agents can install conflicting CRDs across vClusters without cross-contamination
- **Own RBAC** — the agent is effectively cluster-admin inside its vCluster
- **Own CoreDNS** — forwards unknown `svc.cluster.local` queries to host DNS
- **Workload isolation** — pods run in the host namespace `vc-<name>`, opaque to other agents

The vCluster syncs Ingress resources up to the host Traefik so that
`<app>.<agent-name>.localhost` routes correctly with no port-forwarding.
The OTel Collector service is replicated from host `observability` namespace into
each vCluster's `observability` namespace so that
`otel-collector.observability.svc.cluster.local` resolves natively
(via `networking.replicateServices.fromHost` in `values/vcluster.yaml`).

**Agent budget** (enforced by ResourceQuota + LimitRange per namespace):

| Resource | Limit |
|---|---|
| `requests.cpu` | 4 |
| `requests.memory` | 8Gi |
| `limits.memory` | 24Gi |
| Pods | 50 |
| PVCs | 10 |

There is no CPU limit ceiling — only CPU requests are quota'd, so containers
never hit CFS throttling. Default memory limit for containers without explicit
limits (including initContainers and sidecars) is 512Mi.

### Layer 2 — Host Cluster (k3d)

A single k3d cluster named `agent-platform` with 1 server + 2 agents
(4g / 12g memory). Created by `just bootstrap`. Ports published:

- `80` and `443` → loadbalancer (Traefik ingress)
- `30000–30050` → server:0 (pinned NodePorts for vCluster APIs)

Shared services run in the `observability` namespace:

| Service | Chart / Component | Version |
|---|---|---|
| Loki (logs) | `grafana/loki` | 7.1.0 |
| Tempo (traces) | `grafana/tempo` | 1.24.4 |
| Grafana (dashboards) | `grafana/grafana` | 10.5.15 |
| Mimir (metrics) | `charts/observability` | monolithic, one deployment |
| OTel Collector | `charts/observability` | gRPC on NodePort 30417 |

Traefik handles ingress routing by Host header. vCluster Ingress resources
are synced to host Traefik automatically.

### Layer 1 — Host Machine

The bare-metal or VM running everything:

- **Docker Engine** — capped by systemd slice `k3dcap.slice` (`just harden`:
  default 48G memory, 1400% CPU quota, `MemorySwapMax=0`)
- **Forgejo** Git server — Docker container on the `k3d-agent-platform` network,
  HTTP at `forgejo.platform.localhost` (via Traefik), SSH on `localhost:2222`
- **justfile** — ~800 lines of Bash, the full operator interface for lifecycle
  management (bootstrap, provision, workspace, chaos, deprovision, nuke)
- **pool-manager** — Rust binary that maintains a warm pool of vClusters,
  claims them for ready-labelled issues, and coordinates the chaos gate

## Platform Overview

```mermaid
flowchart TB
    USER(["👤 User Browser"])

    subgraph Host["🖥️ Host Machine — Layer 1"]
        JUST[justfile]
        FORGEJO[Forgejo Git server]
        POOL[pool-manager]
        CHAOS[chaos-run.py]
    end

    subgraph K3D["☸️ k3d Cluster — Layer 2"]
        TRAEFIK[Traefik ingress]
        subgraph OBS["observability Namespace"]
            LOKI[Loki logs]
            TEMPO2[Tempo traces]
            MIMIR[Mimir metrics]
            GRAFANA[Grafana dashboards]
            OTEL[OTel Collector]
        end

        subgraph Agents["Agent vClusters — Layer 3"]
            VC1["pool-20: own API, CRDs, CoreDNS"]
            VC2["pool-21: own API, CRDs, CoreDNS"]
        end

        PLATFORM["platform: Forgejo → Traefik routing"]
    end

    subgraph DC["📦 DevContainers — Layer 4"]
        OC1["pool-20: opencode + kubectl + MCPs"]
        OC2["pool-21: opencode + kubectl + MCPs"]
    end

    POOL -->|"just provision"| VC1
    POOL -->|"just workspace"| OC1
    POOL -->|"just chaos-suite"| CHAOS
    OC1 -->|kubectl| VC1
    OC1 -->|git| FORGEJO
    OC1 -.->|Grafana MCP| GRAFANA
    VC1 -->|Ingress sync| TRAEFIK
    VC1 -->|OTLP| OTEL
    OTEL --> LOKI
    OTEL --> TEMPO2
    OTEL --> MIMIR
    CHAOS -->|fault injection| VC1
    TRAEFIK -.->|"*.localhost"| USER
    FORGEJO -.->|forgejo.platform.localhost| USER
    GRAFANA -.->|grafana.platform.localhost| USER
```

## Vertical Stack View

```mermaid
flowchart TB
    subgraph L4["Layer 4 — Agent DevContainer"]
        OP[opencode CLI]
        KC[kubectl]
        HM[helm]
        GR[Grafana MCP]
        K8[Kubernetes MCP]
        OP --> KC
        OP --> HM
        OP --> GR
        OP --> K8
    end

    subgraph L3["Layer 3 — Agent vCluster"]
        API[kube-apiserver]
        CDNS[CoreDNS]
        CRDS[Custom CRDs]
        RBAC[RBAC]
        PODS[Workload Pods]
        API --- CDNS
        API --- CRDS
        API --- RBAC
    end

    subgraph L2["Layer 2 — Host k3d Cluster"]
        direction LR
        T2[Traefik]
        LGTM[LGTM Stack]
        OTEL2[OTel Collector]
    end

    subgraph L1["Layer 1 — Host Machine"]
        direction LR
        DKR[Docker Engine]
        FJ2[Forgejo]
        JF[justfile]
        PM[pool-manager]
    end

    L4 -->|kubectl via pinned NodePort| L3
    L4 -->|git| L1
    L3 -->|Ingress sync| L2
    L3 -->|OTLP telemetry| L2
    L2 -->|runs on| L1
    L3 -->|pods in vc-NAME ns| L1
```

## Data Flow: Agent → OTel → LGTM → Grafana MCP → Agent

```mermaid
sequenceDiagram
    participant APP as Agent Workload Pod<br/>(in vCluster)
    participant OTEL as OTel Collector<br/>observability.svc<br/>(replicated into vCluster)
    participant LOKI as Loki<br/>(logs)
    participant TEMPO as Tempo<br/>(traces)
    participant MIMIR as Mimir<br/>(metrics)
    participant GRAF as Grafana
    participant MCP as Grafana MCP Server<br/>(in DevContainer)
    participant AGENT as opencode Agent<br/>(in DevContainer)

    APP->>OTEL: OTLP gRPC<br/>otel-collector.observability.svc.cluster.local:4317
    Note over APP,OTEL: vCluster CoreDNS resolves FQDN<br/>via replicated Service

    OTEL->>LOKI: Logs<br/>otlphttp → loki:3100/otlp
    OTEL->>TEMPO: Traces<br/>otlp → tempo:4317
    OTEL->>MIMIR: Metrics<br/>otlphttp → mimir:8080/otlp
    Note over MIMIR: Promotes OTel resource attributes<br/>service.name → service_name<br/>k8s.namespace.name → k8s_namespace_name<br/>k8s.pod.name → k8s_pod_name

    AGENT->>MCP: "find error patterns in my app"
    MCP->>GRAF: GET /api/ds/query
    GRAF->>LOKI: LogQL
    GRAF->>TEMPO: TraceQL
    GRAF->>MIMIR: PromQL
    GRAF-->>MCP: Query results
    MCP-->>AGENT: Structured diagnostics

    Note over AGENT,MCP: Agent sees telemetry for its<br/>own workloads only (via MCP)
```

## Hostname Pattern

Two DNS zones coexist, each serving a different purpose:

| Pattern | Purpose | Example |
|---|---|---|
| `*.localhost` | Browser access, Traefik host routing | `app.pool-20.localhost` |
| `*.svc.cluster.local` | Pod-to-pod within cluster (FULL FQDN only) | `otel-collector.observability.svc.cluster.local` |

Browser DNS resolution: `*.localhost` → systemd-resolved wildcard → `127.0.0.1`.
In-cluster, vCluster CoreDNS forwards unknown cluster-local queries to host DNS.
Short names (like `otel-collector`) return NXDOMAIN — the full FQDN is required.
