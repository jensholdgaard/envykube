# Chaos Scenarios

All 10 scenarios in `chaos/scenarios/`. Every claim is validated against the actual
YAML files.

## Default Suite (6)

These run on every `/chaos` invocation.

### `reapply-twice`
- **Teaches**: idempotency
- **Summary**: Deploy twice, assert-same. The cheapest and highest-signal check.
- **Catches**: Schema created without IF NOT EXISTS, seed rows inserted without ON
  CONFLICT, Jobs with fixed names.

### `restart-cold`
- **Teaches**: invariant-recovery
- **Summary**: Kill all app pods, assert-recovered, then verify state unchanged.
- **Catches**: Startup path that only works against an empty dependency (CREATE TABLE
  without IF NOT EXISTS, "initialise then insert", init step assuming empty database).

### `kill-mid-apply`
- **Teaches**: idempotency
- **Summary**: Kill pods mid-rollout, then re-apply. Tests the half-applied state.
- **Catches**: Recovery that needs a human to clean up a half-built state instead of
  just re-running the apply.

### `write-conflict`
- **Teaches**: idempotency
- **Summary**: Arm a 409 Conflict on the next UPDATE, then re-deploy. Scales app to 0
  first so the apply generates a real write.
- **Catches**: Deploy that clobbers a concurrent change (force-write) or gives up on
  first error.

### `api-flaky`
- **Teaches**: foreseeing-errors
- **Summary**: Arm a 500 InternalError on the first write, then a 429
  TooManyRequests on a later write. Re-deploy after each arm.
- **Catches**: Deploy script that aborts on a single transient error rather than
  retrying.

### `dependency-down`
- **Teaches**: foreseeing-errors
- **Requires**: dependency
- **Summary**: Scale dependency to 0, verify fails, scale back, assert-recovered.
- **Catches**: Process that exits when its dependency disappears; health check that
  returns 200 without touching the dependency.

## Extended Suite (4)

These require opt-in via `just chaos-suite <name> --suite extended` or by naming
specific scenarios.

### `oom-squeeze`
- **Teaches**: invariant-recovery
- **Summary**: Drop memory limit to 16Mi, wait 30s for OOMKill, restore limit,
  assert-recovered, verify state unchanged.
- **Catches**: State held only in memory, crash loop that is terminal rather than
  survivable.

### `stuck-not-ready`
- **Teaches**: invariant-recovery
- **Requires**: dependency
- **Summary**: Stall dependency pods (swap to pause image — present, scheduled, never
  Ready, no endpoints), verify fails, unstall, assert-recovered.
- **Catches**: "Running" mistaken for "Ready" — health check that looks at pod
  existence rather than endpoint reachability.

### `partition`
- **Teaches**: invariant-recovery
- **Requires**: dependency
- **Summary**: NetworkPolicy cuts egress from app to dependency for 45 seconds (DNS
  only allowed), assert-recovered.
- **Catches**: Missing connection timeouts — connections hang instead of being refused,
  process waits forever.

### `dependency-slow`
- **Teaches**: foreseeing-errors
- **Requires**: dependency + proxy (dependency.proxy: true)
- **Summary**: Toxiproxy injects 2s latency with 250ms jitter to the dependency,
  verify still succeeds, remove toxic, assert-recovered.
- **Catches**: Missing timeout/probe budgets that assume the happy path. Requires the
  app to address its dependency through `toxiproxy.chaos-system.svc.cluster.local:21000`.
