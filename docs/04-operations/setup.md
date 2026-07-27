# Setup

This page covers everything required to stand up the platform on a host machine.

## Prerequisites

| Requirement | Notes |
|-------------|-------|
| Linux, **cgroup v2** + systemd | Verify: `stat -fc %T /sys/fs/cgroup` → `cgroup2fs` |
| **Docker Engine** (rootful) | Binds `:80/:443` with no sysctl changes |
| CPU / RAM / disk | Per agent: ~1 vCPU + 1–2 GB control plane plus workload usage. Add ~4 vCPU + 8 GB for the LGTM stack. Starter box for ~5 agents + observability: **12+ vCPU, 48+ GB RAM, 150+ GB SSD** |
| Sized Docker data-root | Put `/var/lib/docker` on a dedicated filesystem |

## Toolchain installation

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
```

Verify with `just versions`.

## Wildcard DNS for `*.localhost`

Agents reach apps at `app.<cluster>.localhost` and shared services at
`grafana.platform.localhost`. These must resolve to `127.0.0.1`.

```bash
getent hosts app.cluster-alpha.localhost      # modern systemd-resolved already handles it

# If empty, add a wildcard (NetworkManager + dnsmasq shown):
echo 'address=/localhost/127.0.0.1' | \
  sudo tee /etc/NetworkManager/dnsmasq.d/localhost-wildcard.conf
sudo systemctl restart NetworkManager
```

## `just bootstrap` — Create the host cluster

```bash
just bootstrap
```

Runs:

```bash
k3d cluster create agent-platform \
  --servers 1 --agents 2 \
  --servers-memory 4g --agents-memory 12g \
  --port "80:80@loadbalancer" \
  --port "443:443@loadbalancer" \
  --port "30000-30050:30000-30050@server:0"
```

- `:80`/`:443@loadbalancer` → shared ingress entry point.
- `30000-30050@server:0` → pre-published NodePort range for per-agent vCluster API
  access (51 concurrent agents; widen as needed).
- Traefik ships enabled as the shared ingress controller.
- Context is now `k3d-agent-platform`. Keep it and the Docker socket on the host only.

## `just harden` — Cap Docker resources

k3d runs its node containers privileged, so memory flags don't impose a real kernel
limit. Cap the Docker daemon itself with a systemd slice:

```bash
just harden
```

This creates a `k3dcap.slice` with `MemoryMax=48G`, `MemorySwapMax=0` (hard ceiling,
no swap escape), and `CPUQuota=1400%`, and configures Docker's `cgroup-parent` to
place every container under it. Does not restart Docker if a cluster is already running
(the cap applies to containers created after the next `just bootstrap`).

## `just forgejo` — Start the local Git server

```bash
just forgejo
```

Starts Forgejo (`codeberg.org/forgejo/forgejo:9`) as a Docker container on the k3d
network, then installs `charts/platform` to route it through host Traefik. Accessible
at `http://forgejo.platform.localhost`.

Agents clone from here, push changes for review. No external auth needed.

## `just observability` — Deploy the LGTM stack

```bash
just observability
```

Deploys through Helm:

| Component | Chart |
|-----------|-------|
| Loki (SingleBinary) — logs | upstream `grafana/loki` v7.1.0 |
| Tempo — traces | upstream `grafana/tempo` v1.24.4 |
| Grafana — dashboards | upstream `grafana/grafana` v10.5.15 |
| Mimir (monolithic) — metrics | `charts/observability` |
| OTel Collector — OTLP fan-out | `charts/observability` |
| Grafana datasources + ingress | `charts/observability` |

After deploying, `just grafana-token` mints a service-account token for agent Grafana
MCP authentication, writing it to `.env` as `GRAFANA_SERVICE_ACCOUNT_TOKEN`.

The OTel Collector ingests OTLP on `otel-collector.observability.svc.cluster.local:4317`
and fans out to Tempo (traces), Mimir (metrics), and Loki (logs). Published on host
NodePorts: gRPC 30417, HTTP 30418.

Grafana is accessible at `http://grafana.platform.localhost`.

## `just pool` — Start the pool manager

```bash
just pool
```

Starts `cargo run --release` — the pool manager reconciler that maintains a warm pool
of idle vClusters and claims one per `ready`-labelled Forgejo issue. Add `--auto` for
unattended mode.

## Per-agent lifecycle

### `just provision`

```bash
just provision <agent-name> <index>
```

Creates an isolated vCluster with:
1. vCluster in namespace `vc-<agent-name>`
2. ResourceQuota + LimitRange (`charts/agent`)
3. Pinned API NodePort (`np_base + index`)
4. Static kubeconfigs exported to `~/.local/share/agent-kubeconfigs/`

Two kubeconfig variants are generated: `<name>.yaml` (uses `host.docker.internal`,
for in-container access) and `<name>.host.yaml` (uses `127.0.0.1`, for host-side tools).

### `just workspace`

```bash
just workspace <agent-name> <index> <forgejo-repo-url>
```

Clones the repo from Forgejo, sets up the DevContainer, generates a per-agent
`opencode.json` with MCP servers pointing at the agent's vCluster, builds the
DevContainer image, and launches the container.

### `just deprovision`

```bash
just deprovision <agent-name>
```

Tears down the agent: deletes the vCluster, host namespace, static kubeconfigs,
opencode config, and git workspace clone. Every step is best-effort so a
partially-provisioned agent still cleans up. Name is validated as a DNS label
(`^[a-z0-9]([a-z0-9-]*[a-z0-9])?$`) before any destructive step.

## Optional: watch-dashboards

```bash
# In a separate terminal:
just watch-dashboards
```

Continuously syncs agent-created dashboard ConfigMaps from their vClusters into
Grafana every 10 seconds.
