# Feature List

Complete inventory of platform features.

## Built

- Per-agent isolated vClusters via vcluster CLI
- ResourceQuota + LimitRange per namespace (enforced at provision time)
- Static kubeconfig via pinned NodePort (host.docker.internal + 127.0.0.1 variants)
- DevContainers with per-agent opencode config and skills
- Forgejo Git server (local, docker network-routed through Traefik)
- Ephemeral agent identities (pool-N user, API token, write access to repo)
- Issue → agent claiming (ready label triggers claimer)
- Automated PR creation + review + merge (pr_monitor listener)
- LGTM observability stack (Loki + Tempo + Mimir monolithic + Grafana)
- OTLP collector (gRPC, port 30417 on host)
- Grafana + Kubernetes MCP servers (read-only telemetry, cluster ops)
- Dashboard-as-CM sync (agents create ConfigMaps, operator syncs to Grafana)
- Chaos gate:
  - Fault injection (admission faults, pod kills, scaling, OOM, stall, partition,
    toxiproxy latency)
  - `/chaos` command (agent triggers, operator executes)
  - PR blocking via `chaos_enforcer` (`REQUEST_CHANGES` review without `chaos-passed`)
  - Push invalidation (`PullRequestSynchronized` clears `chaos-passed`)
- Fault webhook (ConfigMap-based, Bun runtime, zero-privilege admission faults)
- Toxiproxy (userspace TCP proxy for latency injection, no NET_ADMIN)
- OpenChamber web UI (per-agent, port 31000+index)
- NetworkPolicy isolation (`just network-policy <name>`)
- Webhook relay (axum server on port 30990, Forgejo webhook → `Event`)
- Polling reconciler (periodic Forgejo API poll, publishes `ReconciliationTick`)
- Health monitor (checks agent liveness, nudges stalled agents)
- Justfile operator interface (`provision`, `workspace`, `deprovision`, `chaos-suite`,
  `sync-dashboards`, etc.)
- OpenCode relay (aggregates all agent opencode servers behind port 30999)
- Operator OpenCode server (host-level permissions, custom tools for just recipes)

## Planned / Experimental

- Cilium CNI (`just bootstrap-cilium`, eBPF kube-proxy replacement, Gateway API,
  Hubble)
- Node-level chaos (Chaos Mesh plane 2 — blocked by agent isolation boundary on shared
  k3d nodes)
- Forgejo branch protection (block merge on rejected reviews)

## Not Built

- Per-scenario cluster reset (scenarios run sequentially against the same cluster,
  cleanup is best-effort between runs)
- Hot-reload of pool-config without restart (config changes require a pool-manager
  restart)
