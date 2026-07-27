# The chaos suite — scenario authoring reference

Deterministic, replayable fault scenarios that every issue must pass before its work goes up
for review. This directory holds the scenario catalogue; the components the scenarios drive
live in `charts/chaos` (installed *inside* the agent's vCluster).

> This file covers **writing and reading scenarios**. For the whole picture — why the gate
> exists, why Chaos Mesh is not installed in agent vClusters, how the lifecycle and its three
> enforcement layers fit together, the verification log and the known gaps — see
> **`../CHAOS-TESTING.md`**.

## Why deterministic and not a chaos monkey

A random pod killer produces a failure you cannot reproduce. For a small model that is the
worst possible feedback: it cannot tell whether its fix worked or whether the dice simply
landed differently. Every fault here is an **ordinal or a fixed step** — "fail the 3rd write
with 409", "kill the app pod after the apply starts" — so a failing run is debuggable and a
passing re-run is evidence.

## The contract: `.chaos.yaml`

Chaos needs something to perturb. Each repo declares its own entrypoints at the repo root:

```yaml
namespace: default              # where the app lives inside the vCluster
deploy:  "kubectl apply -k ./deploy"          # MUST be safe to run twice
ready:   "kubectl rollout status deploy/api --timeout=120s"
verify:  "curl -fsS http://api.$VCLUSTER_NAME.localhost/healthz"
selector: "app=api"             # the app's pods — used by kill / stall / squeeze
dependency:                     # optional; enables the dependency-* scenarios
  name: postgres
  selector: "app=postgres"
  port: 5432
```

A missing or unparseable `.chaos.yaml` fails the suite. That is deliberate: declaring how your
own work is deployed, readied and verified is part of the work.

## Scenario format

A scenario is an ordered list of steps. The runner stops at the first unmet expectation and
reports exactly which one, so the failure that reaches the agent names a behaviour, not an
exit code.

| Step | Meaning |
|---|---|
| `deploy` / `ready` / `verify` | run the matching `.chaos.yaml` command |
| `arm` | arm the fault webhook: `mode` (conflict / error / throttle), `ordinals`, `resources`, `operations` |
| `disarm` | clear the armed fault |
| `assert-fired` | the armed fault actually fired at least `min` times — guards against a silently dead webhook |
| `kill` | delete the pods matching a `target` (`app` or `dependency`) |
| `scale` | scale a target to `replicas` — how `dependency-down` removes a dependency without touching the app |
| `stall` / `unstall` | swap the container image for a pause image: the pod exists, never becomes Ready, endpoints drop |
| `squeeze` / `unsqueeze` | drop the memory limit to force a real OOMKill |
| `partition` | NetworkPolicy cutting egress to the dependency for `seconds` |
| `toxic` / `untoxic` | Toxiproxy fault on the dependency (`latency`, `reset_peer`, `bandwidth`) — needs `dependency.proxy: true` |
| `sleep` | wait |
| `assert-same` | two captures must be identical |
| `assert-recovered` | retry `ready` + `verify` until they pass or the timeout expires |

Common fields: `expect: pass \| fail` (default `pass`), `capture: <name>`, `note:` — the
sentence the agent reads when the step is what failed.

## Suites

`suite: default` is what the gate enforces. `suite: extended` is opt-in
(`just chaos <agent> <scenario>`) and holds the scenarios that need something extra from the
repo — `dependency-slow` requires the app to address its dependency through the proxy, and
the rest are slower or narrower.

## Where the runner runs

`scripts/chaos-run.py` executes on the **operator plane**, against the agent's vCluster via
its kubeconfig — never inside the agent's container. The agent triggers a run by commenting
`/chaos` (optionally `/chaos <scenario>`) on its issue; the pool-manager runs the suite and
posts the report back as an admin comment. The agent can ask for the suite, and it can read
every word of the verdict; it cannot run, weaken or forge one. Same split as teardown.
