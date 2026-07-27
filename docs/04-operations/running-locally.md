# Running Locally

The developer workflow for running the full platform on a single host machine.

## Step-by-step

### 1. Create the host cluster

```bash
just bootstrap
```

Creates a k3d cluster named `agent-platform` with 1 server + 2 agents, ports 80/443
published for the shared ingress, and NodePorts 30000-30050 pre-published for per-agent
vCluster API access. Context: `k3d-agent-platform`.

### 2. Cap Docker resources

```bash
just harden
```

Creates a `k3dcap.slice` systemd slice with `MemoryMax=48G`, `MemorySwapMax=0`,
`CPUQuota=1400%`, and configures Docker's `cgroup-parent` to place all containers
under it. The host always survives.

### 3. Start Forgejo (local Git server)

```bash
just forgejo
```

Starts the Forgejo container on the k3d docker network and routes it through host
Traefik via `charts/platform`. Accessible at `http://forgejo.platform.localhost`.

On first visit, create the admin account through the install form.

### 4. Deploy the LGTM observability stack

```bash
just observability
```

Deploys Loki, Tempo, Grafana, Mimir (monolithic), and the OTel Collector. After
deploy, mints the Grafana service-account token and writes it to `.env`.

Grafana is accessible at `http://grafana.platform.localhost`.

### 5. Start the pool manager

```bash
just pool
```

Starts `cargo run --release` — the pool manager reconciler. It maintains a warm pool
of idle vClusters and claims one per `ready`-labelled Forgejo issue.

### 6. Start continuous dashboard sync (optional)

In another terminal:

```bash
just watch-dashboards
```

Continuously syncs agent-created dashboard ConfigMaps from their vClusters into
Grafana every 10 seconds.

### 7. Create an issue

Create an issue with the `ready` label on Forgejo. The pool manager picks it up
automatically — it provisions a vCluster, clones the repo, launches a DevContainer,
and sends the initial prompt to the agent.

### 8. Monitor

```bash
just list       # active vClusters and their namespaces
just status     # host cluster health
just endpoints  # all endpoints: Traefik routes, ports, NodePorts
```

## Running the operator opencode server

```bash
just operator
```

Starts an opencode server on port 30998 with host-level permissions and custom tools
defined in `operator/opencode.jsonc` that wrap every just recipe into structured JSON.

## Per-agent lifecycle commands

```bash
just provision <name> <index>          # create isolated vCluster
just workspace <name> <index> <url>    # clone repo + launch DevContainer
just ui <name> <index>                 # open OpenChamber web UI
just network-policy <name>             # isolate agent's pods
just deprovision <name>                # tear everything down
```

## Teardown

```bash
just deprovision <name>   # one agent
just nuke                 # the entire host cluster
```
