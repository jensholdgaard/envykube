# Chaos Runner

The operator-side Python runner at `scripts/chaos-run.py` (661 lines). It executes the
chaos suite against one agent's vCluster and reports a structured PASS/FAIL verdict.

## Where it runs

Runs on the **operator plane** against the agent's vCluster via its kubeconfig, never
inside the agent's container. The agent triggers the suite (by commenting `/chaos`), the
operator executes it — the same split as teardown. This means the agent cannot run, weaken,
or forge its own verdict.

## What it does

1. Loads scenarios from `chaos/scenarios/*.yaml`
2. Loads the contract from the workspace's `.chaos.yaml`
3. Installs `charts/chaos` into the vCluster (idempotent, `helm upgrade --install`)
4. Executes each selected scenario's steps sequentially, stopping at the first unmet
   expectation
5. Emits JSON on stdout with a `markdown` field ready for posting as a comment

## Exit codes

| Exit | Meaning |
|------|---------|
| 0 | Every selected scenario passed |
| 1 | At least one scenario failed — **the agent's code is wrong** |
| 2 | The harness could not run (missing `.chaos.yaml`, no kubeconfig, chart install failed) |

1 vs 2 matters: the agent must be told to fix its code only when its code is what broke.
An exit 2 posts a "could not run" comment and sets **no** verdict label.

## Architecture

### `Cluster` class

Wraps `kubectl` and service proxy access against one agent's vCluster. Reaches
fault-webhook and toxiproxy through the API server's service proxy:

```
/api/v1/namespaces/chaos-system/services/http:fault-webhook:8080/proxy/...
```

### `Runner` class

Implements all step types. Holds the contract, manages a capture dictionary and a restore
stack (LIFO callables for undoing mutations after each scenario), and provides environment
variables (`KUBECONFIG`, `VCLUSTER_NAME`, `CHAOS=1`) to contract commands.

## Step vocabulary

| Step | Meaning |
|------|---------|
| `deploy` / `ready` / `verify` | Run the matching `.chaos.yaml` command |
| `arm` | Arm the fault webhook — `mode` (`conflict` 409 / `error` 500 / `throttle` 429), `ordinals`, `operations`, `resources`, `namespaces` |
| `disarm` | Clear the armed fault |
| `assert-fired` | The armed fault actually fired ≥ `min` times |
| `kill` | Delete the pods matching a `target` (`app` / `dependency`) |
| `scale` | Scale a target to `replicas` |
| `stall` / `unstall` | Swap the container image for a pause image (`registry.k8s.io/pause:3.10`) — present, scheduled, never Ready, out of endpoints |
| `squeeze` / `unsqueeze` | Drop the memory limit to force a real kernel OOMKill |
| `partition` | NetworkPolicy cutting egress to the dependency for `seconds` |
| `toxic` / `untoxic` | Toxiproxy fault (`latency`, `reset_peer`, `bandwidth`) — needs `dependency.proxy: true` |
| `sleep` | Wait |
| `assert-same` | Two captures must be identical |
| `assert-recovered` | Retry `ready` + `verify` until pass or `timeout` expires |

### Common fields

- `expect: pass|fail` — default `pass`. Set to `fail` to assert a step _must_ fail.
- `capture: <name>` — save the output of a contract command for later `assert-same`.
- `note:` — the sentence the agent reads when this step is what failed. It names a
  _behaviour_, not an exit code.

## Contract commands

The runner sets `VCLUSTER_NAME`, `KUBECONFIG`, and `CHAOS=1` in the environment, sets
cwd to the workspace root, and runs the contract's shell commands bound by
`STEP_TIMEOUT` (300s).

---

## Operator commands (justfile)

```bash
just chaos-suite <name> [scenario...]   # the gate: default suite, or named scenarios
just chaos <name> [scenario...]         # ad-hoc, includes the `extended` suite
just chaos-install <name>               # install the kit into a vCluster
just chaos-list                         # list the scenario catalogue
```
