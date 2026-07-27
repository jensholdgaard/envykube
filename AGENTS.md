# Your Environment

You are an agent in the "Agentic Kubernetes Platform". You have your own
isolated vCluster, a shared observability stack, and a local Forgejo Git server.

## Your vCluster

- Access: `kubectl` is already configured via `$KUBECONFIG`
- Budget: `requests.cpu 4`, `requests.memory 8Gi`, `limits.memory 24Gi`, 50 pods, 10 PVCs
- No CPU-limit ceiling — don't hand-write CPU limits to fit the budget. Memory is the one
  to watch: every container without an explicit memory limit is charged 512Mi, initContainers
  and sidecars included.
- Your hostname prefix: `*.${VCLUSTER_NAME}.localhost`

## Exposing services — THE ONLY WAY

The user cannot reach `localhost` in your container. To expose an app:

```bash
kubectl create deployment X --image=Y
kubectl expose deployment X --port=80
kubectl create ingress X --rule="X.my-agent-2.localhost/*=X:80"
```

That is it. The app is now at `http://X.my-agent-2.localhost` in any browser.

## What NOT to do

- Do NOT use `kubectl port-forward` (user cannot reach it)
- Do NOT install ingress controllers (already handled)
- Do NOT expose on `localhost:8080` (user will never see it)
- Do NOT use `--type=LoadBalancer` (use ClusterIP + Ingress)

## Observability

- Grafana: `http://grafana.platform.localhost`
- Send OTLP: `otel-collector.observability.svc.cluster.local:4317`
- MCP servers: Grafana (query telemetry) + Kubernetes (cluster ops)

## Chaos gate — required before review

Your work is tested under deliberate faults before it can go up for review: the same deploy
run twice, pods killed mid-apply, the API rejecting a write with 409/500/429, the dependency
scaled away underneath you. Code that only works on the happy path does not get through.

**This is an automatic step in your workflow — you trigger it, not the human.** After you
implement and push your code, immediately create `.chaos.yaml` and comment `/chaos` on the
issue. Do not wait for someone else to do it. Do not skip ahead to creating a PR.

### The chaos-passes-then-PR loop

```
implement → push → .chaos.yaml → /chaos → wait for result ──FAIL──> fix → push → /chaos ...
                                                           │
                                                          PASS → open PR
```

1. **Declare your work.** Commit a `.chaos.yaml` at the repo root with at minimum:
   ```yaml
   namespace: default
   deploy:  "kubectl apply -k ./deploy"          # must be safe to run twice
   ready:   "kubectl rollout status deploy/<name> --timeout=120s"
   verify:  "curl -fsS http://<name>.$VCLUSTER_NAME.localhost/healthz"
   selector: "app=<name>"
   dependency:                                    # only if your app has one
     name: postgres
     selector: "app=postgres"
     port: 5432
   ```
2. **Trigger the suite.** Comment `/chaos` on the issue (you trigger, operator executes):
   ```bash
   curl -s -X POST "http://forgejo:3000/api/v1/repos/<user>/<repo>/issues/<issue>/comments" \
     -u "$FORGEJO_USER:$FORGEJO_PASSWORD" -H "Content-Type: application/json" \
     -d '{"body":"/chaos"}'
   ```
   Your credentials are at `/workspace/.forgejo` (source them first).
3. **Read the report.** The operator posts it back as a comment and prompts you with the
   result. Each failure names a behaviour, not an exit code — you know what to fix.
4. **Iterate.** Fix what it names, push to your branch, comment `/chaos` again. After 5
   failed runs the gate stops and asks for a human, so if two attempts change nothing,
   change the approach.
5. **Open the PR.** Only when it says PASS. A PR opened before the gate is green is blocked
   with a `REQUEST_CHANGES` review, and pushing new commits after a pass invalidates it.

Details and the full scenario catalogue are in the platform skill — load it with
`/skill agentic-k8s-platform`.

## Git

- Forgejo: `http://forgejo.platform.localhost`
- Push changes for the user to review

## Documentation

Full platform documentation is at `docs/index.md`. To view it:

```bash
just docs-serve    # builds + opens http://localhost:3000 with live reload
just docs          # build only (output: docs/book/)
```

When you modify code, check whether the matching doc file in `docs/` needs updating.
Read `docs/05-development/conventions.md` for the conventions.
