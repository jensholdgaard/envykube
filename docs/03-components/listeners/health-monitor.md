# Health Monitor

The health monitor at `crates/pool-manager/src/listeners/health_monitor.rs` checks
whether claimed agents are still alive and nudges stalled ones toward their next step.

## Tick Frequency

The health monitor watches `ReconciliationTick` events and maintains a local counter
(`local_tick`). It only acts on every **3rd** tick (`local_tick % 3 == 0`), meaning
it runs once per ~30 seconds (3 × 10s poll interval).

## Alive Check

For each member with status `Claimed`, the monitor:

1. Updates `last_health_tick` on the member
2. Calls `agent.is_alive(index)` — an HTTP POST to
   `http://localhost:{32000+index}/api/session` with a 5-second timeout
3. If the agent responds with a valid session ID → alive, continue
4. If not alive → proceed to nudge logic

`is_alive()` retries the session creation endpoint; if it returns a session ID, the
agent's OpenCode serve is reachable and responsive.

## Nudge Logic

When an agent is not alive, the monitor first checks whether a PR already exists for
the issue (via `find_related_pr()`). If a PR exists, the monitor does nothing — the
agent has progressed past the stage where nudges apply.

If no PR exists, the monitor checks the `chaos-passed` label:

### Chaos not passed → `nudge_run_chaos()`

- Guards: the `nudge_create_pr` flag must not already be set, and `chaos_status` must
  not be `Running` (never nudge while the suite is in flight)
- Sets `nudge_create_pr = true` (one-shot — prevents repeated prodding on every health
  tick)
- Sends the agent a prompt:
  ```
  Your work on issue #{n} has not passed the chaos suite yet, so do NOT open a PR.
  Make sure /workspace/.chaos.yaml describes how your work is deployed, readied and
  verified, commit and push it to branch issue-{n}, then request a run:
  curl -s -X POST "http://forgejo:3000/api/v1/repos/{repo}/issues/{n}/comments" \
    -u "$FORGEJO_USER:$FORGEJO_PASSWORD" -H "Content-Type: application/json" \
    -d '{"body":"/chaos"}'
  The report is posted back on the issue. Fix what it names, then run it again.
  ```

### Chaos passed → `nudge_create_pr()`

- Guards: the `nudge_create_pr` flag must not already be set
- Sets `nudge_create_pr = true`
- Sends the agent a prompt with the curl command to create a PR and instructions to
  comment the PR number on the issue

## One-Shot Enforcement

The `nudge_create_pr` flag prevents the monitor from sending the same nudge on
every health tick. Once set, the flag persists in state until the member is
reclaimed for a new issue (via `reset_for_issue()`), which clears all nudge flags.

## Chaos-Aware Ordering

The nudge logic is ordered so that chaos-suite nudges take priority over PR-creation
nudges. This avoids ping-pong: if the monitor told the agent to create a PR before the
chaos gate was green, the chaos enforcer would immediately block that PR with a
`REQUEST_CHANGES` review, leaving the agent caught between contradictory instructions
from two parts of its own harness.
