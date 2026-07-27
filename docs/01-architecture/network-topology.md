# Network Topology

## DNS and Routing

### Browser Resolution

`*.localhost` resolves via systemd-resolved wildcard → `127.0.0.1`. This is how
the user reaches all platform services in a browser:

- `grafana.platform.localhost` → Grafana dashboards
- `forgejo.platform.localhost` → Forgejo Git server
- `app.pool-20.localhost` → An agent's deployed application (via Traefik host routing)

No `/etc/hosts` entries, no dnsmasq. The wildcard is provided by systemd-resolved's
built-in handling of `.localhost`.

### In-Cluster DNS

vCluster CoreDNS handles DNS inside the virtual cluster. Unknown `svc.cluster.local`
queries are **forwarded to host DNS** (enabled by `networking.advanced.fallbackHostCluster: true`
in `values/vcluster.yaml`).

Critical rule: **FULL FQDN is required** for host-cluster services. Short names
like `otel-collector` return NXDOMAIN. Use the full form:

```
otel-collector.observability.svc.cluster.local:4317
```

This FQDN resolves because the OTel Collector Service is replicated from the host
`observability` namespace into each vCluster's `observability` namespace
(`networking.replicateServices.fromHost`). The target namespace is created inside
the vCluster during `just provision`.

## Traefik Ingress

Traefik runs on the host k3d loadbalancer, listening on `:80` and `:443`.
It routes by **Host header**:

- vCluster Ingress resources are synced up from the vCluster to host Traefik
  automatically (`sync.toHost.ingresses.enabled: true`)
- The Forgejo container runs outside k3d (Docker network `k3d-agent-platform`)
  but is routed through Traefik via a `platform` Helm chart that creates an
  Endpoint + Service + Ingress pointing at the container IP

```
Browser → :80 (Traefik) → {Host header match} → {vCluster workload or Forgejo}
```

## Agent API Access (NodePorts)

Each agent's vCluster API server is exposed on a deterministic NodePort:

| Port | Formula | Example (index=20) |
|---|---|---|
| vCluster API | `30000 + index` | 30020 |
| OpenChamber UI | `31000 + index` | 31020 |
| opencode API | `32000 + index` | 32020 |

The NodePort is pinned by patching the vCluster Service after creation
(`kubectl patch svc <name> --type merge`). Kubeconfigs are **static** — the
CA certificate is embedded, and the server address is
`https://host.docker.internal:<port>` for in-container clients. No live proxy
process (`vcluster connect`) is needed.

Two kubeconfig variants are exported:

| File | Server Address | Used By |
|---|---|---|
| `<name>.yaml` | `host.docker.internal:<port>` | DevContainer (kubectl, MCPs) |
| `<name>.host.yaml` | `127.0.0.1:<port>` | Host-side tooling (may fail with EOF on some setups) |

The vCluster control plane proxy includes both `host.docker.internal` and `127.0.0.1`
in its TLS certificate SANs (`controlPlane.proxy.extraSANs` in `values/vcluster.yaml`).

## Port Map

```
Host Machine
  :80      → Traefik HTTP ingress          (published by k3d bootstrap)
  :443     → Traefik HTTPS ingress         (published by k3d bootstrap)
  :2222    → Forgejo SSH                    (published by just forgejo)
  :30000+  → vCluster API NodePorts         (published by k3d bootstrap)
  :30417   → OTel Collector gRPC           (NodePort, charts/observability)
  :30990   → pool-manager Forgejo webhooks  (listened by pool-manager)
  :30998   → Operator opencode server       (just operator)
  :30999   → opencode relay                 (just opencode-relay)
  :31000+  → OpenChamber web UIs            (DevContainer port mapping)
  :32000+  → opencode API servers           (DevContainer port mapping)
```

## Forgejo Access

- **HTTP**: `http://forgejo.platform.localhost` — routed through host Traefik
  via the `platform` Helm chart. The Forgejo container IP is discovered at
  runtime with `docker inspect` and wired into a Traefik Ingress.
- **SSH**: `localhost:2222` — published directly by the Forgejo Docker container.
  Used for `git clone` / `git push` from DevContainers (which can reach the
  host via `--add-host=forgejo.platform.localhost:host-gateway`).
