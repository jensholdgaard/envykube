# Issue Claimer

The claimer at `crates/pool-manager/src/listeners/issue_claimer.rs` reserves idle pool
members for ready-labeled issues and sends the initial prompt that kicks off the agent's
six-step workflow.

## Watch

The claimer listens for `Event::IssueLabeled` events where `label == cfg.labels.ready`.
All other events (including `Shutdown`, which breaks the loop) are ignored.

## Atomic Reservation

On a match, `reserve_idle()` performs an atomic write under `blocking_write()`:

1. Finds the first member with status `Idle`
2. Sets status to `Claiming`
3. Calls `member.reset_for_issue(number)` — clears all nudge flags, watermarks, and
   chaos verdicts from any previous card
4. Saves state to disk

If no idle member is available, the claimer posts "No idle agent available" on the
issue and publishes `Event::PoolNeedsRefill` so the pool refiller wakes up.

## Workspace & Identity

`claim_workspace_and_identity()` sets up the agent's environment:

1. Calls `vcluster::claim_member()` — shells out to `just workspace`, which launches
   the DevContainer and clones the repo
2. Calls `forgejo.create_agent_user()` — creates a Forgejo user (`pool-{index}`) with
   write access to the repo and an API token
3. Calls `forgejo.write_agent_creds()` — writes `FORGEJO_USER=pool-{index}` and
   `FORGEJO_PASSWORD=pool-agent-{index}-pass` to `/workspace/.forgejo`, plus a
   `.gitignore` to prevent committing dev-environment files

On success, the member is promoted from `Claiming` to `Claimed`.

## Agent Post

After successfully claiming, the claimer announces the assignment as the agent identity
(not the admin), posting: "Issue #N claimed by pool-N. Workspace ready, agent starting
work."

## Initial Prompt

The `send_initial_prompt()` function constructs and delivers the full six-step workflow
to the agent's OpenCode session. The prompt structure:

```
You are pool-{index}, assigned to issue #{n} in {repo}: "{title}".

Issue description:
{body}  (truncated to 2000 chars)

Steps — complete each step before moving on:
1. Source your credentials: source /workspace/.forgejo
2. Create a branch: cd /workspace; git checkout -b issue-{n}
3. Implement the fix or feature described in the issue.
   Commit and push to branch issue-{n}
4. PASS THE CHAOS GATE — do this BEFORE creating a PR.
   You trigger it, not a human.
   a. Make sure /workspace/.chaos.yaml exists and describes how
      YOUR work is deployed, readied and verified
   b. Trigger the suite by commenting /chaos on issue #{n}:
      curl -s -X POST "http://forgejo:3000/api/v1/repos/{repo}/issues/{n}/comments" \
        -u "$FORGEJO_USER:$FORGEJO_PASSWORD" -H "Content-Type: application/json" \
        -d '{"body":"/chaos"}'
   c. The operator runs the suite and posts the report.
      If FAIL: fix the cause, push, re-run (step 4b).
      If PASS (after at most 5 runs): proceed to step 5.
5. ONLY after the chaos suite PASSES, create a PR via curl.
   A PR opened before the gate is green is blocked.
6. Post a comment on the issue with the PR link.
```

The `.chaos.yaml` template embedded in the prompt includes fields: `namespace`,
`deploy`, `ready`, `verify`, `selector`, and an optional `dependency` block with
`name`, `selector`, and `port`.
