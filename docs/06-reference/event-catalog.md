# Event Catalog

Every `Event` variant from `crates/pool-manager/src/event.rs`.

## Issue / PR Types

`IssueNumber` and `PrNumber` are distinct newtypes at the type level — the compiler
rejects passing one where the other is expected.

## Events

| Event | Fields | Source | Consumers |
|---|---|---|---|
| `IssueOpened` | `number: IssueNumber`, `title: String` | relay / reconciler | — |
| `IssueLabeled` | `number: IssueNumber`, `label: String` | relay / reconciler | `issue_claimer` (matches `ready`), `reconciler` |
| `IssueUnlabeled` | `number: IssueNumber`, `label: String` | relay / reconciler | — |
| `IssueClosed` | `number: IssueNumber` | relay / reconciler | `closed_reaper` |
| `IssueComment` | `issue_number: IssueNumber`, `comment_id: u64`, `user: String`, `body: String` | reconciler | `comment_router`, `chaos_gate` |
| `PrComment` | `pr_number: PrNumber`, `comment_id: u64`, `user: String`, `body: String` | reconciler | `pr_comment_router` |
| `PullRequestOpened` | `pr_number: PrNumber`, `title: String`, `head_branch: String` | reconciler | `chaos_enforcer` |
| `PullRequestReview` | `pr_number: PrNumber`, `state: String`, `body: String` | reconciler | `pr_monitor` |
| `PullRequestSynchronized` | `pr_number: PrNumber` | reconciler | `chaos_enforcer` (clears `chaos-passed`) |
| `PullRequestMerged` | `pr_number: PrNumber` | reconciler | — |
| `MemberProvisioned` | `name: String`, `index: u32` | `pool_refiller` | — |
| `MemberReady` | `name: String`, `index: u32` | `pool_refiller` | — |
| `MemberDestroyed` | `name: String` | `closed_reaper` | — |
| `MemberClaimed` | `name: String`, `index: u32`, `issue: IssueNumber` | `issue_claimer` | — |
| `AgentPromptSent` | `name: String`, `issue: IssueNumber` | (reserved) | — |
| `PoolNeedsRefill` | `current: usize`, `target: usize` | `issue_claimer`, `reconciler`, `closed_reaper` | `pool_refiller` |
| `ReconciliationTick` | — | `reconciler` | `health_monitor` |
| `Shutdown` | — | main (on SIGINT) | all listeners |

## Flow Notes

- Issue/PR comment events come from the **reconciler** (periodic poll), not the webhook
  relay — the relay publishes `IssueOpened`, `IssueLabeled`, etc. but not comments.
- `PoolNeedsRefill` is published by multiple consumers when the idle pool is below
  `min_available`. The `pool_refiller` provisions new members in response.
- `Shutdown` is published once from `main()` on SIGINT. Every listener breaks its loop
  on this event, and the broadcast channel is dropped, producing `RecvError::Closed` for
  any remaining receivers.
