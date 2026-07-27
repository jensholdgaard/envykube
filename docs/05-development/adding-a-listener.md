# Adding a Listener

Each listener is an async task spawned by `spawn_all()` in
`crates/pool-manager/src/listeners/mod.rs`. This guide walks through adding a new one.

## 1. Create the Listener File

Create a new file in `crates/pool-manager/src/listeners/`, e.g. `my_listener.rs`.

## 2. Implement the Run Function

Every listener follows the same pattern. Start with a function signature that takes an
event receiver, shared state, and config:

```rust
use crate::config::Config;
use crate::event::Event;
use super::{EventRx, SharedState};
use std::sync::Arc;
use tracing::{info, warn};

pub async fn run(mut events: EventRx, state: SharedState, cfg: Arc<Config>) {
    loop {
        let event = match events.recv().await {
            Ok(e) => e,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                warn!(skipped = n, "my_listener fell behind");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };

        match event {
            // Handle the events you care about
            Event::Shutdown => break,
            _ => {}
        }
    }
}
```

The critical patterns in this loop:

- **`RecvError::Lagged(n)`**: Log a warning with `skipped = n` and `continue`. The
  listener missed `n` events because it wasn't consuming fast enough. It must not crash
  or retry — catching up from an unknown position would duplicate work. The reconciler
  re-publishes key events on every tick, so missed events are eventually recovered.
- **`RecvError::Closed`**: The channel sender was dropped. `break` out of the loop.
- **`Event::Shutdown`**: The process is shutting down. `break` out of the loop.

## 3. Register the Module

Add the module declaration in `crates/pool-manager/src/listeners/mod.rs`:

```rust
mod my_listener;
```

## 4. Spawn the Listener

Add a `tokio::spawn` call inside `spawn_all()` in `mod.rs`:

```rust
pub fn spawn_all(bus: &Bus, state: SharedState, cfg: &Config) -> Vec<JoinHandle<()>> {
    let cfg = Arc::new(cfg.clone());
    vec![
        // ... existing spawns ...
        tokio::spawn(my_listener::run(bus.subscribe(), state.clone(), cfg.clone())),
    ]
}
```

Each listener receives its own `Receiver` via `bus.subscribe()`. The broadcast channel
is fan-out, so every subscriber sees every event.

## 5. Compile

```bash
cargo check
```

Ensure no compilation errors before proceeding.

## 6. Update Documentation

- Add a new listener doc file in `docs/03-components/listeners/`
- Update `docs/03-components/listeners.md` with a link to the new page
- Add an entry to `docs/SUMMARY.md` under the Components → Listeners section

## Shared Helpers

Four utility functions in `mod.rs` are available to multiple listeners:

| Function | Purpose |
|----------|---------|
| `truncate(s, max)` | Caps a string to `max` chars |
| `escape_quotes(s)` | Removes all `"` characters |
| `extract_issue_ref(text)` | Parses `#NNN` from text, returns `Option<IssueNumber>` |
| `find_related_pr(forgejo, issue_num)` | Searches open PRs for one referencing the given issue |

## Best Practices

- Use the `Forgejo` client (`crate::forgejo::Forgejo::new(&cfg)`) for API calls
- Use the `Agent` client (`crate::agent::Agent::new()`) for sending prompts
- Read/write state through `state.read().await` / `state.write().await`
- Post operator-side comments via `forgejo.post_issue_comment()`, not
  `post_comment_as_agent()` — the operator identity is not the agent's
