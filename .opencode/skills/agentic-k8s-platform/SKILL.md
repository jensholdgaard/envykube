---
name: agentic-k8s-platform
description: Agentic Kubernetes development environment. Use for deploying, validating, and tearing down workloads in isolated per-agent vClusters with shared observability.
---

# Agentic Kubernetes Platform

You are an agent operating inside an ephemeral, isolated Kubernetes development
environment. You have your own vCluster, a shared observability stack, and a
local Git server. Everything is designed for rapid iteration with zero blast
radius.

## Your vCluster

You were provisioned by the harness with `just provision <your-name> <index>`.
Your vCluster runs in the host cluster namespace `vc-<your-name>`.

- **kubeconfig**: already mounted at `$KUBECONFIG` (read-only, static)
- **API access**: via NodePort (no live proxy to maintain)
- **Resource limits**: 6 CPU, 12 GiB memory, 50 pods, 10 PVCs (configurable per agent)
- **Ingress sync**: vCluster syncs your Ingress resources to the host cluster's
  Traefik automatically

## Justfile Recipes

The project justfile provides the full lifecycle. Run `just` to list all recipes.

### Provisioning & teardown
```
just provision <name> <index>    # Create your vCluster
just deprovision <name>          # Destroy your vCluster
just kubeconfig <name>           # Re-export your static kubeconfig
```

### Observability (host cluster — already running)
```
just observability               # Deploy LGTM stack (Prometheus by default)
just observability mimir=distributed  # Or full Mimir (heavy, ~25 pods)
just grafana                     # Open Grafana in browser
```

### Host cluster management
```
just bootstrap                   # Create host cluster (default — flannel + Traefik)
just bootstrap-cilium            # EXPERIMENTAL — Cilium dataplane
just harden                      # Cap Docker daemon resources (sudo)
just nuke                        # Delete entire host cluster
just prune                       # Clean up Docker disk usage
just status                      # Host cluster health
just list                        # All active vClusters
just versions                    # Verify toolchain
```

### Git & workspace
```
just forgejo                     # Start local Git server (Forgejo)
just workspace <name> <index> <repo-url>  # Clone + build + launch DevContainer
just ui <name> <index>           # Open OpenChamber web UI for an agent
```

## Network & Hostnames

```
*.localhost       → 127.0.0.1 (systemd-resolved wildcard)
```

When you create an Ingress in your vCluster:
- Hostname pattern: `<app>.<your-vcluster-name>.localhost`
- Example: `my-app.agent-alpha.localhost`
- vCluster syncs it to host Traefik automatically — no port-forwarding needed

Host cluster services (accessible from your vCluster pods):
- Grafana: `http://grafana.platform.localhost`
- Forgejo: `http://forgejo.localhost:3000`
- OTel Collector: `otel-collector.observability.svc.cluster.local:4317`

## Observability

The host cluster runs a shared LGTM stack. Your workloads automatically send
telemetry via the OTel Collector in the `observability` namespace.

- **Traces** → Tempo
- **Metrics** → Prometheus (or Mimir if `mimir=distributed`)
- **Logs** → Loki

### Agent-side telemetry (OTel Plugin)

Your own actions as an agent are automatically exported to the same
observability stack via the `@devtheops/opencode-plugin-otel` plugin.
This gives you real-time visibility into token usage, tool call durations,
session costs, and more — all queryable in Grafana.

### Shared agent memory (Mnemoria)

The `oc-mnemoria` plugin gives you a persistent shared memory store at
`.opencode/mnemoria/`. All agents write to and read from the same store.

Key tools:
- `remember` — Store an observation (discovery, decision, problem, solution, etc.)
- `search_memory` — Hybrid BM25 + semantic search across all agents' memories
- `ask_memory` — Natural language question against the memory store
- `memory_stats` — View store statistics
- `timeline` — Browse memories chronologically
- `forget` / `compact` — Maintenance (mark obsolete; rebuild store)

Slash commands: `/mn-stats`, `/mn-recent`, `/mn-timeline`, `/mn-search <q>`, `/mn-ask <q>`

### Configuring your apps to send OTLP

Set these env vars on your deployments:
```yaml
env:
  - name: OTEL_EXPORTER_OTLP_ENDPOINT
    value: http://otel-collector.observability.svc.cluster.local:4317
  - name: OTEL_SERVICE_NAME
    value: <your-service-name>
```

### Querying via MCP

You have two MCP servers configured:

**Grafana MCP** — Query telemetry directly:
- "What's the p99 latency of my-svc?"
- "Show error logs for my-svc"
- "Find slow traces for the checkout flow"

**Kubernetes MCP** — Structured cluster operations:
- List/get/create/delete resources
- Diagnose pod issues
- Query events and logs

## Accessing Your Agent

### Web UI (OpenChamber)
OpenChamber provides a browser-based interface for your agent — chat, diff viewer,
git management, integrated terminal, and more. It's auto-started when your
DevContainer launches.

Your OpenChamber URL: `http://localhost:<port>` (printed by `just workspace`).

Restart it anytime: `just ui <your-name> <index>`

### CLI (opencode)
Attach directly to the OpenCode session:
```bash
devcontainer exec --workspace-folder ~/.local/share/agent-workspaces/<your-name> opencode
```

### Shell
Attach a plain shell to the DevContainer:
```bash
docker exec -it $(docker ps -q --filter "label=devcontainer.local_folder=<your-workspace>" | head -1) bash
```

## Deploying & Exposing Services — READ THIS FIRST

**You are inside a DevContainer.** `localhost` means *this container*, not the
host. **The user's browser cannot reach `localhost` in your container.**

### What NOT to do

- ❌ `kubectl port-forward` — pointless. Forwards to the container, user can't reach it.
- ❌ `helm install traefik / nginx-ingress / any ingress controller` — already handled
  by the host cluster. You'll break your vCluster or waste time.
- ❌ `kubectl expose --type=LoadBalancer` — use ClusterIP + Ingress instead.
- ❌ Exposing on `localhost:8080` or `localhost:ANYTHING` — the user will never see it.

### What TO do (only 3 commands)

```bash
kubectl create deployment my-app --image=nginx
kubectl expose deployment my-app --port=80
kubectl create ingress my-app --rule="my-app.<YOUR-NAME>.localhost/*=my-app:80"
```

Replace `<YOUR-NAME>` with your vCluster name (e.g. `my-agent-2`).
The app is now at `http://my-app.<YOUR-NAME>.localhost` in the user's browser.

## Common Patterns

### Deploy and expose (one-liner)
```bash
kubectl create deployment my-app --image=nginx && \
kubectl expose deployment my-app --port=80 && \
kubectl create ingress my-app --rule="my-app.my-agent-2.localhost/*=my-app:80"
```

### Deploy a Helm chart
```bash
helm repo add <repo> <url>
helm install my-release <chart> -n default
```

### Debug a failing deployment
```bash
kubectl describe pod <name>
kubectl logs <name>
kubectl get events --sort-by=.lastTimestamp
```
Then use the **Grafana MCP** to correlate with metrics and traces.

### Push changes to Forgejo
```bash
git remote add origin http://forgejo.localhost:3000/<user>/<repo>.git
git push origin main
```

## Environment Specifics

- **No Docker socket** — you cannot build or push container images directly.
  Use pre-built images or have the harness build them.
- **No host filesystem access** — your workspace is `/workspace` (bind-mounted
  from the host, scoped to your agent).
- **No other agent's resources** — you cannot see other agents' vClusters or
  workspaces. Network isolation via NetworkPolicy (if enabled by the harness).
- **Your ceiling** — ResourceQuota caps your CPU/memory/pods. Check your limits
  with `kubectl describe quota agent-quota`.
- **Teardown is instant** — `just deprovision <your-name>` wipes your vCluster,
  namespace, kubeconfig, and workspace completely. No drift.

## Platform Architecture

```
HOST MACHINE
  Docker Engine (capped by just harden)
  └── k3d agent-platform cluster
      ├── kube-system (Traefik, CoreDNS, local-path-provisioner)
      ├── observability (Loki, Tempo, Prometheus/Mimir, Grafana, OTel Collector)
      └── vc-<agent-name> (your vCluster pods — synced from virtual cluster)
```

Your vCluster has its own API server, CRDs, RBAC, and CoreDNS. It behaves like
a real Kubernetes cluster. Everything you deploy runs as pods in the host
cluster namespace `vc-<your-name>`.
