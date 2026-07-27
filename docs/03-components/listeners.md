# Listeners

Each listener is an async task spawned by `spawn_all()` in
`crates/pool-manager/src/listeners/mod.rs`. The reconciler — which publishes rather
than subscribes — is spawned separately via `spawn_reconciler()`.

## Spawning

```rust
pub fn spawn_all(bus: &Bus, state: SharedState, cfg: &Config) -> Vec<JoinHandle<()>> {
    let cfg = Arc::new(cfg.clone());
    vec![
        tokio::spawn(pool_refiller::run(bus.subscribe(), state.clone(), cfg.clone())),
        tokio::spawn(issue_claimer::run(bus.clone(), bus.subscribe(), state.clone(), cfg.clone())),
        tokio::spawn(closed_reaper::run(bus.clone(), bus.subscribe(), state.clone(), cfg.clone())),
        tokio::spawn(comment_router::run(bus.subscribe(), state.clone(), cfg.clone())),
        tokio::spawn(pr_comment_router::run(bus.subscribe(), state.clone(), cfg.clone())),
        tokio::spawn(pr_monitor::run(bus.subscribe(), state.clone(), cfg.clone())),
        tokio::spawn(health_monitor::run(bus.subscribe(), state.clone(), cfg.clone())),
        tokio::spawn(chaos_gate::run(bus.subscribe(), state.clone(), cfg.clone())),
        tokio::spawn(chaos_enforcer::run(bus.subscribe(), state.clone(), cfg.clone())),
    ]
}
```

Each listener receives its own `Receiver` via `bus.subscribe()` — the broadcast channel
is fan-out, so every subscriber sees every event.

## Lag Handling

Every listener handles `RecvError::Lagged(n)` with a warning:

```rust
Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
    warn!(skipped = n, "listener_name fell behind");
    continue;
}
```

A lag of `n` means the listener missed `n` events because it wasn't consuming fast
enough. The warning logs the count and the listener continues — it does not crash or
retry, because catching up from an unknown position would duplicate work. The reconciler
re-publishes key events on every tick, so missed events are eventually recovered.

## Shared Helpers

Four utility functions in `mod.rs` are used across multiple listeners:

| Function | Purpose |
|----------|---------|
| `truncate(s, max)` | Returns `&s[..max]` if `s.len() > max`, otherwise `s`. Used to cap issue body length in prompts. |
| `escape_quotes(s)` | Removes all double-quote characters from a string. Used when embedding issue titles in JSON curl commands. |
| `extract_issue_ref(text)` | Parses the first `#NNN` token from text, returning `Some(IssueNumber)`. Splits on whitespace, strips trailing non-digit chars. |
| `find_related_pr(forgejo, issue_num)` | Searches open PRs for one whose title or body contains `#NNN` for the given issue. Returns `Option<PrNumber>`. |

## Detailed Pages

- [Claimer](listeners/issue-claimer.md) — reserves idle agents for ready-labeled issues
- [Chaos Gate](listeners/chaos-gate.md) — executes the fault suite on `/chaos`
- [Chaos Enforcer](listeners/chaos-enforcer.md) — blocks PRs without a chaos pass
- [Health Monitor](listeners/health-monitor.md) — checks agent liveness and nudges stalled agents
