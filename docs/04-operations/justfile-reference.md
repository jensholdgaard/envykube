# Justfile Reference

Grouped reference of all recipes in the `justfile`. Run `just` with no arguments to list them.

## Host Setup

| Recipe | Description |
|--------|-------------|
| `versions` | Verify the toolchain is installed (docker, kubectl, helm, k3d, vcluster, devcontainer) |
| `bootstrap` | Create the shared k3d host cluster (flannel CNI + Traefik ingress) |
| `bootstrap-cilium` | EXPERIMENTAL: Create cluster with Cilium CNI + eBPF kube-proxy replacement + Gateway API + Hubble |
| `harden` | Cap Docker with a systemd slice (MemoryMax=48G, CPUQuota=1400%, MemorySwapMax=0) |
| `nuke` | Delete the entire host cluster (agents, observability, everything) |

## Git & Forgejo

| Recipe | Description |
|--------|-------------|
| `forgejo` | Start Forgejo (local Git server) and route it through host Traefik via `charts/platform` |

## Observability

| Recipe | Description |
|--------|-------------|
| `observability` | Deploy the shared LGTM stack (Loki + Tempo + Mimir monolithic + Grafana + OTel Collector) |
| `grafana-token` | Mint the Grafana service-account token agents authenticate with and write it into `.env` |
| `sync-dashboards` | Pull agent-created dashboard ConfigMaps from vClusters and push them to Grafana |
| `watch-dashboards` | Continuous dashboard sync — runs `sync-dashboards` in a loop every 10s |
| `opencode-relay` | Aggregate all agent opencode API servers behind one port (localhost:30999) |
| `operator` | Run the operator opencode server on port 30998 with custom tools |

## Pool Management

| Recipe | Description |
|--------|-------------|
| `pool` | Start the pool manager reconciler (`cargo run --release`); `--auto` for unattended mode |
| `pool-provision` | Alias for `provision` (canonical name for the operator/board model) |
| `claim` | Alias for `workspace` (canonical name for the operator/board model) |

## Per-Agent Lifecycle

| Recipe | Description |
|--------|-------------|
| `provision` | Create a vCluster + ResourceQuota + pinned NodePort + static kubeconfig |
| `workspace` | Clone repo from Forgejo and launch a DevContainer for an agent |
| `ui` | Open the OpenChamber web UI for an agent (or restart it if needed) |
| `network-policy` | Apply default-deny cross-agent NetworkPolicy (works with any CNI; toggle with `false`) |
| `netpol` | Apply CiliumNetworkPolicy for an agent (requires Cilium dataplane) |
| `kubeconfig` | Re-export an agent's static kubeconfig (reads the pinned NodePort back from the svc) |
| `deprovision` | Tear down an agent (vCluster + namespace + kubeconfig + opencode config + DevContainer) |

## Chaos Gate

| Recipe | Description |
|--------|-------------|
| `chaos-install` | Install the fault kit into an agent's vCluster (idempotent; `chaos-suite` does this itself) |
| `chaos-suite` | Run the chaos gate for an agent (default suite); exit 0=PASS, 1=FAIL, 2=ERROR |
| `chaos` | Run one named scenario or the extended suite (`--suite all`) |
| `chaos-list` | List the scenario catalogue with what each one teaches |

## Documentation

| Recipe | Description |
|--------|-------------|
| `docs` | Build the mdBook documentation (outputs to `docs/book/`) |
| `docs-serve` | Serve the documentation with live reload (open http://localhost:3000) |

## Inspect / Operate

| Recipe | Description |
|--------|-------------|
| `list` | List live vClusters and their namespaces |
| `status` | Host cluster health (k3d node list + kubectl get nodes) |
| `endpoints` | Discover every endpoint: Traefik Host-routes, published container ports, and NodePorts |
| `render` | Render charts to stdout without touching the cluster; `just render agent` for one chart |
| `lint` | Lint all 4 charts via `helm lint` |
| `grafana` | Open Grafana in a browser |
| `hubble` | Open the Hubble network-flow UI (Cilium dataplane only; needs the cilium CLI) |
| `prune` | Reclaim disk from dangling images/volumes (`docker system prune -f`) |
