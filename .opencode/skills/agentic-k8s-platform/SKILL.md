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
- **Your budget**: `requests.cpu: 4`, `requests.memory: 8Gi`, `limits.memory: 24Gi`,
  50 pods, 10 PVCs. **There is deliberately no CPU-limit ceiling** — containers that
  declare no CPU limit get none, so you never need to hand-write CPU limits just to fit
  the budget. What you must watch is **`requests`** (what the scheduler reserves) and
  **`limits.memory`**, where every container without an explicit memory limit is charged
  a 512Mi default — including **initContainers and sidecars**, which is the usual reason a
  large chart doesn't fit.
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
just observability               # Deploy LGTM stack (Loki + Tempo + Mimir monolithic + Grafana)
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

**In the user's browser** (host-only hostnames — these do *not* resolve from your pods):
- Grafana: `http://grafana.platform.localhost`
- Forgejo: `http://forgejo.platform.localhost`

**From your vCluster pods** — host-cluster services resolve by their full in-cluster FQDN,
`<service>.<namespace>.svc.cluster.local`. This is pre-configured; you do **not** need to
patch CoreDNS, create ExternalName Services, or look up ClusterIPs:
- OTel Collector: `otel-collector.observability.svc.cluster.local:4317`
- Mimir: `mimir.observability.svc.cluster.local:8080`
- Grafana: `grafana.observability.svc.cluster.local:80`

Use the **full** FQDN. A short name like `mimir.observability` will NXDOMAIN.

## Observability

The host cluster runs a shared LGTM stack. Your workloads automatically send
telemetry via the OTel Collector in the `observability` namespace.

- **Traces** → Tempo
- **Metrics** → Mimir (monolithic)
- **Logs** → Loki

### Agent-side telemetry (OTel Plugin)

Your own actions as an agent are automatically exported to the same
observability stack via the `@devtheops/opencode-plugin-otel` plugin.
This gives you real-time visibility into token usage, tool call durations,
session costs, and more — all queryable in Grafana.

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

**Grafana MCP** — Query telemetry (read-only via Viewer token):
- "What's the p99 latency of my-svc?"
- "Show error logs for my-svc in the last 15 minutes"
- "Find slow traces for the checkout flow"

**Kubernetes MCP** — Structured cluster operations:
- List/get/create/delete resources
- Diagnose pod issues
- Query events and logs

### Creating Grafana Dashboards

Dashboards are NOT created through the Grafana MCP (it's read-only). Instead, create
them as Kubernetes resources — a ConfigMap in your vCluster's default namespace. The
host operator picks them up and pushes them to Grafana.

The ConfigMap must have the annotation `platform.agentic-k8s.dev/dashboard: "true"` and
three data keys: `title` (the dashboard name), `uid` (a stable unique ID — keeps the
dashboard URL stable across updates), and `json` (the Grafana dashboard JSON model):

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: my-app-dashboard
  annotations:
    platform.agentic-k8s.dev/dashboard: "true"
data:
  title: "My App Dashboard"
  uid: "my-app"
  json: |
    {
      "uid": "my-app",
      "title": "My App Dashboard",
      "panels": [ ... ],
      ...
    }
```

Apply it: `kubectl apply -f dashboard.yaml`

**Sync behaviour:**
- Dashboards appear in Grafana within ~10 seconds — a `watch-dashboards` daemon on
  the host polls all agent vClusters and pushes ConfigMaps to Grafana automatically.
  No manual operator step.
- To **update** a dashboard, edit the ConfigMap and re-apply. The sync overwrites
  the Grafana dashboard with the same `uid`.
- To **delete** a dashboard, delete the ConfigMap (`kubectl delete cm my-app-dashboard`).
  The next sync cycle removes it from Grafana.
- The sync tags every dashboard with `agent:<your-name>` and
  `managed-by:agentic-platform` in Grafana. These are visible in the Grafana UI.

**Important: sync lag.** After `kubectl apply`, querying the dashboard via the Grafana
MCP may return empty for up to 10 seconds. Pre-validate with `kubectl get cm` on
the ConfigMap itself, then wait and retry the Grafana MCP query. If a query returns
"dashboard not found," check that `just watch-dashboards` is running on the host.

**The `uid` field is required.** It keeps the dashboard URL stable in Grafana and
is how the sync tracks which dashboards to delete when a ConfigMap is removed.
Pick a short, unique, kebab-case name.

### Host-Service DNS

Host-cluster services are reachable from your workloads by **FQDN only**:

```
otel-collector.observability.svc.cluster.local:4317   ✅  works
mimir.observability.svc.cluster.local:8080             ✅  works
mimir.observability                                    ❌  NXDOMAIN (short names don't resolve)
```

The vCluster CoreDNS forwards unknown cluster-local queries to the host. Always use the
full `.<namespace>.svc.cluster.local` suffix when targeting host services.

### Metric Labels

Mimir promotes these OTel resource attributes as metric labels:

| Attribute | Label in Mimir |
|---|---|
| `service.name` | `service_name` |
| `service.namespace` | `service_namespace` |
| `k8s.namespace.name` | `k8s_namespace_name` |
| `k8s.pod.name` | `k8s_pod_name` |
| `deployment.environment` | `deployment_environment` |

The legacy `job` label also exists (mapped from `service.name` by OTel ingest).

**Known-good PromQL** — always use an explicit range; `$__rate_interval` depends on
scrape assumptions that don't apply to OTLP-pushed metrics:

```promql
rate(http_requests_total{service_name="my-svc"}[5m])
histogram_quantile(0.99, rate(http_request_duration_seconds_bucket{service_name="my-svc"}[5m]))
```

If a metric series is empty, the app probably isn't sending OTLP yet — not "everything is fine".

### Checking Your Resource Budget

The ResourceQuota lives in the host namespace, not inside the vCluster. To check it:

```bash
# In your terminal (the vCluster can't see the host quota):
kubectl --kubeconfig /home/agent/.kube/config describe quota agent-quota
```

Or use the Kubernetes MCP to query the quota. If pods are Pending, check `kubectl describe
pod <name>` for quota-related events — look for `exceeded quota` or `limitrange` in the
output.

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
- ❌ `kubectl delete svc kubernetes` — this is the vCluster API service. Deleting it
  breaks your cluster.
- ❌ `kubectl delete <resource> --all` — bulk deletion. Delete specific resources instead.
- ❌ Short hostnames for host services — e.g. `mimir.observability`. Always use the FQDN:
  `mimir.observability.svc.cluster.local`.

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
git remote add origin http://forgejo.platform.localhost/<user>/<repo>.git
git push origin main
```

## Debugging & Validating Your Work

There is no `just validate` — **you** verify your own work with the real tools
(`kubectl`, `helm`, the k8s MCP, and the Grafana MCP). Validation means proving each
of a feature's technical requirements against a real signal from the cluster, not
guessing. The loop is always: **change → apply → wait for the real state → read the
signal → diagnose → iterate.** For a focused triage of one resource, use `/diagnose
<kind>/<name>` or delegate to the `k8s-debugger` subagent.

### The verify loop for any deployment
```bash
kubectl rollout status deploy/<app> --timeout=120s   # did it actually become ready?
kubectl get pods -l app=<app> -o wide                 # phase, restarts, node
kubectl get endpoints <svc>                            # is the Service backed by ready pods?
curl -sS -o /dev/null -w '%{http_code}\n' http://<app>.<your-name>.localhost/healthz
```
A green rollout + non-empty endpoints + the HTTP code you expect = the feature is
actually working. Anything else → triage below, then re-run this loop.

### Triage by symptom

**Pending / Unschedulable** — the scheduler can't place the pod.
```bash
kubectl describe pod <pod>            # read the Events at the bottom
```
Your ResourceQuota lives in the **host** namespace, so `kubectl describe quota` inside your
vCluster returns `not found`. When the quota is the blocker, the signal is a **`SyncError`
event from `pod-syncer`** on the Pending pod, naming the exact resource:

```
Error syncing to host cluster: ... exceeded quota: agent-quota,
requested: limits.memory=1Gi, used: limits.memory=11434Mi, limited: limits.memory=12Gi
```

Read that event before changing anything — it tells you which dimension you actually hit.
Remember the quota charges the **512Mi default memory limit** for every container that
declares none, initContainers and sidecars included, so a chart's real footprint and its
quota cost are not the same number.

**Important:** the pod-syncer gives up after ~2 attempts and does **not** retry once the
blocker clears. After you fix the cause, the already-Pending pods stay Pending forever —
delete them so the ReplicaSet recreates them:
```bash
kubectl delete pods -l <selector> --field-selector=status.phase=Pending
```
Skipping this step is what makes a quota problem look like "my fix didn't apply".

**CrashLoopBackOff / restarts climbing**
```bash
kubectl logs <pod> --previous          # logs from the CRASHED instance, not the restart
kubectl describe pod <pod>             # exit code + last state
```
Read the *previous* container's logs — the current one may be too young to show the
error. Exit 1 = app error (read logs); 137 = OOM/SIGKILL (below).

**OOMKilled** — `describe` shows `Last State: Terminated, Reason: OOMKilled`. Raise the
container memory limit or fix the leak. Your namespace LimitRange sets defaults if you
specified none.

**ImagePullBackOff / ErrImagePull**
```bash
kubectl describe pod <pod>             # exact registry/auth/tag error in Events
```
Wrong tag, private registry without a pull secret, or a typo. (No Docker socket here —
you deploy pre-built images, you don't build them.)

**Running but not Ready** — a readiness probe is failing.
```bash
kubectl describe pod <pod>             # "Readiness probe failed: ..." in Events
```
Wrong probe path/port, or the app isn't up yet. Until Ready, the Service has no
endpoints and traffic won't route.

**Service returns nothing / connection refused**
```bash
kubectl get endpoints <svc>            # empty = no Ready pods match the selector
kubectl get svc <svc> -o yaml          # does spec.selector match the pod labels?
```

**Ingress not reachable in the browser**
```bash
kubectl get ingress                    # host + backend service/port correct?
kubectl get endpoints <svc>            # backend actually has endpoints?
```
The vCluster syncs your Ingress up to host Traefik automatically — you never install an
ingress controller. If the host resolves but you get a 404/502, the backend Service or
its endpoints are the problem, not the Ingress.

### Helm debugging
```bash
helm status <release>                              # release state + notes
helm get manifest <release>                        # what actually got applied
helm get values <release> -a                       # values in effect (incl. computed defaults)
helm upgrade <release> <chart> --dry-run --debug   # render + validate without applying
helm template <chart> -f values.yaml               # render locally, no cluster needed
helm history <release>                             # revisions; roll back: helm rollback <release> <rev>
```

### Correlate with telemetry (Grafana MCP)

Once the workload is up, use the **Grafana MCP** to check *behavior*, not just liveness —
ask in natural language:
- "Show error-level logs for `<svc>` in the last 15 minutes" (Loki)
- "What's the p99 latency / error rate of `<svc>`?" (Mimir)
- "Find the slowest traces for the `<flow>` request" (Tempo)

Your app must emit OTLP to the collector (see *Configuring your apps to send OTLP*) for
metrics/traces to appear. An empty panel usually means **no data being sent**, not "no
problem" — confirm the app is actually exporting before concluding it's healthy.

### Validating a feature's requirements

Turn each requirement into a check you can run and point at:
- "endpoint returns 200 with the right body" → `curl` it and read the body
- "writes N rows" → `kubectl exec` into the pod (or run a Job) and query
- "emits metric X" → Grafana MCP query for that metric series
- "survives a restart" → `kubectl rollout restart` then re-run the verify loop

Report what you verified and the evidence for it. If a requirement isn't yet proven, say
so explicitly rather than assuming.

## Environment Specifics

- **No Docker socket** — you cannot build or push container images directly.
  Use pre-built images or have the harness build them.
- **No host filesystem access** — your workspace is `/workspace` (bind-mounted
  from the host, scoped to your agent).
- **No other agent's resources** — you cannot see other agents' vClusters or
  workspaces. Network isolation via NetworkPolicy (if enabled by the harness).
- **Your ceiling** — a ResourceQuota caps your requests, memory limits, pods and PVCs
  (see *Your budget* above). It is enforced in the **host** namespace, so you cannot
  `describe` it from inside your vCluster; you learn you hit it from the `pod-syncer`
  `SyncError` event on a Pending pod.
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
