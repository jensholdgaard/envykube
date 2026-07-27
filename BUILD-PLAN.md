# Build Plan — opencode-driven agentic platform

Ordered roadmap for turning envykube into an opencode-driven, board-orchestrated,
warm-pool platform. Status: ✅ done · ⬜ todo.

## Architecture (decided)

- **Two-plane.** A privileged **operator** opencode on the host (holds the admin
  kubeconfig + Docker socket) vs. the sandboxed **per-agent** opencode in each
  DevContainer (scoped vCluster kubeconfig only, no admin, no Docker socket).
- **Board-driven, one heavyweight worker per task, no runtime leader.** Board =
  Forgejo issues/labels. A worker = a full vCluster + DevContainer + workspace.
  A "leader" only ever exists as an optional *detached planner* that fills the
  board and exits (deferred — see Open decisions).
- **Warm pool**, `minAvailable = 2–3`, **fresh-per-task**. Teardown = **Option A**:
  on the merge-to-main webhook the operator *always destroys* the finished (dirty)
  cluster; a reconciler holds `idle == minAvailable` via background fresh creates.
  Used clusters are never recycled.
- Operator lifecycle ops = opencode **custom tools**. P1 wraps the existing `just`
  recipes via Bun `$`; logic is absorbed into TypeScript incrementally where it pays.

## Declarative layout (Helm) ✅

Cluster state is Helm, not bash. Nothing is created by a `kubectl apply` heredoc any more.

```
charts/observability/   Mimir (monolithic) + OTel Collector + Grafana datasources + ingress
charts/platform/        Forgejo behind Traefik (selector-less Service + Endpoints + Ingress)
charts/agent/           per-agent ResourceQuota + LimitRange + optional NetworkPolicy
values/loki.yaml        \
values/tempo.yaml        }  config for the pinned UPSTREAM releases
values/grafana.yaml     /
values/vcluster.yaml    one vCluster release per agent (identical for every agent)
justfile                thin driver: installs the above in order
```

**Exactly one value must be supplied at install time anywhere in the repo:**
`--set forgejo.ip=<ip>`, because Forgejo is a Docker container on the k3d network and its IP is
only knowable at runtime. Every other chart installs with no `--set` — namespaces come from
`-n`, and policy numbers live in `values.yaml` next to the reasoning for them.

- `just render` — render every chart without touching the cluster (review / diff / audit)
- `just lint` — validate all charts
- Upstream chart versions pinned in the justfile: loki 7.1.0, tempo 1.24.4, grafana 10.5.15
  (this also delivers part of P8).

Deliberately still imperative, with reasons:
- **vCluster API NodePort** stays a `kubectl patch` — the Service belongs to the vCluster release,
  so a second Helm release cannot own it.
- **`just harden`** writes host systemd/docker files — outside Kubernetes entirely.
- **`just netpol`** (the experimental Cilium variant) is left as-is; the Cilium path is not the
  recommended dataplane.

## Critical path & parallelism

- Critical path (orchestration spine): **P1 → P2 → P3 → P4**.
- Independent, can interleave anytime: **P0** (determinism), **P5** (sovereignty/
  guardrails), **P6** (debug skills). **P7** (image slim-down) → **P8** (pinning)
  is its own short chain.

---

## P0 — Determinism quick wins (independent, high-ROI) ✅

Close the feedback-loop holes the "better harness → smaller models" theory depends on.

- ✅ `harden` rescoped, Metrics backend = Mimir monolithic, OTel Collector → Mimir OTLP,
  Alloy reverted, `observability` recipe rebuilt — all verified working.
- ✅ Run against a live cluster — verified.

## P1 — Two-plane skeleton + operator as driver (wrap, reversible) ✅

Whole platform drivable through opencode; justfile stays the source of truth.

- ✅ Operator opencode project in `operator/` with its own `opencode.jsonc`. The operator
  agent holds a host-level permission whitelist: `just *`, `k3d *`, `docker *`, `kubectl *`,
  `vcluster *`, `helm *` are all explicitly allowed; every other bash command is denied.
  Model/provider/MCP servers match the per-agent config. Operator config lives in a
  dedicated directory (`operator/`) with a symlink to `.opencode/` for shared resources.
- ✅ Custom tools (`.opencode/tools/*.ts`) wrapping recipes via Bun `$`: `provision`,
  `deprovision`, `observability`, `list`, `status`, `network-policy`, `nuke`, `workspace`
  (claim), `harden`. Each returns **structured JSON** (name, namespace, nodePort,
  kubeconfig path, URLs) — no scraped text.
- ✅ Operator runs as `opencode serve` via `just operator` (port 30998, hostname 0.0.0.0).
  The recipe switches to the `operator/` directory so opencode picks up the operator
  config, and the `.opencode/` symlink supplies the shared tools/skills/agents.

## P2 — `pool-provision` + `claim` refactor (enable the warm pool) ✅

Split the slow/flaky card-agnostic half from the fast card-specific half.

- ✅ `pool-provision` / `claim` are canonical names now (thin `just` aliases over the existing
  `provision` / `workspace`, so all docs keep working). `provision` is already the card-agnostic
  half (vCluster + quota + NodePort + kubeconfig); `workspace` is the card-bound half.
- ✅ **Retry/backoff around `vcluster connect`** in `provision` + `kubeconfig`: each attempt is
  `timeout 90`-bounded, retried 5× with linear backoff, and required to produce non-empty output.
  Kills the ">2 min hang" flakiness that made provisioning nondeterministic.
- ✅ `127.0.0.1` kubeconfig: `provision` now also emits a `<name>.host.yaml` variant with the
  `127.0.0.1` server (both addresses are in the cert SANs); non-fatal if it fails. Verified
  working against a live cluster.

## P3 — Warm-pool reconciler + board integration (orchestration core) ✅

The control loop.

- ✅ Reconciler as a standalone Python daemon (`scripts/pool-manager.py` + `scripts/pool-config.json`):
  polls Forgejo for `ready`-labelled issues, holds `idle >= minAvailable` warm vClusters,
  claims an idle member per card → relabels the issue `in-progress` → sends a prompt to
  the child agent's opencode serve. When the issue is closed, the vCluster is destroyed and
  the pool refills automatically. Pool state persisted to `~/.local/share/agent-pool/state.json`
  for crash recovery. Start via `just pool`.
- ✅ Config: `minAvailable` (default 3), `poolBase` (default 20 to avoid collisions with
  manual indices), poll interval, label scheme. Secrets (Forgejo creds) from `.env`.
- ✅ Fresh-per-task: only idle (clean, unclaimed) vClusters are used. Claimed vClusters are
  never returned to the idle pool. On issue close → destroy → refill to `minAvailable`.
- ✅ PR workflow: child agent creates a feature branch, pushes, opens a PR (linked to the
  issue via `Closes #{num}`), polls indefinitely for reviews. On `CHANGES_REQUESTED` it
  addresses feedback and re-pushes; on `APPROVED` it merges and labels the issue `review`.
- ✅ Ephemeral Forgejo identity: each agent gets its own user (`pool-{index}`) with write
  access to the repo, enabling comments on issues and PR operations with a traceable identity.
  Created on claim, deleted on teardown.
- ✅ Issue commenting: agents can post comments on the issue for Q&A, progress updates,
  and clarification requests throughout the PR lifecycle.
- ✅ Manual execution via `just pool` (human-supervised). Human closes the issue when
  satisfied — this triggers the teardown.

## P4 — Forgejo merge-webhook → operator reap (Option A teardown)

Agent-triggered, operator-executed teardown.

- ⬜ Forgejo webhook on merge-to-main → authenticated operator endpoint.
- ⬜ Handler maps repo/branch → worker → `deprovision` → pool refills to `minAvailable`.
  Validates it can only reap the mapped worker (secret-authenticated).

## P5 — Agent sovereignty + enforced guardrails ✅

Digital sovereignty + machine-enforced action-space constraint.

- ✅ `provider` block (DeepSeek default via `@ai-sdk/openai-compatible`, key from
  `{env:DEEPSEEK_API_KEY}`) + `enabled_providers` restriction + `model` — in both `opencode.jsonc`
  and the generated per-agent config (`workspace` recipe; model overridable via `AGENT_MODEL`).
  Swap the provider/model for any OpenAI-compatible or local endpoint. DevContainer now passes
  `DEEPSEEK_API_KEY` + `GRAFANA_SERVICE_ACCOUNT_TOKEN` through.
- ✅ `permission.bash` guardrails: deny `kubectl port-forward` and the LoadBalancer-expose patterns;
  `*: allow` otherwise. (Leaky against LoadBalancer-via-`apply -f` — bash matching can't read YAML.)
- ✅ Grafana MCP write **enabled** (dropped `--disable-write`) so agents build dashboards for the
  user. Requires the Grafana service-account token to hold the **Editor** role, or writes 403.

## P6 — Debug skills + subagent ✅

Agents validate feature requirements themselves (no `just validate`).

- ✅ Expanded `SKILL.md` with a "Debugging & Validating Your Work" section: the verify loop,
  symptom triage (Pending / CrashLoop / OOM / ImagePull / probes / Service / Ingress),
  `helm status/get/--dry-run/template/rollback`, Grafana-MCP correlation, and turning feature
  requirements into evidence-backed checks. Also fixed stale Prometheus / `mimir=distributed`
  references to Mimir monolithic.
- ✅ Read-mostly `k8s-debugger` subagent (`.opencode/agents/k8s-debugger.md`), bash-gated to
  read-only kubectl/helm + curl; mutating commands denied.
- ✅ `/diagnose <kind>/<name>` command (`.opencode/commands/diagnose.md`) runs the triage playbook.
- ✅ `workspace` recipe now copies `.opencode/agents/` into each agent workspace (it previously
  copied only skills + commands, so the subagent would never have reached the agents).

## PC — Chaos gate in the issue lifecycle ✅ (plane 1)

Chaos testing is a **mandatory stage** between "push" and "open a PR": the agent comments
`/chaos`, the operator runs the fault suite against its vCluster and posts a structured
verdict, and a PR without `chaos-passed` is blocked. Agent-triggered, operator-executed —
the same split as the P4 teardown.

- ✅ `charts/chaos` (installed *inside* the vCluster: deterministic fault-injecting admission
  webhook + toxiproxy, zero privilege), `chaos/scenarios/*.yaml`, `scripts/chaos-run.py`,
  `chaos_gate` + `chaos_enforcer` listeners, and reconciler polling for comments/PRs/reviews
  (no Forgejo webhook is registered, so that half of the lifecycle was previously dead in Rust).
- ⬜ Plane 2 — node-level faults (latency/loss/IO/time) from the host cluster via Chaos Mesh.
  Deliberately NOT inside agent vClusters: a privileged chaos-daemon there lands on a shared
  k3d node and ends the isolation boundary.
- ⬜ Forgejo branch protection on `main` (the only enforcement layer the agent cannot bypass).

**Full design, verification log and known gaps: `CHAOS-TESTING.md`.**

## P7 — Agent image slim-down (finalize the toolset)

Tighten the sandbox before pinning.

- ⬜ Multi-stage build: compile mnemoria (`cargo install --git one-bit/mnemoria --tag v0.3.5`)
  in a builder stage, copy the binary → drop rustup/cargo from the runtime image.
- ⬜ Remove the vcluster CLI from the agent image (operator-only).
- ⬜ Verify opencode's browser-UI feature set → keep or drop OpenChamber.

## P8 — Version pinning + Helm 4 migration (final toolset)

Reproducibility; cut to Helm 4 deliberately, with validation.

- ⬜ Pin the Dockerfile: kubectl `v1.36.3`, helm `v4.2.3` (get.helm.sh tarball), node
  `22.23.1`, opencode `v1.18.4`, uv `0.11.32`, mcp-grafana `0.17.2`, kubectl-mcp-server
  `1.24.0`, `@devtheops/opencode-plugin-otel@1.4.0`, `oc-mnemoria@0.3.5`, mnemoria git
  `v0.3.5`, `@openchamber/web@1.16.3` (if kept).
- ⬜ Pin the otel-collector image `0.157.0` (manifests) + helm chart versions in the
  justfile (loki / tempo / grafana). Mimir monolithic is a pinned manifest image
  (`grafana/mimir:3.1.4`), not a chart.
- ⬜ Document the host/operator tool pins: k3d `v5.9.0`, vcluster `v0.36.0`.
- ⬜ **Validate every `helm` call + the LGTM/vcluster charts under Helm 4.2.3** and fix
  breakages. Deferred to here on purpose so the early orchestration work isn't destabilized
  by a chart migration at the same time.

---

## Open decisions

- **OpenChamber vs. opencode-native UI** — resolve in P7 after verifying opencode's browser UI.
- **Execution mode default** — human-supervised vs. `--auto` unattended (P3 knob).
- **Agent model/provider** (P5) — Claude-via-API is too costly for the user privately; agents
  will run a cheaper/other model. The provider block is written model-agnostic; the user plugs
  in the endpoint.
- **RUM→Faro via Alloy** — a separate future observability workstream (browser session telemetry),
  not part of the LGTM metrics path. (Note: otel-collector-contrib ships a `faroreceiver`, worth a
  check if consolidating later.)
- **Gateway API vs Ingress** — candidate evolution (better tenancy: platform owns the Gateway,
  agents own HTTPRoutes; `allowedRoutes` could enforce per-agent hostname isolation). Gate on
  verifying vCluster can sync HTTPRoutes to the host (it currently syncs Ingresses); use Traefik v3,
  not Cilium. Forgejo/Grafana Ingresses migrate trivially to HTTPRoutes if adopted.
- **Deferred entirely for now:** cross-card dependencies, prioritization/ordering, a planner agent.
