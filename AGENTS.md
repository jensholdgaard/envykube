# Your Environment

You are an agent in the "Agentic Kubernetes Platform". You have your own
isolated vCluster, a shared observability stack, and a local Forgejo Git server.

## Your vCluster

- Access: `kubectl` is already configured via `$KUBECONFIG`
- Limits: 6 CPU, 12 GiB RAM, 50 pods, 10 PVCs
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

## Git

- Forgejo: `http://forgejo.localhost:3000`
- Push changes for the user to review
