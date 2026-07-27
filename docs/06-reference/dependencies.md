# Dependencies

Every external dependency and what it does.

## System (Host Machine)

- **Docker Engine** — container runtime for k3d cluster and DevContainers. Capped by
  systemd slice `k3dcap.slice` (MemoryMax=48G, MemorySwapMax=0, CPUQuota=1400%).
- **k3d** — runs Kubernetes-in-Docker. Creates `agent-platform` cluster (1 server 4g +
  2 agents 12g each).
- **kubectl** — Kubernetes CLI, configured via per-agent static kubeconfig.
- **helm** — package manager for Kubernetes. Installs all 4 charts (agent, chaos,
  observability, platform).
- **vcluster CLI** — creates ephemeral virtual Kubernetes clusters. Uses Helm driver.
  Pins API to NodePort.
- **devcontainer CLI** — builds and launches DevContainers from `.devcontainer/`
  config.
- **systemd** — resource capping via systemd slice
  (`/etc/systemd/system/k3dcap.slice`).
- **systemd-resolved** — wildcard DNS resolution (`*.localhost` → 127.0.0.1).
- **python3** — runs `scripts/chaos-run.py`, `scripts/sync-dashboards.py`,
  `scripts/oc-relay.py`, and inline Python in justfile recipes.
- **PyYAML** (Python) — YAML parsing for chaos-run.py (reads scenarios and
  `.chaos.yaml`).
- **Cilium CLI** (optional) — Cilium CNI management (experimental
  `just bootstrap-cilium`).

## Rust (crates/pool-manager/Cargo.toml)

- **tokio** (v1, full features) — async runtime. Drives all listeners, relay server,
  and reconciler timer.
- **axum** (v0.7) — HTTP framework for the webhook relay server (port 30990).
- **reqwest** (v0.12, rustls-tls + json) — HTTP client for Forgejo REST API. Uses
  rustls (no OpenSSL dependency).
- **serde / serde_json** (v1) — JSON serialization for config, state persistence,
  event models. `derive` feature for struct annotations.
- **tracing / tracing-subscriber** (v0.1 / v0.3) — structured logging. Supports
  env-filter and JSON output. Can export to OTLP.
- **thiserror** (v2) — derive macros for error types.
- **anyhow** (v1) — flexible error handling.
- **clap** (v4, derive) — CLI argument parsing for `--config` flag.
- **dotenvy** (v0.15) — loads `.env` file for environment variables
  (`DEEPSEEK_API_KEY`, `FORGEJO_USER`, `FORGEJO_PASSWORD`,
  `GRAFANA_SERVICE_ACCOUNT_TOKEN`).
- **base64** (v0.22) — Basic auth encoding for Forgejo API calls.
- **opentelemetry / opentelemetry_sdk / opentelemetry-otlp** (v0.24 / v0.17) —
  OpenTelemetry tracing export via OTLP over HTTP.
- **tracing-opentelemetry** (v0.25) — bridges tracing spans to OTel.

## Container Images

- **codeberg.org/forgejo/forgejo:9** — local Git server. Runs on k3d docker network,
  routed through Traefik at `http://forgejo.platform.localhost`.
- **oven/bun:1** — runtime for the fault-webhook handler (ConfigMap-based, no build
  needed).
- **ghcr.io/shopify/toxiproxy:2.12.0** — TCP proxy for latency injection
  (zero-privilege, no NET_ADMIN).
- **postgres:17-alpine** — example dependency image used in agent workloads.

## Observability (Helm Charts)

- **grafana/loki** (v7.1.0) — log aggregation. Configured via `values/loki.yaml`.
- **grafana/tempo** (v1.24.4) — distributed tracing. Configured via
  `values/tempo.yaml`.
- **grafana/grafana** (v10.5.15) — dashboards and visualization. Configured via
  `values/grafana.yaml`.
- **Mimir monolithic** — metrics store deployed via `charts/observability`. One
  deployment, no microservice sprawl.

## OpenCode Plugins

- **@devtheops/opencode-plugin-otel** — OpenCode OTel plugin. Exports token usage,
  tool calls, session costs as OTLP to the collector.

## Documentation

- **mdbook** (v0.5.4) — Rust-native static site generator for documentation.
- **mdbook-mermaid** (v0.17.0) — Mermaid diagram preprocessor for mdBook. Enables
  ` ```mermaid ` code blocks.
