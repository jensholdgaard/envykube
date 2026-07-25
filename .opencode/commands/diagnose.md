Diagnose the Kubernetes resource: $ARGUMENTS

Run the triage playbook from the agentic-k8s-platform skill against this resource:

1. `kubectl get` and `kubectl describe` it, and read the Events.
2. If it is a workload, check pod phase, restart count, and `kubectl logs --previous`
   on any crashed container; check the ResourceQuota if it is Pending.
3. Follow the chain to its backing Service `endpoints` and Ingress if it exposes traffic.
4. If the workload is up, correlate errors with the Grafana MCP (logs / metrics / traces).

Report the root cause with the specific command output that proves it, then the minimal
fix. Do not apply changes unless I ask.

Usage: /diagnose <kind>/<name>   (e.g. /diagnose deploy/my-app)
