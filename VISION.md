# Vision & Requirements

## What this platform is

An agentic Kubernetes development environment where multiple AI coding agents
operate in isolated per-agent vClusters, share one observability stack, push
code to a local Git server, and are managed from a single browser tab.

## What each agent gets

| Resource | Spec |
|----------|------|
| vCluster | Isolated K8s API server, 6 CPU / 12 GiB / 50 pods / 10 PVCs |
| Kubeconfig | Static, pinned NodePort, no live proxy |
| DevContainer | Ubuntu 24.04, kubectl, helm, vcluster CLI, opencode |
| Git | Cloned from local Forgejo, pushes for review |
| Ingress | `*.{name}.localhost` — synced to host Traefik automatically |

## What agents share

| Service | Access |
|---------|--------|
| Grafana | `grafana.platform.localhost` (LGTM stack) |
| OTel Collector | `host.docker.internal:30417` (traces→Tempo, metrics→Prometheus, logs→Loki) |
| Forgejo | `forgejo.localhost:3000` (Git hosting) |
| Agent memory | `.opencode/mnemoria/` (git-mediated, shared via commits) |

## How agents are constrained

- **No port-forward** — use Ingress only (`kubectl create ingress --rule=`)
- **No LoadBalancer / localhost exposure** — ClusterIP + Ingress is the only pattern
- **No Docker socket** — agents push code, not images
- **No cross-agent visibility** — NetworkPolicy isolates vCluster traffic
- **ResourceQuota** — hard CPU/memory/pod/PVC caps per namespace

## Single pane of glass

One browser page lists all running agents with links to their OpenChamber web UIs.
`just dashboard` generates it. Each agent gets a deterministic port (`31000 + index`).

## What this platform is NOT

- A multi-agent orchestration framework (agents work independently)
- A CI/CD pipeline (agents push to Forgejo, humans review)
- A model router or API proxy
- A replacement for production K8s clusters (it's a dev sandbox)

## Key design decisions

- **Trunk-based dev, not worktree swarms** — one branch per agent, one workspace
- **Per-agent UIs, not a centralized multi-session tool** — OpenChamber per DevContainer,
  dashboard for switching. Centralized tools (CodeNomad, TermHive, Stoneforge) add
  orchestration complexity without benefit for independent agents
- **Git-mediated memory, not real-time sync** — oc-mnemoria stores in `.opencode/`,
  committed and pushed alongside code
- **OTel for agent telemetry** — `opencode-plugin-otel` exports agent session data
  (tokens, costs, tool durations) to the shared Grafana stack
