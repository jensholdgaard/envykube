# Architecture & Use-Case

## What problem does this solve?

An LLM coding agent needs a **real Kubernetes cluster** to validate its work — not
mocks, not `docker-compose`, not guessing. It needs to deploy, expose, observe, and
iterate. Doing this on your laptop should be **safe, cheap, and zero-config**.

This platform gives each agent its own **ephemeral, isolated vCluster** with a shared
observability stack, a local Git server for code, and MCP servers for introspection.
One `just` command provisions an agent. One `just` command tears it down.

```mermaid
flowchart TB
    %% ── User ──
    USER(["👤 User / Harness"])
    BROWSER(["🌐 Browser
    *.localhost"])

    %% ── Host Machine ──
    subgraph Host["🖥️ Host Machine"]
        JUST["justfile
        19 recipes for full lifecycle"]
        FORGEJO[("Forgejo
        git server
        port 3000")]
    end

    %% ── k3d Cluster ──
    subgraph K3D["☸️ k3d agent-platform cluster"]
        direction LR

        subgraph Shared["Shared Services"]
            TRAEFIK["Traefik
            ingress :80/:443"]
            OBS["LGTM Observability
            Loki · Tempo · Prometheus · Grafana"]
            OTEL["OTel Collector
            otel-collector.observability:4317"]
        end

        subgraph Agents["Agent vClusters"]
            VC1["agent-alpha
            ns: vc-agent-alpha
            own API · CRDs · RBAC
            ResourceQuota · NetworkPolicy"]
            VC2["agent-beta
            ns: vc-agent-beta
            own API · CRDs · RBAC
            ResourceQuota · NetworkPolicy"]
        end
    end

    %% ── DevContainers ──
    subgraph DC1["📦 DevContainer: agent-alpha"]
        OC1["OpenCode + kubectl + helm + vcluster"]
        MCP1["MCP: Grafana + Kubernetes"]
    end
    subgraph DC2["📦 DevContainer: agent-beta"]
        OC2["OpenCode + kubectl + helm + vcluster"]
        MCP2["MCP: Grafana + Kubernetes"]
    end

    %% ── Connections ──

    %% User provisions
    USER -->|"just provision agent N"| VC1
    USER -->|"just provision agent N"| VC2
    USER -->|"just forgejo"| FORGEJO

    %% Agent ↔ vCluster (kubectl via static kubeconfig)
    OC1 -->|kubectl / helm| VC1
    OC2 -->|kubectl / helm| VC2

    %% Agent → Observability (MCP)
    OC1 -.->|Grafana MCP| OBS
    OC2 -.->|Grafana MCP| OBS

    %% Telemetry flow
    VC1 -->|OTLP| OTEL
    VC2 -->|OTLP| OTEL
    OTEL --> OBS

    %% Ingress sync
    VC1 -->|Ingress| TRAEFIK
    VC2 -->|Ingress| TRAEFIK

    %% Browser access
    TRAEFIK -.->|"app.agent-alpha.localhost
    grafana.platform.localhost
    traefik.localhost"| BROWSER
    FORGEJO -.->|"forgejo.platform.localhost"| BROWSER

    %% Git
    FORGEJO <-->|clone / push| DC1
    FORGEJO <-->|clone / push| DC2
```
```

## Architecture Layers

```mermaid
graph TB
    subgraph Layer4["Layer 4: Agent (DevContainer)"]
        direction LR
        A1["opencode + CLIs + MCP servers
        /workspace = git clone from Forgejo
        NO Docker socket, NO host FS"]
    end

    subgraph Layer3["Layer 3: Agent vCluster (per agent)"]
        direction LR
        V1["Virtual Kubernetes
        own API server, CRDs, RBAC, CoreDNS
        Ingress syncs to host Traefik
        Pods run in 'vc-NAME' host namespace"]
    end

    subgraph Layer2["Layer 2: Host Cluster (k3d)"]
        direction LR
        T["Traefik Ingress → :80, :443
        routes by hostname"]
        LGTM["LGTM Stack
        Loki (logs) + Tempo (traces) + Prometheus (metrics)
        + Grafana (dashboards) + OTel Collector"]
    end

    subgraph Layer1["Layer 1: Host Machine"]
        direction LR
        D["Docker Engine
        capped by systemd slice"]
        F["Forgejo
        private git server"]
        J["justfile
        19 recipes, full lifecycle"]
    end

    A1 -->|kubectl via static kubeconfig| V1
    A1 -->|MCP| LGTM
    V1 -->|Ingress sync| T
    V1 -->|OTLP| LGTM
    J -->|provision| V1
    J -->|launch DevContainer| A1
    J -->|start Forgejo| F
    A1 -->|git| F
    T -->|HTTP| Browser
```

## Agent Lifecycle

```mermaid
stateDiagram-v2
    [*] --> Provisioned: just provision agent 1
    Provisioned --> Isolated: just network-policy agent
    Isolated --> Ready: just workspace agent <repo-url>
    Ready --> Deploying: agent deploy + expose
    Deploying --> Validating: curl / MCP queries
    Validating --> Deploying: iterate (fix + redeploy)
    Validating --> Done: task complete
    Done --> [*]: just deprovision agent
```

## Data Flow: Observability

```mermaid
flowchart LR
    subgraph Agent["Agent vCluster pods"]
        APP[("My App
        OTel SDK")] --> COLLECTOR
    end

    COLLECTOR["OTel Collector
    otel-collector.observability.svc:4317"]

    COLLECTOR -->|traces| TEMPO["Tempo"]
    COLLECTOR -->|metrics| PROM["Prometheus"]
    COLLECTOR -->|logs| LOKI["Loki"]

    TEMPO --> GRAFANA["Grafana
    grafana.platform.localhost"]
    PROM --> GRAFANA
    LOKI --> GRAFANA

    GRAFANA -->|Grafana MCP| AGENT_OC["OpenCode Agent
    'What's the p99 latency?'"]
```

## Configuration Flow

```mermaid
flowchart TD
    USER["User / Harness"] -->|"just provision agent 1"| VCLUSTER
    USER -->|"just workspace agent <repo>"| WORKSPACE

    VCLUSTER["vCluster created
    ns: vc-agent
    NodePort: 30001"] --> KUBECONFIG["Static kubeconfig
    host.docker.internal:30001"]

    FORGEJO[("Forgejo")] -->|git clone| WORKSPACE
    WORKSPACE["Workspace
    $HOME/.../agent-workspaces/agent/"] --> DEVCONTAINER

    KUBECONFIG -->|bind mount, readonly| DEVCONTAINER
    OPENCODE_CFG["Per-agent opencode.json
    MCP: Grafana + K8s"] -->|bind mount, readonly| DEVCONTAINER

    DEVCONTAINER["DevContainer
    ubuntu:24.04
    + opencode + kubectl + helm + vcluster
    + MCP servers
    network: k3d-agent-platform"] --> AGENT

    AGENT["Agent ready
    KUBECONFIG=/home/agent/.kube/config
    workspace = /workspace"]
```

## Key Design Principles

| Principle | Implementation |
|---|---|
| **Zero blast radius** | Agents have their own vCluster. `kubectl delete all` only affects their namespace. Teardown is instant. |
| **Real Kubernetes** | vClusters have their own API server, CRDs, RBAC, CoreDNS. Not a mock. |
| **Shared observability** | One LGTM stack + OTel Collector serves all agents. Telemetry tagged by vCluster namespace. |
| **Cheap iteration** | Agents are cheap LLM models (~$0.01/1M tokens). They can explore, fail, and retry without financial risk. |
| **No Docker socket** | Agents can deploy and expose but can't create containers or access the host. |
| **Local-first** | Everything runs on one machine. No cloud dependencies. No external Git hosting. |
| **Justfile as interface** | User never touches `kubectl` or `helm` directly. `just provision` / `just deprovision` / `just nuke`. |
| **Agent orientation via skill** | `.opencode/skills/agentic-k8s-platform.md` gives agents instant context about the platform. |
