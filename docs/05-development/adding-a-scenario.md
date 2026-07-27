# Adding a Chaos Scenario

Scenarios live at `chaos/scenarios/<name>.yaml`. Each one is a YAML file read by
`scripts/chaos-run.py` — the operator-side runner that executes the suite against an
agent's vCluster.

## Required Fields

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Unique identifier, matching the filename (e.g. `reapply-twice`) |
| `teaches` | string | One of `idempotency`, `foreseeing-errors`, or `invariant-recovery` |
| `suite` | string | `default` or `extended` |
| `description` | string | What the scenario tests and why |
| `steps` | list | Ordered list of step objects |

## Optional Fields

- `requires: [dependency]` — scenario needs a dependency declared in `.chaos.yaml`
- `requires: [proxy]` — scenario needs `dependency.proxy: true` (toxiproxy routing)

When a `requires` condition is not met, the scenario is skipped with a clear reason.

## Step Vocabulary

Each step object has a `do` field naming the step type, plus optional fields.

| Step | Required Fields | Optional Fields | Description |
|------|----------------|-----------------|-------------|
| `deploy` | — | `note`, `capture` | Runs the `deploy` command from `.chaos.yaml` |
| `ready` | — | `note`, `capture` | Runs the `ready` command from `.chaos.yaml` |
| `verify` | — | `note`, `capture`, `expect: fail` | Runs the `verify` command; `expect: fail` means the command should exit non-zero |
| `arm` | `mode` | `ordinals`, `operations`, `resources`, `namespaces`, `note` | Arms a fault on the admission webhook. Mode: `conflict`, `error`, `throttle` |
| `disarm` | — | | Resets the admission webhook to pass-through |
| `assert-fired` | `min` | `note` | Fails if the webhook fired fewer than `min` faults |
| `kill` | `target` | `note` | Deletes all pods matching the target's selector |
| `scale` | `target`, `replicas` | `note` | Scales target to `replicas` |
| `stall` | `target` | `note` | Swaps target containers for `pause` — pod exists, never Ready, no endpoints |
| `unstall` | `target` | | Restores the original images |
| `squeeze` | `target` | `memory`, `note` | Drops the memory limit to trigger OOMKill |
| `unsqueeze` | `target` | | Restores the original memory limits |
| `partition` | `seconds` | `note` | Cuts egress with a NetworkPolicy for N seconds |
| `toxic` | `type` | `latency`, `jitter`, `note` | Injects latency via toxiproxy |
| `untoxic` | — | | Removes the toxiproxy toxic |
| `sleep` | `seconds` | | Pauses execution for N seconds |
| `assert-same` | `a`, `b` | `note` | Asserts that two captured values are equal |
| `assert-recovered` | `timeout` | `note` | Polls `ready` + `verify` until both succeed or timeout |

## Example: Simplest Scenario (reapply-twice)

```yaml
name: reapply-twice
teaches: idempotency
suite: default
description: >-
  Applying the same work twice must converge to the same state.
steps:
  - do: deploy
    note: the first apply must succeed on a clean cluster
  - do: ready
  - do: verify
    capture: first
  - do: deploy
    note: the second apply is the whole test
  - do: ready
  - do: verify
    capture: second
  - do: assert-same
    a: first
    b: second
```

## Example: Dependency-Aware Scenario (dependency-down)

```yaml
name: dependency-down
teaches: foreseeing-errors
suite: default
requires: [dependency]
description: >-
  The declared dependency is scaled to zero. The process must not exit, and
  the health endpoint must tell the truth.
steps:
  - do: deploy
  - do: ready
  - do: scale
    target: dependency
    replicas: 0
  - do: verify
    expect: fail
  - do: scale
    target: dependency
    replicas: 1
  - do: assert-recovered
    timeout: 240
```

## Verification

After creating a scenario:

1. **List it**: `just chaos-list` — verifies it appears in the catalogue
2. **Run it**: `just chaos-suite <agent> <scenario>` — tests against a live agent
3. **Full suite**: `just chaos-suite <agent>` — runs the default suite including your new scenario
