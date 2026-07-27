# Chaos Enforcer

The blocking side of the chaos gate at
`crates/pool-manager/src/listeners/chaos_enforcer.rs`. Where the chaos gate executes
the suite, the enforcer ensures its verdict is respected — even if the agent does not
cooperate.

## Events Watched

The enforcer listens for two event types:

- `PullRequestOpened` — a new PR was detected
- `PullRequestSynchronized` — new commits were pushed to an open PR

## Push Invalidation

When `PullRequestSynchronized` is received (a push), the enforcer calls `invalidate()`:

1. Checks whether the issue carries the `chaos-passed` label
2. If yes: removes the label, resets `ChaosStatus::NotRun` on the member, and posts a
   comment: "New commits were pushed, so the chaos pass no longer applies to this tree.
   Comment `/chaos` to re-run the suite before this can go to review."
3. If no: nothing to invalidate, returns early

This prevents an agent from passing the suite on a stub commit and then pushing the real
implementation — the tested tree must be the proposed tree.

## Blocking

After handling invalidation, the enforcer checks the `chaos-passed` label on the
issue:

- **Label present** → "chaos gate green — PR may proceed to review" → no block
- **Label missing** → `block()` is called

The `block()` function:

1. Posts a `REQUEST_CHANGES` review on the PR as the operator (admin):
   - "This PR cannot go to review yet — issue #N does not carry `chaos-passed`."
   - Prompts the agent to comment `/chaos` on the issue
2. Sends the agent a prompt via OpenCode: "Your PR for issue #N was opened before the
   chaos suite passed, so it is blocked…"

## Three-Layer Enforcement

The chaos gate's enforcement has three layers, progressively harder for an agent to
route around:

| Layer | Mechanism | Agent Can Bypass? |
|-------|-----------|-------------------|
| 1 | Prompt instruction — "do NOT open a PR before the gate passes" | Yes — model may ignore |
| 2 | `REQUEST_CHANGES` review posted by the enforcer | Yes — agent could dismiss its own review with `DISMISS` via the API |
| 3 | Forgejo branch protection on `main` with "block merge on rejected reviews" | **No** — enforced server-side on merge, the agent merges with its own credentials |

Layer 3 is the only layer the agent genuinely cannot route around. The enforcer
implements layer 2; layer 3 must be configured as a branch protection rule in Forgejo.
