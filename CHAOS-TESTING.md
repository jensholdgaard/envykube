# Chaos Testing

Every issue must pass a deliberate-fault suite before its work can go up for review.
Status: ✅ built and verified live · ⬜ designed, not built.

---

## Why

Agents in this harness fail in three specific ways: they don't **foresee error paths**, they
write **non-idempotent code**, and they can't **recover from invariant states** (half-applied,
stuck, partially-failed). A happy-path `deploy → wait-ready → curl` loop is blind to all three,
because it never produces the states that expose them.

The premise of this repo is that a better harness substitutes for reasoning capacity in a weak
model. A model that has never *observed* a mid-apply pod kill cannot reason its way to
idempotency; one that observes it, gets a named failure, and re-runs, can. So the environment
manufactures those states on purpose.

**Deterministic, not a chaos monkey.** Every fault is a fixed, named step — "reject the 1st
matching write with 409", "scale the dependency to zero" — never a random one. A random killer
produces a failure the agent cannot reproduce, and for a small model that is the worst possible
feedback: it can't tell whether its fix worked or the dice simply landed differently.

---

## Why not Chaos Mesh or Litmus inside the vCluster

Chaos Mesh *does* run inside a vCluster — loft documents it. But everything past the API layer
needs a **privileged `chaos-daemon` DaemonSet mounting the container-runtime socket**. In
envykube that pod is synced into `vc-<name>` on a **k3d node container shared by every agent**,
where it can reach every other agent's containers and the host runtime. That ends the two-plane
isolation boundary in `BUILD-PLAN.md`. It also adds an unauthenticated RCE surface
(CVE-2025-59358…61, fixed in Chaos Mesh 2.7.3, mitigated by `--set enableCtrlServer=false`).

| Option | Verdict |
|---|---|
| Chaos Mesh **inside** an agent vCluster | ❌ privileged daemon on a shared node = isolation gone |
| Litmus | ❌ same split (`pod-delete` is pure API, netem/stress need `hostPID` + the runtime socket), and ChaosCenter is far too heavy for an ephemeral per-issue cluster |
| `kube-monkey` / `chaoskube` | ❌ zero-privilege but random ⇒ non-reproducible ⇒ weak iteration signal |
| Chaos Mesh in the **host** cluster, operator-driven | ⬜ the right home for node-level faults — see [Plane 2](#plane-2--node-level-faults-not-built) |
| Zero-privilege kit inside the vCluster | ✅ what this is |

**Two planes, mirroring the existing security model:**

| Plane | Runs in | Privilege | Driven by | Status |
|---|---|---|---|---|
| 1 — fault kit | the agent's vCluster | none | the agent, via `/chaos` | ✅ |
| 2 — node chaos | the host cluster | privileged (already is) | the operator only | ⬜ |

---

## The lifecycle

```
ready → claim → implement → push → /chaos ──fail──> fix, /chaos again
                                      │
                                     pass → open PR → review → merge → reap
```

The trigger is **agent-initiated, operator-executed** — the same split already chosen for
teardown. The agent asks for a run by commenting on its own issue; it never holds the privilege
to run the suite, so it cannot weaken or forge a verdict.

```
agent comments "/chaos"  →  reconciler poll (or webhook relay)  →  Event::IssueComment
   →  chaos_gate listener  →  just chaos-suite <name>            (operator plane)
   →  JSON report posted to the issue as ADMIN  →  label chaos-passed | chaos-failed
```

### Three enforcement layers

| # | Layer | Where | Status |
|---|---|---|---|
| 1 | The initial prompt orders `/chaos` before the PR step | `issue_claimer::send_initial_prompt` | ✅ |
| 2 | A PR without `chaos-passed` gets a blocking `REQUEST_CHANGES`; the approved-merge nudge is withheld; the stalled-agent nudge asks for `/chaos` instead of a PR | `chaos_enforcer`, `pr_monitor`, `health_monitor` | ✅ |
| 3 | Forgejo branch protection on `main` with *block merge on rejected reviews* | Forgejo repo settings | ⬜ **not configured** |

Layer 3 matters: the agent merges with **its own credentials** via the API, so a server-side
rule is the only thing it genuinely cannot route around. Until it is enabled, a determined
agent could merge past a blocking review.

**Staleness.** Pushing new commits to an open PR clears `chaos-passed` and re-blocks, otherwise
an agent could pass the gate on a stub and then push the real implementation. Detected by
watching the PR head SHA (`Event::PullRequestSynchronized`).

**Runaway guard.** `MAX_CHAOS_RUNS = 5` per card (`crates/pool-manager/src/state.rs`). On
exhaustion the gate stops, ignores further `/chaos`, and asks for a human — a weak model that
cannot work out the fix must escalate, not spin.

---

## The contract: `.chaos.yaml`

Chaos needs something to perturb. Each agent repo declares its own entrypoints at the root:

```yaml
namespace: default
deploy:  "./deploy.sh"                                   # MUST be safe to run twice
ready:   "kubectl rollout status deploy/visits --timeout=180s"
verify:  "kubectl exec deploy/visits -- psql -h postgres -U postgres -t -A -c 'select total from visits where id=1'"
selector: "app=visits"                                    # the app's pods
dependency:                                               # optional
  name: postgres
  selector: "app=postgres"
  port: 5432
  proxy: false                                            # true only for the toxiproxy scenarios
```

`$VCLUSTER_NAME` and `KUBECONFIG` are set for the commands; cwd is the workspace root. A missing
or incomplete `.chaos.yaml` fails the suite with exit 2 and an actionable message. Forcing the
agent to declare how its own work is deployed, readied and verified is valuable output in its
own right.

---

## Scenario catalogue

`chaos/scenarios/*.yaml`. `suite: default` is what the gate enforces; `suite: extended` is
opt-in via `just chaos <agent> <scenario>`.

| Scenario | Suite | Teaches | Catches |
|---|---|---|---|
| `reapply-twice` | default | idempotency | schema created without `IF NOT EXISTS`, seeds without `ON CONFLICT`, anything that appends instead of reconciling |
| `restart-cold` | default | invariant-recovery | a startup path that only works against an empty dependency |
| `kill-mid-apply` | default | idempotency | recovery that needs a human to clean up the half-built state first |
| `write-conflict` | default | idempotency | clobbering a concurrent change instead of re-reading on 409 |
| `api-flaky` | default | foreseeing-errors | a deploy that aborts on a transient 500/429 |
| `dependency-down` | default | foreseeing-errors | a process that exits at startup; a `/healthz` that returns 200 without touching the DB |
| `oom-squeeze` | extended | invariant-recovery | state held only in memory; a crash loop that is terminal |
| `stuck-not-ready` | extended | invariant-recovery | "the pod is Running" mistaken for "the pod is serving" |
| `partition` | extended | invariant-recovery | connections that hang forever with no timeout |
| `dependency-slow` | extended | foreseeing-errors | missing timeouts and probe budgets |

### Step vocabulary

A scenario is an ordered list of steps; the runner stops at the first unmet expectation and
reports which one, so what reaches the agent names a **behaviour**, not an exit code.

| Step | Meaning |
|---|---|
| `deploy` / `ready` / `verify` | run the matching `.chaos.yaml` command |
| `arm` | arm the fault webhook — `mode` (`conflict` 409 / `error` 500 / `throttle` 429), `ordinals`, `operations`, `resources` |
| `disarm` | clear the armed fault |
| `assert-fired` | the armed fault actually fired ≥ `min` times |
| `kill` | delete the pods matching a `target` (`app` \| `dependency`) |
| `scale` | scale a target to `replicas` |
| `stall` / `unstall` | swap the container image for a pause image — present, scheduled, never Ready, out of endpoints |
| `squeeze` / `unsqueeze` | drop the memory limit to force a real kernel OOMKill |
| `partition` | NetworkPolicy cutting egress to the dependency for `seconds` |
| `toxic` / `untoxic` | Toxiproxy fault (`latency`, `reset_peer`, `bandwidth`) — needs `dependency.proxy: true` |
| `sleep` | wait |
| `assert-same` | two captures must be identical |
| `assert-recovered` | retry `ready` + `verify` until they pass or `timeout` expires |

Common fields: `expect: pass|fail` (default `pass`), `capture: <name>`, `note:` — the sentence
the agent reads when that step is what failed.

---

## Components

### `charts/chaos/` — installed **inside** the agent's vCluster

The only chart in this repo that does not target the host cluster. Everything is deployable by
an ordinary namespace user: no privilege, no hostPath, no runtime socket. Both images are
upstream — **nothing needs a container build**, because agents have no Docker socket and the
platform has no in-cluster registry.

| Component | What it is |
|---|---|
| `fault-webhook` | a `ValidatingAdmissionWebhook` that rejects a deterministic *ordinal* subset of writes. `oven/bun:1` running a handler mounted from a ConfigMap. Control API on plain HTTP :8080, admission on TLS :8443 |
| `toxiproxy` | `ghcr.io/shopify/toxiproxy:2.12.0` — the only zero-privilege way to get latency (tc/netem would need `NET_ADMIN` in the target's netns) |
| RBAC | `chaos-runner` ServiceAccount + ClusterRole for the API-only faults |

Cost: 2 pods, ~192Mi against the agent's 8Gi / 50-pod quota.

### `scripts/chaos-run.py` — the runner

Runs on the **operator plane** against the agent's vCluster via its kubeconfig, never inside the
agent's container. Emits JSON on stdout with a ready-to-post `markdown` field.

| Exit | Meaning |
|---|---|
| 0 | every selected scenario passed |
| 1 | at least one failed — **the agent's code is wrong** |
| 2 | the harness could not run (missing `.chaos.yaml`, no kubeconfig, chart install failed) |

1 vs 2 matters: the agent is told to fix its code only when its code is what broke. An exit 2
posts a "could not run" comment and sets **no** verdict label.

### `crates/pool-manager` — the wiring

| File | Role |
|---|---|
| `listeners/chaos_gate.rs` | consumes `/chaos` comments → runs `just chaos-suite` → posts the report as admin → sets the verdict label → prompts the agent with the failing invariants |
| `listeners/chaos_enforcer.rs` | blocks PRs without `chaos-passed`; clears the label on push |
| `listeners/reconciler.rs` | **polls** comments / PRs / reviews (see [Polling](#polling-not-webhooks)) |
| `listeners/comment_router.rs` | skips comments authored by `pool-{index}` and bodies starting with `/` |
| `listeners/pr_monitor.rs` | withholds the merge nudge unless `chaos-passed` |
| `listeners/health_monitor.rs` | a stalled agent is nudged to run `/chaos`, not to open a PR |
| `state.rs` | `ChaosStatus`, `chaos_runs`, PR/comment/review watermarks; `Member::new` / `reset_for_issue` |

Labels `chaos-passed` / `chaos-failed` are created on boot by `main::ensure_labels` and
configured under `labels` in `scripts/pool-config.json`.

---

## Operator commands

```bash
just chaos-suite <name> [scenario...]   # the gate: default suite, or named scenarios
just chaos <name> [scenario...]         # ad-hoc, includes the `extended` suite
just chaos-install <name>               # install the kit into a vCluster (chaos-suite does this itself)
just chaos-list                         # the catalogue and what each scenario teaches
```

`charts/chaos` is registered in `just render` and `just lint` alongside the other charts.

---

## Design decisions worth not re-litigating

**`failurePolicy: Ignore`, not `Fail`.** A webhook that fails closed and then crashes would
reject every write in the agent's cluster — including the writes that would fix it. That is an
unrecoverable state in a sandbox whose whole purpose is producing *recoverable* ones. The trade
is that a dead webhook injects nothing **silently**, which is why every fault-injecting scenario
ends with `assert-fired`.

**`assert-fired` is not ceremony.** It caught a real hole during development: `kubectl apply`
computes its patch client-side and sends **no request at all** when the object already matches
the manifest, so an armed fault had nothing to reject and the scenario passed having tested
nothing. Scenarios that arm a fault now `scale` the live state away from the manifest first, to
guarantee the next apply is a real write.

**The CA is generated once and reused via `lookup`.** "These clusters are disposable so
regenerating the cert is fine" is wrong: the webhook process reads its key and cert at startup,
so an upgrade that rotates the Secret without rolling the pod leaves the API server trusting a
CA the process is not serving — `x509: certificate signed by unknown authority`, swallowed
silently by `failurePolicy: Ignore`. Observed for real. The Secret now carries `ca.crt` so the
next render reuses it, and the Deployment carries a `checksum/tls` annotation so a genuine
rotation rolls the pod.

**No `PATCH` in the webhook rules.** The admission layer only knows `CREATE` / `UPDATE` /
`DELETE` / `CONNECT`; both `kubectl patch` and `kubectl apply` arrive as `UPDATE`. Listing
`PATCH` is rejected by the API server at install time.

**The control APIs go through the API server's service proxy**
(`/api/v1/namespaces/chaos-system/services/http:fault-webhook:8080/proxy/...`), not
port-forward or an Ingress — both of which the platform forbids agents. A tool the agent is not
allowed to imitate teaches it the wrong pattern.

**`replicas: 1` on the fault webhook is load-bearing.** The ordinal counter lives in process
memory; a second replica would split the count and "the 3rd write" would stop meaning anything.

### Polling, not webhooks

`relay.rs` translates Forgejo webhooks into events, but **no webhook is registered** on the repo
and the Forgejo container sits on the k3d docker network with no stable route back to the host
relay on :30990. Before this work the Rust manager only ever saw `ready` labels — comments, PRs
and reviews never arrived, and that half of the lifecycle was being carried by the deprecated
`scripts/pool-manager.py`. The reconciler now polls comments, PRs and reviews every
`pollInterval`, advancing watermarks at publish time so a slow listener can't cause replays. The
webhook relay remains as an optional latency accelerator.

---

## Verified live (2026-07-27)

Full run against `adminadmin/board-integration-test` issue #19 — *"Visit counter API with
Postgres persistence"* — on pool member `pool-20`.

**Kit correctness first.** Run against a deliberately non-idempotent deploy: `reapply-twice`
reported FAIL with `configmaps "seed" already exists`. Fixed the deploy → 5/5 PASS. A kit that
cannot fail a known-bad input measures nothing.

**Then the gate, end to end:**

| Run | Verdict | What the report said |
|---|---|---|
| 1 | could not run (exit 2) | no `.chaos.yaml` — correctly **not** labelled a code failure |
| 2 | **FAIL** 0/6 | `relation "visits" already exists` — `CREATE TABLE` / `INSERT` with no guards |
| 3 | **FAIL** 5/6 | *"the workload never recovered once the dependency came back"* — schema only created at deploy time, lost when Postgres was scaled away |
| 4 | **PASS** 6/6 | — |

Then: PR #20 opened → `chaos gate green — PR may proceed to review`; a subsequent push →
`polled push to PR #20` → `clearing chaos-passed` → automatic `REQUEST_CHANGES`.

**One caveat on that evidence.** The fix-and-retry iterations were driven by hand, not by the
agent. The agent claimed the issue, received the chaos-first prompt and began implementing, then
stalled: `opencode serve` in the container ignores the mounted `deepseek/deepseek-v4-pro` config
*and* an explicit per-prompt model override, serving `opencode/ling-3.0-flash-free`, which
returns HTTP 429 under load. The DeepSeek key is valid and `opencode models` inside the container
resolves only DeepSeek models, so this is the fork's server-side provider resolution. Everything
from `/chaos` onward is verified; **the agent reading its own report and iterating unaided is
not.** Re-run the acceptance test once the model provider is fixed.

---

## Known gaps

- ⬜ **Plane 2 — node-level faults (not built).** Latency, packet loss, IO and time chaos need a
  privileged daemon, which belongs in the **host** cluster, driven by the operator against the
  agent's synced pods in `vc-<name>`. Sketch: install Chaos Mesh ≥ 2.7.3 with
  `--set enableCtrlServer=false`, `chaosDaemon.runtime=containerd`,
  `chaosDaemon.socketPath=/run/k3s/containerd/containerd.sock` (the k3s path — loft's blog uses
  the Docker socket, which is wrong for k3d). Verify what labels vCluster actually puts on synced
  pods before writing selector logic; do not assume `vcluster.loft.sh/managed-by`. Two host-side
  faults need no framework at all: restarting the vCluster control-plane pod
  (`kubectl -n vc-<name> delete pod <name>-0` — the agent's API blinks and watches take `410
  Gone`) and blipping its pinned API NodePort.
- ⬜ **Branch protection is not configured**, so enforcement layer 3 is inactive.
- ⚠️ **Scenarios share cluster state.** They run in sequence against the same namespace, so one
  scenario can poison the next — in run 2 above, five scenarios reported the same root cause
  because the first left a table behind. The verdict was right and the message was actionable,
  but the per-scenario attribution was misleading. A per-scenario reset (or ordering guarantee)
  would sharpen it.
- ⚠️ **`dependency-slow` requires the app to address its dependency through toxiproxy**
  (`dependency.proxy: true`). That is the honest cost of latency without privilege; it is why
  the scenario is `extended` rather than `default`.
- ⚠️ **The runner is operator-side only.** There is no in-container `just chaos`; the agent's
  workspace is a clone of *its* repo, not this one. The agent's self-service path is
  `/chaos <scenario>`, which is enough and keeps one implementation.

## Troubleshooting

| Symptom | Cause |
|---|---|
| Every scenario passes suspiciously fast | the fault webhook is not being called — check `kubectl -n chaos-system get pods` and the vCluster control-plane logs for `failed calling webhook`. `failurePolicy: Ignore` hides this; `assert-fired` is what catches it |
| `the armed fault fired 0 times` | the deploy sent no matching write. Usually `kubectl apply` short-circuiting on an unchanged object — move the live state away first |
| Exit 2, "no kubeconfig" | the suite runs on the host and needs the `127.0.0.1` variant, `<name>.host.yaml`. Re-run `just kubeconfig <name>` |
| `/chaos` does nothing | no claimed member holds that issue, or the reconciler isn't polling — check `polled new comment` in the pool-manager log |
| Agent never responds to a prompt | check `GET localhost:$((32000+index))/api/session/<id>/message` for `model` and `error` before suspecting the harness |
