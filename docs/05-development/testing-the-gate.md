# Testing the Chaos Gate

The chaos gate is an end-to-end flow: issue → implementation → fault suite → PR. This
guide walks through testing it from scratch.

## 1. Create an Issue

Create a Forgejo issue on the repo with the `ready` label. The reconciler detects it and
publishes `IssueLabeled { label: "ready" }`. The issue claimer picks it up, reserves an
idle pool member, and sends the agent its initial prompt.

## 2. Agent Implements

The agent clones the repo, checks out `issue-{n}`, and implements the fix. It writes a
`.chaos.yaml` at the repo root describing how the work is deployed, readied, and
verified. Example:

```yaml
namespace: default
deploy:  "kubectl apply -k ./deploy"
ready:   "kubectl rollout status deploy/<name> --timeout=120s"
verify:  "curl -fsS http://<name>.$VCLUSTER_NAME.localhost/healthz"
selector: "app=<name>"
```

The agent commits and pushes to branch `issue-{n}`.

## 3. Trigger the Suite

The agent comments `/chaos` on the issue:

```bash
curl -s -X POST "http://forgejo:3000/api/v1/repos/<repo>/issues/<n>/comments" \
  -u "$FORGEJO_USER:$FORGEJO_PASSWORD" -H "Content-Type: application/json" \
  -d '{"body":"/chaos"}'
```

The `chaos_gate` listener matches the comment, runs `just chaos-suite <name>`, and posts
the report back as a comment (as the operator, not the agent).

## 4. Check the Verdict

The gate sets issue labels based on the result:

- **`chaos-passed`**: All scenarios passed. The agent is prompted to open a PR.
- **`chaos-failed`**: At least one scenario failed. The agent gets a summary of what
  failed and is directed to fix the cause, push, and re-run `/chaos`.

After 5 failed runs, the gate posts a stopped message and ignores further `/chaos`
requests — a human must intervene.

## 5. Chaos Enforcer: PR Blocking

The `chaos_enforcer` listener acts on two events:

- **`PullRequestOpened`**: If the linked issue does not carry `chaos-passed`, the
  enforcer posts a `REQUEST_CHANGES` review blocking the PR. The review body tells the
  agent to run `/chaos`.
- **`PullRequestSynchronized`** (new commits pushed): If the issue carries
  `chaos-passed`, the enforcer removes the label and posts a comment explaining the pass
  no longer applies. The suite must run again.

## 6. After a Pass

With `chaos-passed` on the issue, the enforcer allows the PR through. The `pr_monitor`
listener watches for approvals and merges when conditions are met.

## 7. Push Invalidation

New commits pushed to an open PR clear `chaos-passed`. This prevents the agent from
passing the suite on a stub, then pushing the real implementation afterwards. The agent
must re-run `/chaos` after every push.

## Debugging

- Check Forgejo issue labels via `GET /repos/{repo}/issues/{n}` — labels are the
  authoritative state
- The chaos suite report is posted as a comment on the issue — every step, timing, and
  failure detail is included
- Exit codes: `0` = pass, `1` = code failure, `2` = harness failure (not the agent's
  fault, run count not incremented)
