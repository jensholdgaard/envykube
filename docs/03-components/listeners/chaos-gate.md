# Chaos Gate

The execution side of the chaos gate at
`crates/pool-manager/src/listeners/chaos_gate.rs`. Agent-triggered,
operator-executed — the agent asks for a run by commenting `/chaos`; the operator
shells out to `just chaos-suite`, posts the report **as admin**, and sets the verdict
label. The agent can read every word of the verdict but cannot forge, weaken, or skip
one.

## Trigger

The listener subscribes to `IssueComment` events and parses the body with
`parse_command()`:

- Must start with **exactly** `/chaos` at the beginning of a line
- Optionally followed by whitespace-separated scenario names (e.g.
  `/chaos reapply-twice oom-squeeze`)
- Trailing text after a newline is ignored (not treated as scenario names)
- `/chaosbeep` and prose like "running /chaos now" are **not** matched

### Unit tests (from source)

```rust
#[test]
fn recognises_the_command_and_its_arguments() {
    assert_eq!(parse_command("/chaos"), Some(vec![]));
    assert_eq!(
        parse_command("/chaos reapply-twice oom-squeeze"),
        Some(vec!["reapply-twice".into(), "oom-squeeze".into()])
    );
    assert_eq!(parse_command("/chaos\nplease run this"), Some(vec![]));
}

#[test]
fn ignores_everything_else() {
    assert_eq!(parse_command("running /chaos now"), None);
    assert_eq!(parse_command("/chaosbeep"), None);
    assert_eq!(parse_command("the chaos suite failed"), None);
}
```

## Guards

Before executing, three guards are checked:

1. **No claimed member** — if `state.find_by_issue()` returns `None`, posts "no agent
   is currently working this issue" and ignores the command
2. **Already running** — if `chaos_status == ChaosStatus::Running`, the duplicate
   `/chaos` is ignored (logged at info level)
3. **Run cap reached** — if `chaos_runs >= MAX_CHAOS_RUNS` (5), escalates to a human
   with a comment explaining the cap has been reached and further `/chaos` requests are
   ignored

## Execution

1. Sets `chaos_status = Running`, increments `chaos_runs`, saves state
2. Posts "Running the chaos suite against `{name}`" as admin
3. Shells out to `just chaos-suite <name> [scenarios...]` with a **1800-second
   timeout**
4. Parses the stdout as JSON and extracts the `markdown` field as the report
5. Posts the report as an **admin** comment (never via `post_comment_as_agent` — the
   verdict must not be attributable to, or forgeable by, the agent being judged)

## Verdict Handling

### Pass (exit code 0)

- Sets the `chaos-passed` label, removes `chaos-failed`
- Records `ChaosStatus::Passed` in state
- Sends the agent a prompt: "The chaos suite PASSED. Proceed to open the pull request
  now (step 5 of your instructions)…"

### Fail (exit code 1)

- Sets the `chaos-failed` label, removes `chaos-passed`
- Records `ChaosStatus::Failed` in state
- Extracts per-scenario failure notes via `summarise_failures()` — iterates the
  report's `scenarios` array, collecting `name` and `failure.why` for each scenario
  with `result == "FAIL"`
- Sends the agent a prompt with the named failures and instructions to fix, push, and
  re-run: "If two attempts change nothing, change the approach rather than the details.
  After 5 failed runs the gate stops and asks for a human."

### Harness Error (exit code 2 or crash/timeout)

- Records `ChaosStatus::NotRun` (does NOT label the work as failed)
- Refunds the run count via `saturating_sub(1)` — a broken harness is not the agent's
  fault
- Posts "could not run" with the stderr truncated to 1500 characters
- Does not send a prompt to the agent — there is nothing for the agent to fix
