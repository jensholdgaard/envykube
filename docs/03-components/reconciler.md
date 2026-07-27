# Reconciler

The polling engine at `crates/pool-manager/src/listeners/reconciler.rs`. Unlike the
other listeners — which receive events from the broadcast bus — the reconciler runs on
its own timer and publishes into the bus.

## Tick Loop

Every `pollInterval` seconds (default 10), the reconciler fires `ReconciliationTick`
and performs five operations in sequence:

1. **Log stats** — idle count, claimed count, total active, and configured `minAvailable`
2. **Sanitise vanished vClusters** — detects vCluster namespaces that no longer exist
   and removes those members from state, cleaning up stale Forgejo users and local
   vcluster state
3. **Refill pool** — provisions new members up to `minAvailable` if the count has
   dropped (same provision path as `pool_refiller`)
4. **Re-publish ready issues** — fetches all open issues with the `ready` label from
   Forgejo and publishes `IssueLabeled` events for each. This ensures that the claimer
   sees work even if the relabel event was missed (e.g. after a restart)
5. **Poll conversations** — for each claimed member, fetches new comments, detects new
   PRs, pushes to open PRs, and new reviews by comparing watermarks

## Conversation Polling

Because no Forgejo webhook is registered on the repo — Forgejo sits on the k3d docker
network with no stable route back to the host relay — the reconciler polls as the
primary data source. The webhook relay at `:30990` remains as an optional latency
accelerator.

### Watermarks

Four watermark fields on each `Member` prevent replays:

| Watermark | Type | What it tracks |
|-----------|------|----------------|
| `last_comment_seen` | `HashMap<String, u64>` | Highest comment ID seen per issue (`key = "c-{issue}"`) |
| `seen_pr` | `Option<u64>` | Whether the PR for this issue has been announced |
| `last_review_seen` | `u64` | Highest review ID already turned into an event |
| `pr_head_sha` | `Option<String>` | HEAD SHA of the open PR; a change means new commits were pushed |

### Watermark Ordering

Watermarks are advanced **before** events are published. This prevents a slow or
panicking listener from causing the same comment or review to be replayed on every
subsequent tick:

```rust
// Advance the watermark BEFORE publishing
{
    let mut st = state.write().await;
    if let Some(m) = st.find_member_mut(name) {
        m.last_comment_seen.insert(key, highest);
    }
}

// Only then publish — a listener panic cannot cause a replay loop
for c in fresh {
    bus.publish(Event::IssueComment { ... });
}
```

### Events Published

The reconciler publishes these event types from polled data:

- `IssueComment` — new comments on claimed issues
- `PullRequestOpened` — a PR was detected for a claimed issue (announced once per member, tracked via `seen_pr`)
- `PullRequestSynchronized` — the PR's head SHA changed (new commits were pushed; tracked via `pr_head_sha`)
- `PullRequestReview` — new APPROVED or REQUEST_CHANGES review on the PR (PENDING/COMMENT are filtered out)
- `IssueLabeled` — issues with the `ready` label found during the ready-issue scan

## Startup Recovery

Before entering the tick loop, the reconciler runs `recover_stale_state()`. It inspects
every member whose status is `Provisioning` or `Claiming`:

- **vCluster exists** → recover as `Idle` (a crash during provision/claim left a usable
  vCluster behind)
- **vCluster gone** → remove from state and clean up (the cluster was torn down while
  pool-manager was stopped)

This prevents "already exists" errors on re-provision after a nuke+bootstrap cycle.
