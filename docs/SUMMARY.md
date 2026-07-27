# Summary

[index.md](index.md) — landing page: what the platform does, quick start, architecture at a glance

# Architecture

- [Overview](01-architecture/overview.md) — four-layer stack, Mermaid diagrams of all components
- [Two-Plane Security Model](01-architecture/two-plane-security.md) — why operator/agent split exists, what each plane can do
- [Network Topology](01-architecture/network-topology.md) — DNS, Ingress sync, hostname patterns, port mapping
- [Event System](01-architecture/event-system.md) — Event enum, IssueNumber/PrNumber newtypes, Bus broadcast
- [Observability](01-architecture/observability.md) — LGTM stack, OTLP pipeline, dashboard sync, metric labels

# Lifecycle

- [Agent Lifecycle](02-lifecycle/agent-lifecycle.md) — state machine: provision → idle → claim → work → destroy
- [Issue Claiming](02-lifecycle/issue-claiming.md) — how ready-labeled issues become agent prompts
- [Chaos Gate](02-lifecycle/chaos-gate.md) — fault injection flow, scenarios, enforcement, step vocabulary
- [PR Review & Merge](02-lifecycle/pr-review-merge.md) — chaos_enforcer blocking, push invalidation, merge flow

# Components

- [Pool Manager](03-components/pool-manager.md) — Rust binary overview: config, event bus, all 10 listeners
- [Reconciler](03-components/reconciler.md) — polling engine: watermarks, conversation detection, startup recovery
- [Listeners](03-components/listeners.md) — spawn pattern, lag handling, shared helpers
  - [Claimer](03-components/listeners/issue-claimer.md) — IssueLabeled → reserve member → send initial prompt
  - [Chaos Gate](03-components/listeners/chaos-gate.md) — /chaos parsing, suite execution, verdict posting
  - [Chaos Enforcer](03-components/listeners/chaos-enforcer.md) — PR blocking, push invalidation, label clearing
  - [Health Monitor](03-components/listeners/health-monitor.md) — agent liveness, chaos-aware nudges
- [Chaos Kit](03-components/chaos-kit.md) — fault-webhook, toxiproxy, RBAC inside agent vClusters
- [Chaos Runner](03-components/chaos-runner.md) — operator-side Python runner, step vocabulary, exit codes
- [Charts](03-components/charts.md) — all 4 Helm charts: agent, chaos, observability, platform
- [Forgejo Client](03-components/forgejo-client.md) — REST API client: labels, issues, PRs, agent identities

# Operations

- [Setup](04-operations/setup.md) — first-time bootstrap, forgejo, observability, pool start
- [Justfile Reference](04-operations/justfile-reference.md) — every recipe grouped by domain
- [Running Locally](04-operations/running-locally.md) — dev workflow, operator opencode, dashboard sync
- [Troubleshooting](04-operations/troubleshooting.md) — symptoms → causes, common fixes

# Development

- [Adding a Chaos Scenario](05-development/adding-a-scenario.md) — YAML format, step vocabulary, catalogue registration
- [Adding a Listener](05-development/adding-a-listener.md) — file creation, event handling, spawn registration
- [Testing the Chaos Gate](05-development/testing-the-gate.md) — end-to-end: issue → /chaos → verdict → PR → merge
- [Documentation Conventions](05-development/conventions.md) — rules for agents maintaining docs

# Reference

- [Feature List](06-reference/feature-list.md) — every built, planned and not-built feature
- [Chaos Scenarios](06-reference/chaos-scenarios.md) — all 10 scenarios with teaches/requires/catches
- [Event Catalog](06-reference/event-catalog.md) — every Event variant with fields, source, and consumers
- [API Endpoints](06-reference/api-endpoints.md) — Forgejo, OpenCode, Relay, Grafana endpoints
- [Dependencies](06-reference/dependencies.md) — every external dependency grouped by category

# Appendix

- [Environment Idea](07-appendix/environment-idea.md) — original concept sketch
- [Improvements](07-appendix/improvements.md) — known issues and planned fixes
- [Vision](07-appendix/vision.md) — project vision and goals
