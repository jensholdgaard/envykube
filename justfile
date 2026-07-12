# Agentic Kubernetes Platform — routine operations
# Usage: `just <recipe>`   •   `just` on its own lists everything.
# See PLATFORM-SETUP.md for the full design.

set shell := ["bash", "-euo", "pipefail", "-c"]

# ── Config (override on the CLI, e.g. `just np_base=31000 provision foo 5`) ──
cluster     := "agent-platform"
context     := "k3d-agent-platform"
kubeconfigs := "$HOME/.local/share/agent-kubeconfigs"      # where per-agent static kubeconfigs land
opencodes   := "$HOME/.local/share/agent-opencode"         # where per-agent opencode.json land
obs_ns      := "observability"
np_base     := "30000"                        # agent API NodePort = np_base + index
np_max      := "30050"                         # end of the pre-published range
op_base     := "31000"                         # OpenChamber web UI port = op_base + index
grafana_url := "http://grafana.platform.localhost"
forgejo_url := "http://forgejo.localhost:3000"
workspaces  := "$HOME/.local/share/agent-workspaces"  # per-agent git workspaces

# Show all recipes
default:
    @just --list

# ── Host setup ──────────────────────────────────────────────────────────────

# Verify the toolchain is installed
versions:
    @docker --version && kubectl version --client && helm version --short \
      && k3d --version && vcluster --version && devcontainer --version \
      && echo -n "cilium " && cilium version --client 2>/dev/null || echo "cilium CLI: missing"

# Create the shared host cluster — RECOMMENDED DEFAULT (flannel CNI + Traefik ingress)
bootstrap:
    k3d cluster create {{cluster}} \
      --servers 1 --agents 2 \
      --servers-memory 4g --agents-memory 12g \
      --port "80:80@loadbalancer" \
      --port "443:443@loadbalancer" \
      --port "{{np_base}}-{{np_max}}:{{np_base}}-{{np_max}}@server:0"
    @echo "Host cluster up. Context: {{context}}."
    @echo "Next: 'just harden' → 'just observability' → 'just provision <name> <index>'"

# ── Optional: Cilium dataplane (not recommended — see improvements.md) ──

# EXPERIMENTAL: Cilium CNI + eBPF kube-proxy replacement + Gateway API + Hubble (keeps Traefik for ingress)
bootstrap-cilium gwapi_version="v1.5.1":
    #!/usr/bin/env bash
    set -euo pipefail
    sudo mount bpffs -t bpf /sys/fs/bpf 2>/dev/null || true
    k3d cluster create {{cluster}} \
      --servers 1 --agents 2 \
      --k3s-arg "--flannel-backend=none@server:*" \
      --k3s-arg "--disable-network-policy@server:*" \
      --port "80:80@loadbalancer" \
      --port "443:443@loadbalancer" \
      --port "{{np_base}}-{{np_max}}:{{np_base}}-{{np_max}}@server:0"

    echo "→ Installing Gateway API {{gwapi_version}} CRDs..."
    kubectl --context {{context}} apply -f \
      "https://github.com/kubernetes-sigs/gateway-api/releases/download/{{gwapi_version}}/standard-install.yaml"

    # Cilium 1.19 requires TLSRoute v1alpha2 (served=false in standard installs since v1.5.1)
    echo "→ Patching TLSRoute CRD to serve v1alpha2 (Cilium 1.19 compatibility)..."
    kubectl --context {{context}} patch crd tlsroutes.gateway.networking.k8s.io --type=json \
      -p='[{"op":"replace","path":"/spec/versions/1/served","value":true}]'

    echo "→ Installing Cilium with kube-proxy replacement + Gateway API + Hubble..."
    helm repo add cilium https://helm.cilium.io >/dev/null 2>&1 || true
    helm repo update >/dev/null
    helm install cilium cilium/cilium -n kube-system \
      --set kubeProxyReplacement=true \
      --set k8sServiceHost=k3d-{{cluster}}-server-0 --set k8sServicePort=6443 \
      --set gatewayAPI.enabled=true \
      --set envoy.securityContext.capabilities.keepCapNetBindService=true \
      --set hubble.relay.enabled=true --set hubble.ui.enabled=true \
      --set hubble.metrics.enableOpenMetrics=true \
      --set 'hubble.metrics.enabled={dns,drop,tcp,flow,icmp,httpV2}'

    echo "→ Waiting for Cilium agents..."
    until kubectl --context {{context}} -n kube-system get pods -l app.kubernetes.io/name=cilium-agent \
      --field-selector=status.phase=Running 2>/dev/null | grep -q '1/1'; do sleep 5; done

    # k3d's auto-deploy jobs conflict with pre-existing Gateway CRDs; install Traefik manually
    echo "→ Installing Traefik ingress controller..."
    kubectl --context {{context}} -n kube-system delete job helm-install-traefik helm-install-traefik-crd --ignore-not-found || true
    helm repo add traefik https://traefik.github.io/charts >/dev/null 2>&1 || true
    helm repo update traefik >/dev/null
    helm install traefik traefik/traefik -n kube-system \
      --set ingressClass.enabled=true --set ingressClass.isDefaultClass=true \
      --set dashboard.enabled=true \
      --skip-crds

    echo "→ Creating Traefik dashboard route..."
    kubectl --context {{context}} apply -f - <<'EOF'
    apiVersion: traefik.io/v1alpha1
    kind: IngressRoute
    metadata: { name: traefik-dashboard, namespace: kube-system }
    spec:
      entryPoints: [web]
      routes:
        - match: Host(`traefik.localhost`)
          kind: Rule
          services:
            - name: traefik
              port: 9000
    EOF

    echo ""
    echo "✓ Cluster ready:  CNI=Cilium  Ingress=Traefik  GatewayAPI=Cilium  Hubble=enabled"
    echo "  Dashboard:  http://traefik.localhost"

# Kernel-enforced host ceiling: systemd slice on the Docker daemon (sudo; recreate cluster after)
harden mem="48G" cpu="1400":
    #!/usr/bin/env bash
    set -euo pipefail
    sudo mkdir -p /etc/systemd/system/docker.service.d
    printf '[Service]\nMemoryMax=%s\nMemorySwapMax=%s\nCPUQuota=%s%%\n' \
      "{{mem}}" "{{mem}}" "{{cpu}}" | sudo tee /etc/systemd/system/docker.service.d/resource-cap.conf
    sudo systemctl daemon-reload && sudo systemctl restart docker
    echo "Docker capped at {{mem}} / {{cpu}}% CPU. Re-run 'just bootstrap' to recreate the cluster."

# ── Local Git server (Forgejo) — private repo hosting for agent workspaces ──

# Start Forgejo (local Git server) — agents clone from here, push changes for CI/review
forgejo:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p "$HOME/.local/share/forgejo/data"
    if docker ps -a --filter name=forgejo --format 'found' | grep -q found; then
      docker start forgejo 2>/dev/null || true
      echo "Forgejo running → {{forgejo_url}}"
      exit 0
    fi
    docker run -d --name forgejo \
      --network k3d-{{cluster}} \
      -p 3000:3000 -p 2222:22 \
      -v "$HOME/.local/share/forgejo/data:/data" \
      -e FORGEJO__server__DOMAIN="forgejo.localhost" \
      -e FORGEJO__server__SSH_DOMAIN="forgejo.localhost" \
      -e FORGEJO__server__ROOT_URL="{{forgejo_url}}" \
      codeberg.org/forgejo/forgejo:9
    echo "Forgejo starting → {{forgejo_url}}"
    echo "  First visit: open {{forgejo_url}} to create admin account"
    echo "  Run 'just forgejo' again to skip setup (container already exists)"

# Deploy the shared LGTM + Alloy observability stack and expose Grafana via ingress
#   just observability                 → Prometheus (lightweight, single binary)
#   just observability mimir=distributed  → Mimir distributed (production-grade, 25+ pods)
observability mimir="single":
    #!/usr/bin/env bash
    set -euo pipefail
    helm repo add grafana https://grafana.github.io/helm-charts >/dev/null 2>&1 || true
    helm repo update >/dev/null
    kubectl --context {{context}} create namespace {{obs_ns}} --dry-run=client -o yaml | kubectl --context {{context}} apply -f -
    helm --kube-context {{context}} upgrade --install loki grafana/loki -n {{obs_ns}} \
      --set deploymentMode=SingleBinary --set singleBinary.replicas=1 \
      --set backend.replicas=0 --set read.replicas=0 --set write.replicas=0 \
      --set loki.storage.type=filesystem --set loki.commonConfig.replication_factor=1 \
      --set loki.useTestSchema=true --set 'loki.auth_enabled=false'
    helm --kube-context {{context}} upgrade --install tempo grafana/tempo -n {{obs_ns}}

    if [ "{{mimir}}" = "distributed" ]; then
      echo "→ Deploying Mimir distributed (production-grade, 25+ pods)..."
      helm --kube-context {{context}} upgrade --install mimir grafana/mimir-distributed -n {{obs_ns}}
      METRICS_SERVICE="mimir-gateway.{{obs_ns}}.svc.cluster.local:80/prometheus"
    else
      echo "→ Deploying Prometheus (single binary)..."
      helm repo add prometheus-community https://prometheus-community.github.io/helm-charts >/dev/null 2>&1 || true
      helm repo update prometheus-community >/dev/null
      helm --kube-context {{context}} upgrade --install prometheus prometheus-community/prometheus -n {{obs_ns}} \
        --set alertmanager.enabled=false --set kube-state-metrics.enabled=false \
        --set prometheus-node-exporter.enabled=false --set prometheus-pushgateway.enabled=false \
        --set server.global.scrape_interval=15s
      METRICS_SERVICE="prometheus-server.{{obs_ns}}.svc.cluster.local:80"
    fi

    helm --kube-context {{context}} upgrade --install grafana grafana/grafana -n {{obs_ns}}

    # Grafana ingress — apply before k8s-monitoring so it survives recipe failures
    kubectl --context {{context}} apply -f - <<'EOF'
    apiVersion: networking.k8s.io/v1
    kind: Ingress
    metadata: { name: grafana, namespace: observability }
    spec:
      rules:
        - host: grafana.platform.localhost
          http:
            paths:
              - path: /
                pathType: Prefix
                backend: { service: { name: grafana, port: { number: 80 } } }
    EOF

    # OTel Collector — relays agent telemetry to Tempo/Prometheus/Loki
    echo "→ Deploying OTel Collector..."
    kubectl --context {{context}} apply -f manifests/otel-collector.yaml

    # k8s-monitoring (Alloy) — requires k8s-monitoring-values.yaml with collectors config
    if [ -f k8s-monitoring-values.yaml ]; then
      helm --kube-context {{context}} upgrade --install k8smon grafana/k8s-monitoring -n {{obs_ns}} \
        --set "cluster.name={{cluster}}" --values k8s-monitoring-values.yaml
    else
      echo "NOTE: k8s-monitoring-values.yaml not found — skipping Alloy collector deployment."
      echo "      Create one to enable automated metrics/logs collection."
    fi

    echo "Grafana → {{grafana_url}}  (admin pw: kubectl -n {{obs_ns}} get secret grafana -o jsonpath='{.data.admin-password}' | base64 -d)"
    echo "Metrics at: http://$METRICS_SERVICE"
    echo "OTel Collector → port 30417 (gRPC) on host — agents send telemetry to host.docker.internal:30417"

# ── Per-agent lifecycle ─────────────────────────────────────────────────────

# Provision an agent: vCluster + ResourceQuota + pinned NodePort + static kubeconfig (e.g. just provision cluster-alpha 1)
provision name index:
    #!/usr/bin/env bash
    set -euo pipefail
    ns="vc-{{name}}"
    port=$(( {{np_base}} + {{index}} ))
    if [ "$port" -gt "{{np_max}}" ]; then
      echo "ERROR: NodePort $port exceeds the published range end {{np_max}}. Widen the range in 'bootstrap'." >&2
      exit 1
    fi
    echo "→ Provisioning {{name}} (ns=$ns, API NodePort=$port)"

    vcluster use driver helm
    kubectl config use-context {{context}}

    tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
    cat >"$tmp/vcluster.yaml" <<EOF
    sync:
      toHost:
        ingresses:
          enabled: true
    controlPlane:
      statefulSet:
        resources:
          requests: { cpu: 200m, memory: 256Mi }
          limits: { cpu: "2", memory: 2Gi }
      proxy:
        extraSANs:
          - host.docker.internal
          - 127.0.0.1
    EOF

    vcluster create {{name}} --namespace "$ns" --create-namespace \
      --values "$tmp/vcluster.yaml" --connect=false

    kubectl --context {{context}} apply -f - <<EOF
    apiVersion: v1
    kind: ResourceQuota
    metadata: { name: agent-quota, namespace: $ns }
    spec:
      hard:
        requests.cpu: "4"
        requests.memory: 8Gi
        limits.cpu: "6"
        limits.memory: 12Gi
        pods: "50"
        persistentvolumeclaims: "10"
    ---
    apiVersion: v1
    kind: LimitRange
    metadata: { name: agent-defaults, namespace: $ns }
    spec:
      limits:
        - type: Container
          default: { cpu: 500m, memory: 512Mi }
          defaultRequest: { cpu: 100m, memory: 128Mi }
    EOF

    # Pin the vCluster API service to the deterministic NodePort
    kubectl --context {{context}} -n "$ns" patch svc {{name}} --type merge \
      -p "{\"spec\":{\"type\":\"NodePort\",\"ports\":[{\"port\":443,\"targetPort\":8443,\"nodePort\":${port},\"protocol\":\"TCP\"}]}}"

    # Export a STATIC kubeconfig (CA embedded, no live proxy process)
    mkdir -p {{kubeconfigs}}
    vcluster connect {{name}} -n "$ns" --print \
      --server "https://host.docker.internal:${port}" > "{{kubeconfigs}}/{{name}}.yaml"

    echo "✓ {{name}} ready. Kubeconfig: {{kubeconfigs}}/{{name}}.yaml"
    echo "  Next: 'just workspace {{name}} <index> <forgejo-repo-url>' to launch the DevContainer."

# Clone repo from Forgejo and launch a DevContainer for an agent (e.g. just workspace agent-alpha 1 http://forgejo.localhost:3000/user/repo.git)
workspace name index repo_url:
    #!/usr/bin/env bash
    set -euo pipefail
    ws="{{workspaces}}/{{name}}"
    kc="{{kubeconfigs}}/{{name}}.yaml"
    oc="{{opencodes}}/{{name}}.json"

    if [ ! -f "$kc" ]; then
      echo "ERROR: kubeconfig not found at $kc — run 'just provision {{name}} <index>' first." >&2
      exit 1
    fi

    echo "→ Cloning workspace from {{repo_url}}..."
    rm -rf "$ws"
    git clone "{{repo_url}}" "$ws"

    ui_port=$(( {{op_base}} + {{index}} ))

    # Copy DevContainer config, agent skills, commands, and instructions into the workspace
    mkdir -p "$ws/.devcontainer" "$ws/.opencode/skills/agentic-k8s-platform" "$ws/.opencode/commands"
    cp .devcontainer/Dockerfile .devcontainer/devcontainer.json "$ws/.devcontainer/"
    cp .devcontainer/start-openchamber.sh "$ws/.devcontainer/"
    cp .opencode/skills/agentic-k8s-platform/SKILL.md "$ws/.opencode/skills/agentic-k8s-platform/"
    cp .opencode/commands/*.md "$ws/.opencode/commands/" 2>/dev/null || true
    cp AGENTS.md "$ws/" 2>/dev/null || true

    # Patch devcontainer.json with per-agent OpenChamber port mapping
    sed -i "s|\"--add-host=forgejo.localhost:host-gateway\"|\"-p\",\n      \"${ui_port}:3000\",\n      \"--add-host=forgejo.localhost:host-gateway\"|" "$ws/.devcontainer/devcontainer.json"

    # Generate per-agent opencode config
    mkdir -p {{opencodes}}
    python3 -c 'import json; cfg={"plugin":[["@devtheops/opencode-plugin-otel",{"enabled":True,"endpoint":"http://host.docker.internal:30417","protocol":"grpc","resourceAttributes":"deployment.environment=agentic-k8s-platform"}],"oc-mnemoria"],"mcp":{"grafana":{"type":"local","command":["mcp-grafana","--disable-write"],"environment":{"GRAFANA_URL":"http://grafana.platform.localhost"}},"k8s":{"type":"local","command":["kubectl-mcp-server"],"environment":{"KUBECONFIG":"/home/agent/.kube/config"}}}}; open("'"$oc"'","w").write(json.dumps(cfg,indent=2)+"\n")'

    echo "→ Building DevContainer image..."
    devcontainer build --workspace-folder "$ws"

    echo "→ Launching DevContainer (cd to workspace to satisfy CWD check)..."
    cd "$ws"
    KUBECONFIG_PATH="$kc" OPENCODE_CONFIG_PATH="$oc" \
      devcontainer up --workspace-folder "$ws"

    echo "→ Starting OpenChamber web UI..."
    container_id=$(docker ps -q --filter "label=devcontainer.local_folder=$(readlink -f "$ws")" | head -1)
    if [ -n "$container_id" ]; then
      docker exec "$container_id" start-openchamber || true
    fi

    echo ""
    echo "✓ Workspace ready at $ws"
    echo ""
    echo "  OpenChamber:  http://localhost:$ui_port  (web UI — chat, diff, git, terminal)"
    echo "  OpenCode CLI: devcontainer exec --workspace-folder $ws opencode"
    echo "  Shell:        docker exec -it \$(docker ps -q --filter label=devcontainer.local_folder=$ws | head -1) bash"

# Open the OpenChamber web UI for an agent (or restart it if needed)
ui name index="0":
    #!/usr/bin/env bash
    set -euo pipefail
    ws="{{workspaces}}/{{name}}"
    ui_port=$(( {{op_base}} + {{index}} ))
    container_id=$(docker ps -q --filter "label=devcontainer.local_folder=$(readlink -f "$ws")" 2>/dev/null | head -1)
    if [ -z "$container_id" ]; then
      echo "ERROR: DevContainer not running for {{name}}. Run 'just workspace {{name}} {{index}} <repo-url>' first." >&2
      exit 1
    fi
    docker exec "$container_id" start-openchamber 2>/dev/null || true
    echo "OpenChamber → http://localhost:$ui_port"
    -@xdg-open "http://localhost:$ui_port" 2>/dev/null || true

# Default-deny cross-agent network traffic (works with any CNI — recommended for default bootstrap)
network-policy name:
    #!/usr/bin/env bash
    set -euo pipefail
    kubectl --context {{context}} apply -f - <<EOF
    apiVersion: networking.k8s.io/v1
    kind: NetworkPolicy
    metadata: { name: isolate-agent, namespace: vc-{{name}} }
    spec:
      podSelector: {}
      policyTypes: [Ingress, Egress]
      ingress:
        - from:
            - namespaceSelector:
                matchLabels:
                  kubernetes.io/metadata.name: kube-system
            - podSelector: {}
      egress:
        - to:
            - namespaceSelector:
                matchLabels:
                  kubernetes.io/metadata.name: kube-system
            - podSelector: {}
        - to:
            - namespaceSelector:
                matchLabels:
                  kubernetes.io/metadata.name: kube-system
              podSelector:
                matchLabels:
                  k8s-app: kube-dns
          ports:
            - protocol: UDP
              port: 53
            - protocol: TCP
              port: 53
    EOF
    echo "✓ NetworkPolicy applied to vc-{{name}}."

# Default-deny cross-agent network traffic for an agent (requires the Cilium dataplane — use network-policy instead)
netpol name:
    #!/usr/bin/env bash
    set -euo pipefail
    kubectl --context {{context}} apply -f - <<EOF
    apiVersion: cilium.io/v2
    kind: CiliumNetworkPolicy
    metadata: { name: isolate-agent, namespace: vc-{{name}} }
    spec:
      endpointSelector: {}
      ingress:
        - fromEndpoints:
            - {}
        - fromEndpoints:
            - matchLabels:
                "k8s:io.kubernetes.pod.namespace": kube-system
      egress:
        - toEndpoints:
            - {}
        - toEndpoints:
            - matchLabels:
                "k8s:io.kubernetes.pod.namespace": kube-system
        - toEntities:
            - world
    EOF

# Re-export an agent's static kubeconfig (reads the pinned NodePort back from the svc)
kubeconfig name:
    #!/usr/bin/env bash
    set -euo pipefail
    ns="vc-{{name}}"
    port=$(kubectl --context {{context}} -n "$ns" get svc {{name}} -o jsonpath='{.spec.ports[0].nodePort}')
    mkdir -p {{kubeconfigs}}
    vcluster connect {{name}} -n "$ns" --print \
      --server "https://host.docker.internal:${port}" > "{{kubeconfigs}}/{{name}}.yaml"
    echo "✓ {{kubeconfigs}}/{{name}}.yaml (NodePort $port)"

# Tear down an agent (vCluster + namespace + kubeconfig + opencode config)
deprovision name:
    -vcluster delete {{name}} -n vc-{{name}}
    -kubectl --context {{context}} delete ns vc-{{name}} --ignore-not-found
    -rm -rf {{workspaces}}/{{name}} {{kubeconfigs}}/{{name}}.yaml {{opencodes}}/{{name}}.json
    @echo "✓ {{name}} torn down."

# ── Inspect / operate ───────────────────────────────────────────────────────

# List live vClusters and their namespaces
list:
    @vcluster list || true
    @echo "---"
    @kubectl --context {{context}} get ns -l vcluster.loft.sh/managed-by --no-headers 2>/dev/null || \
      kubectl --context {{context}} get ns | grep '^vc-' || echo "no agent namespaces"

# Host cluster health
status:
    @k3d node list
    @echo "---"
    @kubectl --context {{context}} get nodes -o wide

# Open Grafana in a browser
grafana:
    @echo "{{grafana_url}}"
    -@xdg-open "{{grafana_url}}" 2>/dev/null

# Open the Hubble network-flow UI (Cilium dataplane only; needs the cilium CLI)
hubble:
    cilium hubble ui

# Reclaim disk from dangling images/volumes
prune:
    docker system df
    docker system prune -f

# Delete the entire host cluster (agents, observability, everything)
nuke:
    k3d cluster delete {{cluster}}
    @echo "✓ Host cluster deleted. Static kubeconfigs remain under {{kubeconfigs}}."
