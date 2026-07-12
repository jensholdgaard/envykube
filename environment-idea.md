Autonomous Agent Development Environment

This document outlines the architectural blueprint for a secure, stateless, and ephemeral development environment for AI coding agents (using OpenCode). The setup strictly separates the agent's execution sandbox from its validation cluster using DevContainers, Rootless Podman, and vCluster (VinD).

1. Core Technology Stack

Agent Harness: OpenCode (running sub-agents via AGENTS.md and SKILL.md).

Execution Sandbox: DevContainers (chrooted to isolated Git worktrees).

Validation Sandbox: vCluster in Docker/Podman (VinD).

Host OS: Bare-metal or VM compute pool running Rootless Podman (no host Kubernetes cluster required).

Routing & Ingress: Kubernetes Gateway API (replacing deprecated Ingress).

2. Architectural Topology

The environment relies on strict boundaries to ensure a zero-blast-radius execution loop.

The Host Machine (Stateless)

The host machine requires no local Kubernetes clusters (like KinD or k3s) and no global Docker daemon. Its only requirements are:

Rootless Podman: To run the agent's DevContainer and the agent-provisioned VinD clusters.

Sysctl Configuration: Running sysctl -w net.ipv4.ip_unprivileged_port_start=80 to allow rootless Podman to bind HTTP/HTTPS ports for local subdomain routing.

The Execution Sandbox (DevContainer)

The OpenCode agent runs entirely inside a DevContainer.

Strict Mounting: The container is restricted via workspaceMount to a specific Git worktree. It cannot traverse to parent directories or access the host filesystem.

Tooling: Contains the opencode CLI, vcluster CLI, kubectl, and helm.

No DinD: Avoids the compute overhead and security risks of Docker-in-Docker. It simply communicates outward to the Podman network.

The Validation Sandbox (VinD)

The agent autonomously provisions its own virtual cluster using the vCluster Docker driver (vcluster create ... --driver docker). This runs as a sibling container on the Podman host, entirely isolated from the agent's workspace.

3. Network & Routing Strategy (Gateway API)

Because VinD runs natively on Podman, we bypass host-level ingress controllers. The agent installs and configures the Gateway API inside its ephemeral cluster to handle L7 traffic and map custom subdomains (e.g., *.cluster-alpha.localhost) to local workloads.

Routing Flow:

DNS: Host OS resolves *.localhost to 127.0.0.1.

Podman Port Binding: The Gateway Controller (e.g., Envoy) provisions a LoadBalancer service. VinD detects this and instructs Podman to bind port 80/443 on the host to the virtual cluster.

HTTPRoute: Traffic to grafana.cluster-alpha.localhost hits the host, passes through Podman to the VinD container, and is routed by the Gateway Controller to the specific pod.

4. The Agent Lifecycle Workflow

The OpenCode agent is given autonomy to execute the following Plan-Execute-Validate loop:

Step 1: Provisioning

The agent spins up an ephemeral cluster using the Docker driver and exposes it locally.

vcluster create cluster-alpha --driver docker --expose-local


The agent automatically receives a scoped kubeconfig pointing directly to this isolated Podman container.

Step 2: Gateway API Bootstrapping

The agent establishes the routing plane by installing the experimental Gateway API CRDs and a Gateway Controller (e.g., Envoy Gateway).

kubectl apply -k "github.com/kubernetes-sigs/gateway-api/config/crd/experimental?ref=v1.1.0"
helm install eg oci://docker.io/envoyproxy/gateway-helm --version v1.1.0 -n envoy-gateway-system --create-namespace


Step 3: Configuring the Gateway Listener

The agent applies a Gateway manifest to bind the HTTP port, enabling external traffic to enter the cluster.

apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: agent-gateway
  namespace: default
spec:
  gatewayClassName: envoy
  listeners:
  - name: http
    protocol: HTTP
    port: 80
    allowedRoutes:
      namespaces:
        from: All


Step 4: Deploying & Validating Workloads

The agent builds its application, applies the standard manifests, and attaches an HTTPRoute for testing.

apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: test-app-route
spec:
  parentRefs:
  - name: agent-gateway
  hostnames:
  - "app.cluster-alpha.localhost"
  rules:
  - matches:
    - path:
        type: PathPrefix
        value: /
    backendRefs:
    - name: test-app-service
      port: 80


The agent can now validate its implementation via internal integration tests or standard curl commands against the configured local hostname.

Step 5: Teardown

Once the task is validated or discarded, the agent destroys the environment.

vcluster delete cluster-alpha


All CRDs, workloads, and Podman containers are instantly purged, leaving zero drift on the host system.

5. Security and Isolation Summary

Zero Blast Radius: Misconfigured manifests or destructive kubectl commands only affect the ephemeral SQLite datastore of the VinD instance.

CRD Isolation: Each agent can install conflicting CRDs across different clusters simultaneously without cross-contamination.

Least Privilege: The agent never touches the host OS, lacks host root access, and requires no permanent infrastructure.
