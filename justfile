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
oc_base     := "32000"                         # opencode serve API port = oc_base + index
grafana_url := "http://grafana.platform.localhost"
forgejo_url := "http://forgejo.platform.localhost"
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

# Host-survival ceiling: caps EVERYTHING Docker runs (k3d + all agents combined) so the host always survives.
# NOTE: this is NOT the per-agent fairness mechanism — agents-can't-starve-each-other is enforced by the
# per-namespace ResourceQuota/LimitRange in `provision`. This only bounds the whole stack against the host.
harden mem="48G" cpu="1400":
    #!/usr/bin/env bash
    set -euo pipefail
    # Drop the legacy drop-in from older versions of this recipe (it capped dockerd, not the containers).
    sudo rm -f /etc/systemd/system/docker.service.d/resource-cap.conf
    slice="k3dcap.slice"
    # 1) A dedicated slice carrying the limits. MemorySwapMax=0 makes MemoryMax a HARD ceiling
    #    (no swap escape): under memory pressure workloads are killed, the host is not swapped to death.
    printf '[Slice]\nMemoryMax=%s\nMemorySwapMax=0\nCPUQuota=%s%%\n' \
      "{{mem}}" "{{cpu}}" | sudo tee "/etc/systemd/system/${slice}" >/dev/null
    # 2) Place every Docker container under that slice — without clobbering an existing daemon.json.
    if [ -f /etc/docker/daemon.json ] && ! grep -q '"cgroup-parent"' /etc/docker/daemon.json; then
      echo "ERROR: /etc/docker/daemon.json exists without a cgroup-parent key." >&2
      echo "       Add   \"cgroup-parent\": \"${slice}\"   to it, then re-run 'just harden'." >&2
      exit 1
    fi
    [ -f /etc/docker/daemon.json ] || printf '{ "cgroup-parent": "%s" }\n' "$slice" | sudo tee /etc/docker/daemon.json >/dev/null
    sudo systemctl daemon-reload
    # 3) Never nuke a live cluster: only bounce dockerd when no cluster is running. Running containers
    #    keep their current cgroup until the next `just bootstrap` recreates them under the slice.
    if k3d cluster list 2>/dev/null | grep -q '{{cluster}}'; then
      echo "⚠  '{{cluster}}' is running — the cap applies to containers created after the next 'just bootstrap'."
    else
      sudo systemctl restart docker
      echo "✓ Docker restarted; all containers now run under ${slice}."
    fi
    echo "Ceiling set: mem={{mem}} cpu={{cpu}}%  ·  verify with: systemctl show ${slice} -p MemoryMax -p CPUQuota"
    echo "Per-agent fairness is enforced separately by the ResourceQuota in 'provision'."

# ── Local Git server (Forgejo) — private repo hosting for agent workspaces ──

# Start Forgejo (local Git server) — agents clone from here, push changes for CI/review
forgejo:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p "$HOME/.local/share/forgejo/data"
    if docker ps -a --filter name=forgejo --format 'found' | grep -q found; then
      docker start forgejo 2>/dev/null || true
    else
      # Note: no -p 3000:3000 — HTTP is served through Traefik (below). Only git-ssh is published.
      docker run -d --name forgejo \
        --network k3d-{{cluster}} \
        -p 2222:22 \
        -v "$HOME/.local/share/forgejo/data:/data" \
        -e FORGEJO__server__DOMAIN="forgejo.platform.localhost" \
        -e FORGEJO__server__SSH_DOMAIN="forgejo.platform.localhost" \
        -e FORGEJO__server__ROOT_URL="{{forgejo_url}}" \
        codeberg.org/forgejo/forgejo:9
    fi
    # Route the out-of-cluster Forgejo container through host Traefik so it joins the Host-based
    # scheme at forgejo.platform.localhost:80 — discoverable via `kubectl get ingress`, and no longer
    # a host-agnostic :3000 catch-all. A selector-less Service + manual Endpoints point at the
    # container's IP on the k3d network; re-run `just forgejo` if the container is recreated.
    fip="$(docker inspect forgejo | python3 -c 'import json,sys; print(json.load(sys.stdin)[0]["NetworkSettings"]["Networks"]["k3d-{{cluster}}"]["IPAddress"])')"
    kubectl --context {{context}} apply -f - <<EOF
    apiVersion: v1
    kind: Namespace
    metadata: { name: platform }
    ---
    apiVersion: v1
    kind: Service
    metadata: { name: forgejo, namespace: platform }
    spec:
      ports:
        - { name: http, port: 3000, targetPort: 3000 }
    ---
    apiVersion: v1
    kind: Endpoints
    metadata: { name: forgejo, namespace: platform }
    subsets:
      - addresses:
          - { ip: ${fip} }
        ports:
          - { name: http, port: 3000 }
    ---
    apiVersion: networking.k8s.io/v1
    kind: Ingress
    metadata: { name: forgejo, namespace: platform }
    spec:
      rules:
        - host: forgejo.platform.localhost
          http:
            paths:
              - path: /
                pathType: Prefix
                backend: { service: { name: forgejo, port: { number: 3000 } } }
    EOF
    echo "Forgejo → {{forgejo_url}}   (git ssh: localhost:2222)"
    echo "  First visit: open {{forgejo_url}} to create the admin account"

# Deploy the shared LGTM observability stack (Loki + Tempo + Mimir monolithic + Grafana)
# and expose Grafana via ingress. Metrics store = Mimir monolithic — one deployment, no pod sprawl.
observability:
    #!/usr/bin/env bash
    set -euo pipefail
    helm repo add grafana https://grafana.github.io/helm-charts >/dev/null 2>&1 || true
    helm repo update >/dev/null
    kubectl --context {{context}} create namespace {{obs_ns}} --dry-run=client -o yaml | kubectl --context {{context}} apply -f -

    # Logs (Loki, single binary) + Traces (Tempo)
    helm --kube-context {{context}} upgrade --install loki grafana/loki -n {{obs_ns}} \
      --set deploymentMode=SingleBinary --set singleBinary.replicas=1 \
      --set backend.replicas=0 --set read.replicas=0 --set write.replicas=0 \
      --set loki.storage.type=filesystem --set loki.commonConfig.replication_factor=1 \
      --set loki.useTestSchema=true --set 'loki.auth_enabled=false'
    helm --kube-context {{context}} upgrade --install tempo grafana/tempo -n {{obs_ns}}

    # Metrics store — Mimir monolithic (single deployment, filesystem storage, multitenancy off)
    echo "→ Deploying Mimir (monolithic)..."
    kubectl --context {{context}} apply -f manifests/mimir-monolithic.yaml

    # Grafana + datasource sidecar; datasources (Mimir/Tempo/Loki) provisioned from a labeled ConfigMap
    helm --kube-context {{context}} upgrade --install grafana grafana/grafana -n {{obs_ns}} \
      --set sidecar.datasources.enabled=true
    kubectl --context {{context}} apply -f manifests/grafana-datasources.yaml

    # Grafana ingress — applied before the collector so the UI survives if a later step fails
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

    # OTel Collector — relays agent app telemetry: traces→Tempo, metrics→Mimir, logs→Loki
    echo "→ Deploying OTel Collector..."
    kubectl --context {{context}} apply -f manifests/otel-collector.yaml

    # Mint the service-account token the agents' Grafana MCP authenticates with. Last step on
    # purpose: everything above is already deployed if this fails, and it fails loudly rather
    # than leaving an empty token for an agent to discover the hard way.
    just grafana-token

    echo "Grafana → {{grafana_url}}  (admin pw: kubectl -n {{obs_ns}} get secret grafana -o jsonpath='{.data.admin-password}' | base64 -d)"
    echo "Metrics store (Mimir) → http://mimir.{{obs_ns}}.svc.cluster.local:8080/prometheus"
    echo "OTel Collector → port 30417 (gRPC) on host — agents send telemetry to host.docker.internal:30417"
    echo ""
    echo "For automatic dashboard sync, run in a separate terminal:"
    echo "  just watch-dashboards"

# Idempotent: reuses the service account, replaces its tokens (Grafana reveals a key only once).
# Mint the Grafana service-account token that agents authenticate with, and write it into .env
grafana-token sa_name="agent-platform":
    #!/usr/bin/env bash
    set -euo pipefail
    echo "→ Provisioning Grafana service-account token ({{sa_name}}, role Editor)..."
    kubectl --context {{context}} -n {{obs_ns}} rollout status deploy/grafana --timeout=180s >/dev/null

    admin_user="$(kubectl --context {{context}} -n {{obs_ns}} get secret grafana -o jsonpath='{.data.admin-user}' | base64 -d)"
    admin_pw="$(kubectl --context {{context}} -n {{obs_ns}} get secret grafana -o jsonpath='{.data.admin-password}' | base64 -d)"
    auth="${admin_user}:${admin_pw}"

    # The API can lag the rollout by a few seconds.
    for i in $(seq 1 30); do
      [ "$(curl -s -o /dev/null -w '%{http_code}' -m 5 "{{grafana_url}}/api/health" || true)" = "200" ] && break
      [ "$i" -eq 30 ] && { echo "ERROR: Grafana API never became reachable at {{grafana_url}}." >&2; exit 1; }
      sleep 2
    done

    api() { curl -sS -m 15 -u "$auth" -H 'Content-Type: application/json' "$@"; }

    # Reuse the service account if it already exists — matching on exact name, not substring.
    sa_id="$(api "{{grafana_url}}/api/serviceaccounts/search?perpage=100" \
      | python3 -c 'import json,sys; print(next((str(a["id"]) for a in json.load(sys.stdin).get("serviceAccounts",[]) if a["name"]=="{{sa_name}}"), ""))')"

    if [ -z "$sa_id" ]; then
      sa_id="$(api -X POST -d '{"name":"{{sa_name}}","role":"Editor","isDisabled":false}' \
        "{{grafana_url}}/api/serviceaccounts" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
      echo "  created service account {{sa_name}} (id=$sa_id)"
    else
      # An existing SA may predate this recipe and carry the wrong role; dashboard writes need Editor.
      api -X PATCH -d '{"role":"Editor"}' "{{grafana_url}}/api/serviceaccounts/${sa_id}" >/dev/null
      echo "  reusing service account {{sa_name}} (id=$sa_id, role forced to Editor)"
    fi

    # Grafana returns a token key exactly once, so a pre-existing token is unusable to us —
    # revoke and re-mint rather than accumulating dead tokens on every run.
    api "{{grafana_url}}/api/serviceaccounts/${sa_id}/tokens" \
      | python3 -c 'import json,sys; [print(t["id"]) for t in json.load(sys.stdin)]' \
      | while read -r tid; do
          [ -n "$tid" ] && api -X DELETE "{{grafana_url}}/api/serviceaccounts/${sa_id}/tokens/${tid}" >/dev/null || true
        done

    token="$(api -X POST -d '{"name":"{{sa_name}}"}' "{{grafana_url}}/api/serviceaccounts/${sa_id}/tokens" \
      | python3 -c 'import json,sys; print(json.load(sys.stdin)["key"])')"
    [ -n "$token" ] || { echo "ERROR: Grafana returned an empty token." >&2; exit 1; }

    # Write into .env without disturbing the other keys (and never echo the value).
    [ -f .env ] || { [ -f .env.example ] && cp .env.example .env || : > .env; }
    TOKEN="$token" python3 - <<'PY'
    import os, pathlib
    p = pathlib.Path(".env")
    key, val = "GRAFANA_SERVICE_ACCOUNT_TOKEN", os.environ["TOKEN"]
    lines = p.read_text().splitlines() if p.exists() else []
    for i, line in enumerate(lines):
        if line.split("=", 1)[0].strip() == key:
            lines[i] = f"{key}={val}"
            break
    else:
        lines.append(f"{key}={val}")
    p.write_text("\n".join(lines) + "\n")
    PY
    chmod 600 .env
    echo "✓ Token written to .env as GRAFANA_SERVICE_ACCOUNT_TOKEN (${#token} chars). 'just workspace' picks it up."

# Sync agent-created dashboards from their vClusters to Grafana.
# Agents create ConfigMaps with the annotation platform.agentic-k8s.dev/dashboard="true"
# in their vCluster's default namespace; this recipe pulls them and pushes them to
# Grafana via the API using host-side admin credentials (never exposed to agents).
# Keeps the Grafana MCP server read-only — no Editor token needed inside the container.
# Run periodically or after agent push: just sync-dashboards
sync-dashboards:
    #!/usr/bin/env bash
    set -euo pipefail
    admin_pw="$(kubectl --context {{context}} -n {{obs_ns}} get secret grafana -o jsonpath='{.data.admin-password}' | base64 -d)"
    auth="admin:${admin_pw}"
    tmp_expected="$(mktemp)"

    for ns in $(kubectl --context {{context}} get ns -o name | grep '^namespace/vc-' | cut -d/ -f2); do
        name="${ns#vc-}"
        kc="{{kubeconfigs}}/$name.host.yaml"
        if [ -s "$kc" ]; then
            cmaps="$(timeout 15 kubectl --kubeconfig "$kc" get cm -o json 2>/dev/null)"
        else
            cmaps="$(timeout 60 vcluster connect "$name" -n "$ns" -- kubectl get cm -o json 2>/dev/null)"
        fi
        [ -n "$cmaps" ] || continue

        # Collect expected uids for this agent (one per line, prefixed by agent name)
        > "$tmp_expected"
        echo "$cmaps" | python3 scripts/sync-dashboards.py | while IFS= read -r payload; do
            title="$(echo "$payload" | python3 -c 'import json,sys; print(json.load(sys.stdin)["title"])')"
            uid="$(echo "$payload" | python3 -c 'import json,sys; print(json.load(sys.stdin)["uid"])')"
            echo "$uid" >> "$tmp_expected"
            j="$(echo "$payload" | python3 -c 'import json,sys; d=json.load(sys.stdin)["json"]; d["tags"]=d.get("tags",[])+["agent:'"$name"'","managed-by:agentic-platform"]; print(json.dumps(d))')"
            echo "  → $title ($ns)"
            curl -sS -m 10 -u "$auth" -X POST -H 'Content-Type: application/json' \
                -d "{\"dashboard\":$j,\"overwrite\":true}" \
                "{{grafana_url}}/api/dashboards/db" >/dev/null
        done

        # Delete stale Grafana dashboards: managed-by:agentic-platform + agent:<name>,
        # but uid not present in the current ConfigMap set from the vCluster.
        curl -sS -m 10 -u "$auth" \
            "{{grafana_url}}/api/search?tag=agent:${name}&tag=managed-by:agentic-platform" \
            | python3 -c 'import json,sys; print("\n".join(d["uid"] for d in json.load(sys.stdin)))' 2>/dev/null \
            | while IFS= read -r grafana_uid; do
                [ -n "$grafana_uid" ] || continue
                if ! grep -qxF "$grafana_uid" "$tmp_expected" 2>/dev/null; then
                    echo "  ✗ deleting stale dashboard $grafana_uid ($ns)"
                    curl -sS -m 10 -u "$auth" -X DELETE \
                        "{{grafana_url}}/api/dashboards/uid/$grafana_uid" >/dev/null
                fi
            done
    done
    rm -f "$tmp_expected"
    echo "✓ Dashboards synced."

# Continuous dashboard sync — runs sync-dashboards in a loop so agent-created
# dashboards appear in Grafana within seconds, no manual operator step needed.
# Start once: just watch-dashboards
# Stop:  pkill -f 'just watch-dashboards'
watch-dashboards interval_sec="10":
    #!/usr/bin/env bash
    set -euo pipefail
    echo "→ Watching for dashboard ConfigMaps every {{interval_sec}}s (Ctrl-C to stop)..."
    while true; do
        just sync-dashboards 2>/dev/null || true
        sleep {{interval_sec}}
    done

# opencode relay — aggregates all agent opencode API servers behind one port.
# Routes /agent/<name>/api/* to that agent's opencode serve. GET / lists all
# agents with session counts. Start once: just opencode-relay
# Stop:  pkill -f 'python3 scripts/oc-relay.py'
opencode-relay:
    #!/usr/bin/env bash
    set -euo pipefail
    export OC_BASE="{{oc_base}}"
    echo "→ Starting opencode relay on http://localhost:30999 ..."
    python3 scripts/oc-relay.py

# operator — run the operator opencode server (host-level permissions, custom tools).
# Start once: just operator
# The operator config lives in operator/opencode.jsonc with a custom-tool set that
# wraps every just recipe into structured JSON — no more scraping `just` output.
operator port="30998":
    #!/usr/bin/env bash
    set -euo pipefail
    set -a; [ -f .env ] && source .env; set +a
    echo "→ Starting operator opencode server on http://localhost:{{port}} ..."
    cd operator
    exec opencode serve --port {{port}} --hostname 0.0.0.0

# ── Per-agent lifecycle ─────────────────────────────────────────────────────

# Canonical names for the operator/board model: pool-provision = an idle pool member (card-agnostic);
# claim = bind a member to a card (clone repo + launch). Thin aliases so existing docs keep working.
alias pool-provision := provision
alias claim := workspace

# Safety gate for every recipe that interpolates an agent name into a path it then rm -rf's.
# `just deprovision ""` would otherwise expand to `rm -rf {{workspaces}}/` and wipe EVERY agent's
# workspace, not just one — and a name containing `/` or `..` could escape the root entirely.
# The pattern is a DNS label, which is what a vCluster name has to be anyway.
[private]
_check-name name:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! [[ "{{name}}" =~ ^[a-z0-9]([a-z0-9-]*[a-z0-9])?$ ]]; then
      echo "ERROR: invalid agent name '{{name}}'." >&2
      echo "       Must be a DNS label: ^[a-z0-9]([a-z0-9-]*[a-z0-9])?\$ — no empty string, no '/', no '..'." >&2
      exit 1
    fi

# Guard against an unset/degenerate $HOME turning the storage roots into '/' or ''.
[private]
_check-roots:
    #!/usr/bin/env bash
    set -euo pipefail
    for root in "{{workspaces}}" "{{kubeconfigs}}" "{{opencodes}}"; do
      case "$root" in
        "" | "/" | "$HOME" | "$HOME/" )
          echo "ERROR: unsafe storage root '$root' — is \$HOME set?" >&2; exit 1 ;;
      esac
    done

# Provision an agent: vCluster + ResourceQuota + pinned NodePort + static kubeconfig (e.g. just provision cluster-alpha 1)
provision name index: (_check-name name) _check-roots
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
    networking:
      # Host-cluster services the agent's workloads must reach, replicated into the vCluster as
      # real Services so the documented FQDN (otel-collector.{{obs_ns}}.svc.cluster.local) resolves
      # natively. Without this every agent has to hand-patch the vCluster CoreDNS ConfigMap — and
      # that patch dies with the vCluster. The chart auto-grants the ClusterRole rules this needs
      # (templates/clusterrole.yaml is gated on networking.replicateServices.fromHost).
      # NOTE: the target namespace must exist INSIDE the vCluster — created below, after connect.
      replicateServices:
        fromHost:
          - from: {{obs_ns}}/otel-collector
            to: {{obs_ns}}/otel-collector
      advanced:
        # Safety net for every other host service: names the vCluster can't resolve fall back to
        # host DNS (addressed as <service>.<namespace>). networking.resolveDNS would be the precise
        # tool but it requires embedded CoreDNS — a vCluster PRO feature, so not an option here.
        fallbackHostCluster: true
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

    # CPU is governed by REQUESTS, memory by both requests and limits. Deliberate:
    # a quota on limits.cpu forces every container to carry a CPU limit, which forces the
    # LimitRange to invent one — and that default, applied to every container INCLUDING
    # initContainers and sidecars, is what actually exhausts the budget. Measured on the
    # Astroshop run: requests.cpu 610m/4 while limits.cpu sat at 5050m/6. The workload was
    # nowhere near its real budget; the default manufactured 8x phantom demand.
    # Dropping limits.cpu means containers need no CPU limit at all — nothing to invent, and
    # no CFS throttling starving JVM startup. CPU is compressible; the absolute ceiling stays
    # with 'just harden' (CPUQuota across all of Docker) and fairness with requests.cpu.
    # Memory is NOT compressible, so it keeps a hard cap.
    kubectl --context {{context}} apply -f - <<EOF
    apiVersion: v1
    kind: ResourceQuota
    metadata: { name: agent-quota, namespace: $ns }
    spec:
      hard:
        requests.cpu: "4"
        requests.memory: 8Gi
        # 3x requests.memory, deliberately. Memory can't be dropped from the quota the way
        # limits.cpu was (it isn't compressible), so the LimitRange must keep a memory default
        # — which means the same "invented demand" arithmetic still applies: every container
        # without an explicit limit contributes 512Mi. A 20-pod/3-container app therefore books
        # ~20Gi of limits while genuinely reserving ~5Gi. Sizing this at the overcommit the
        # defaults produce, not at the real footprint, is what keeps ordinary charts installable.
        # The actual protections are requests.memory (what the scheduler reserves), node
        # capacity, and the 'just harden' ceiling — not this number.
        limits.memory: 24Gi
        pods: "50"
        persistentvolumeclaims: "10"
    ---
    apiVersion: v1
    kind: LimitRange
    metadata: { name: agent-defaults, namespace: $ns }
    spec:
      limits:
        - type: Container
          # No default cpu — see above. A memory default is required because limits.memory is quota'd.
          default: { memory: 512Mi }
          defaultRequest: { cpu: 50m, memory: 128Mi }
          # Deliberately NO max.cpu: the LimitRanger back-fills `default` from `max` for any
          # resource `default` omits, so a max.cpu of 2 silently reappears as a 2-CPU default
          # limit on every container — reintroducing exactly the invented limit this change
          # removes. Total CPU stays bounded by the quota's requests.cpu and by 'just harden'.
          max: { memory: 4Gi }
    EOF

    # Pin the vCluster API service to the deterministic NodePort
    kubectl --context {{context}} -n "$ns" patch svc {{name}} --type merge \
      -p "{\"spec\":{\"type\":\"NodePort\",\"ports\":[{\"port\":443,\"targetPort\":8443,\"nodePort\":${port},\"protocol\":\"TCP\"}]}}"

    # Export a STATIC kubeconfig (CA embedded, no live proxy process).
    # `vcluster connect --print` occasionally hangs (see improvements.md) — bound each attempt
    # with a timeout and retry with backoff so provisioning is deterministic, not flaky.
    mkdir -p {{kubeconfigs}}
    kc_connect() {
      local server="$1" out="$2" n=0
      until timeout 90 vcluster connect {{name}} -n "$ns" --print --server "$server" >"$out" 2>/dev/null && [ -s "$out" ]; do
        n=$((n + 1))
        if [ "$n" -ge 5 ]; then echo "ERROR: 'vcluster connect' failed for $server after 5 attempts." >&2; return 1; fi
        echo "  vcluster connect ($server) attempt $n failed; retrying in $((n * 5))s..." >&2
        sleep $((n * 5))
      done
    }
    # In-container clients use host.docker.internal (mapped via --add-host); host-side tools use
    # the 127.0.0.1 variant (both are in the cert SANs). The 127.0.0.1 path has shown intermittent
    # EOFs on some setups (improvements.md) — its failure here is non-fatal; verify on a live cluster.
    kc_connect "https://host.docker.internal:${port}" "{{kubeconfigs}}/{{name}}.yaml"
    kc_connect "https://127.0.0.1:${port}" "{{kubeconfigs}}/{{name}}.host.yaml" \
      || echo "  (host-side kubeconfig skipped — use 'vcluster connect {{name}} -n $ns' from the host)"

    # Run kubectl against the VIRTUAL cluster from the host. The primary kubeconfig targets
    # host.docker.internal, which only resolves inside containers — so prefer the 127.0.0.1
    # variant and fall back to a short-lived 'vcluster connect' proxy if it's missing/broken.
    vkubectl() {
      local hkc="{{kubeconfigs}}/{{name}}.host.yaml"
      if [ -s "$hkc" ] && timeout 20 kubectl --kubeconfig "$hkc" version >/dev/null 2>&1; then
        kubectl --kubeconfig "$hkc" "$@"
      else
        timeout 120 vcluster connect {{name}} -n "$ns" -- kubectl "$@"
      fi
    }

    # networking.replicateServices.fromHost only lands if the TARGET namespace exists inside the
    # vCluster. Create it so otel-collector.{{obs_ns}}.svc.cluster.local resolves exactly as documented.
    echo "→ Preparing in-vCluster '{{obs_ns}}' namespace for the replicated collector Service..."
    vkubectl get namespace {{obs_ns}} >/dev/null 2>&1 || vkubectl create namespace {{obs_ns}} >/dev/null

    # Confirm the replication actually took, rather than assuming it did.
    for i in $(seq 1 12); do
      if vkubectl -n {{obs_ns}} get svc otel-collector >/dev/null 2>&1; then
        echo "  ✓ otel-collector.{{obs_ns}}.svc.cluster.local resolvable inside the vCluster"
        break
      fi
      [ "$i" -eq 12 ] && echo "  ⚠ replicated otel-collector Service not present yet — check 'vcluster logs' if telemetry doesn't arrive" >&2
      sleep 5
    done

    echo "✓ {{name}} ready. Kubeconfig: {{kubeconfigs}}/{{name}}.yaml"
    echo "  Next: 'just workspace {{name}} <index> <forgejo-repo-url>' to launch the DevContainer."

# Clone repo from Forgejo and launch a DevContainer for an agent (e.g. just workspace agent-alpha 1 http://forgejo.platform.localhost/user/repo.git)
workspace name index repo_url: (_check-name name) _check-roots
    #!/usr/bin/env bash
    set -euo pipefail
    set -a; [ -f .env ] && source .env; set +a
    # Validated by _check-name — without it the `rm -rf "$ws"` below would, for an empty name,
    # delete the whole workspaces root and every other agent's clone with it.
    ws="{{workspaces}}/{{name}}"
    kc="{{kubeconfigs}}/{{name}}.yaml"
    oc="{{opencodes}}/{{name}}.json"

    if [ ! -f "$kc" ]; then
      echo "ERROR: kubeconfig not found at $kc — run 'just provision {{name}} <index>' first." >&2
      exit 1
    fi

    if [ -z "${DEEPSEEK_API_KEY:-}" ]; then
      echo "ERROR: DEEPSEEK_API_KEY not set. Create a .env file with DEEPSEEK_API_KEY=sk-..." >&2
      exit 1
    fi

    if [ -z "${GRAFANA_SERVICE_ACCOUNT_TOKEN:-}" ]; then
      echo "ERROR: GRAFANA_SERVICE_ACCOUNT_TOKEN not set — the agent's Grafana MCP would start" >&2
      echo "       unauthenticated and every query would 401. Run 'just grafana-token' first." >&2
      exit 1
    fi

    echo "→ Cloning workspace from {{repo_url}}..."
    if [ -d "$ws/.git" ]; then
        old="$ws.old.$$"
        mv "$ws" "$old" 2>/dev/null || true
        rm -rf "$old" 2>/dev/null || true
    fi
    git clone "{{repo_url}}" "$ws"
    # Make workspace writable by the container agent user (UID may not match host)
    chmod -R ugo+rwX "$ws"
    # Agent name file — used by the opencode relay for discovery
    echo "{{name}}" > "$ws/.agent-name"

    ui_port=$(( {{op_base}} + {{index}} ))
    oc_port=$(( {{oc_base}} + {{index}} ))

    # Copy DevContainer config, agent skills, commands, and instructions into the workspace
    mkdir -p "$ws/.devcontainer" "$ws/.opencode/skills/agentic-k8s-platform" "$ws/.opencode/commands" "$ws/.opencode/agents"
    cp .devcontainer/Dockerfile .devcontainer/devcontainer.json "$ws/.devcontainer/"
    cp .devcontainer/start-openchamber.sh .devcontainer/curl-shim.sh "$ws/.devcontainer/"
    cp .opencode/skills/agentic-k8s-platform/SKILL.md "$ws/.opencode/skills/agentic-k8s-platform/"
    cp .opencode/commands/*.md "$ws/.opencode/commands/" 2>/dev/null || true
    cp .opencode/agents/*.md "$ws/.opencode/agents/" 2>/dev/null || true
    cp AGENTS.md "$ws/" 2>/dev/null || true

    # Patch devcontainer.json with per-agent port mappings (OpenChamber UI + opencode API)
    sed -i "s|\"--add-host=forgejo.platform.localhost:host-gateway\"|\"-p\",\n      \"${oc_port}:4096\",\n      \"-p\",\n      \"${ui_port}:3000\",\n      \"--add-host=forgejo.platform.localhost:host-gateway\"|" "$ws/.devcontainer/devcontainer.json"

    # Generate per-agent opencode config (model overridable via AGENT_MODEL; provider + guardrails baked in)
    mkdir -p {{opencodes}}
    agent_model="${AGENT_MODEL:-deepseek/deepseek-v4-pro}"
    python3 -c 'import json; cfg={"$schema":"https://opencode.ai/config.json","model":"'"$agent_model"'","provider":{"deepseek":{"npm":"@ai-sdk/openai-compatible","name":"DeepSeek","options":{"baseURL":"https://api.deepseek.com","apiKey":"'"$DEEPSEEK_API_KEY"'"},"models":{"deepseek-v4-pro":{"name":"DeepSeek V4 Pro"}}}},"enabled_providers":["deepseek"],"permission":{"bash":{"kubectl port-forward *":"deny","kubectl expose *--type=LoadBalancer*":"deny","kubectl expose *--type LoadBalancer*":"deny","kubectl create service loadbalancer*":"deny","kubectl delete *--all*":"deny","kubectl delete svc kubernetes*":"deny","*":"allow"}},"plugin":[["@devtheops/opencode-plugin-otel",{"enabled":True,"endpoint":"http://host.docker.internal:30417","protocol":"grpc","resourceAttributes":"deployment.environment=agentic-k8s-platform"}]],"mcp":{"grafana":{"type":"local","command":["mcp-grafana"],"environment":{"GRAFANA_URL":"http://grafana.platform.localhost","GRAFANA_SERVICE_ACCOUNT_TOKEN":"'"$GRAFANA_SERVICE_ACCOUNT_TOKEN"'"}},"k8s":{"type":"local","command":["kubectl-mcp-server"],"environment":{"KUBECONFIG":"/home/agent/.kube/config"}}},"references":{"workspace":{"path":"/workspace","description":"Agent workspace"}}}; open("'"$oc"'","w").write(json.dumps(cfg,indent=2)+"\n")'

    echo "→ Building DevContainer image..."
    devcontainer build --workspace-folder "$ws"

    echo "→ Launching DevContainer..."
    # devcontainer up internally does exec into the container; CWD must be
    # on a path that maps into the container mount namespace (the workspace dir)
    cd "$ws"
    KUBECONFIG_PATH="$kc" OPENCODE_CONFIG_PATH="$oc" \
      devcontainer up --workspace-folder "$ws"

    echo "→ Starting OpenChamber web UI..."
    container_id=$(docker ps -q --filter "label=devcontainer.local_folder=$(readlink -f "$ws")" | head -1)
    if [ -n "$container_id" ]; then
      docker exec "$container_id" start-openchamber || true
    fi

    # Sync existing dashboards from this agent's vCluster to Grafana now.
    # Continuous sync runs via `just watch-dashboards` on the host.
    just sync-dashboards 2>/dev/null || true

    echo ""
    echo "✓ Workspace ready at $ws"
    echo ""
    echo "  OpenChamber:  http://localhost:$ui_port  (web UI — chat, diff, git, terminal)"
    echo "  OpenCode API: http://localhost:$oc_port  (programmatic — sessions, prompts)"
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
    n=0
    until timeout 90 vcluster connect {{name}} -n "$ns" --print \
      --server "https://host.docker.internal:${port}" >"{{kubeconfigs}}/{{name}}.yaml" 2>/dev/null \
      && [ -s "{{kubeconfigs}}/{{name}}.yaml" ]; do
      n=$((n + 1))
      [ "$n" -ge 5 ] && { echo "ERROR: 'vcluster connect' failed after 5 attempts." >&2; exit 1; }
      echo "  vcluster connect attempt $n failed; retrying in $((n * 5))s..." >&2
      sleep $((n * 5))
    done
    echo "✓ {{kubeconfigs}}/{{name}}.yaml (NodePort $port)"

# Tear down an agent (vCluster + namespace + kubeconfig + opencode config + DevContainer)
deprovision name: (_check-name name) _check-roots
    #!/usr/bin/env bash
    set -uo pipefail
    name="{{name}}"                       # already validated as a DNS label by _check-name
    ws="{{workspaces}}/$name"
    # Every step is best-effort so a partially-provisioned agent still cleans up, but the paths
    # are now fixed strings built from a validated name — no empty-variable expansion possible.
    vcluster delete "$name" -n "vc-$name" || true
    kubectl --context {{context}} delete ns "vc-$name" --ignore-not-found || true
    cids="$(docker ps -aq --filter "label=devcontainer.local_folder=$(readlink -f "$ws" 2>/dev/null || echo "$ws")" 2>/dev/null || true)"
    [ -n "$cids" ] && docker rm -f $cids >/dev/null 2>&1 || true
    rm -rf -- "$ws" 2>/dev/null || true
    rm -f -- \
      "{{kubeconfigs}}/$name.yaml" "{{kubeconfigs}}/$name.host.yaml" \
      "{{opencodes}}/$name.json"
    echo "✓ $name torn down."

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

# Discover every endpoint: Traefik Host-routes (:80), published container ports, and NodePorts
endpoints:
    #!/usr/bin/env bash
    set -uo pipefail
    echo "── HTTP via Traefik — reach at http://<HOST> on :80 (routed by Host header) ──"
    kubectl --context {{context}} get ingress -A --no-headers \
      -o custom-columns='HOST:.spec.rules[*].host,NAMESPACE:.metadata.namespace' 2>/dev/null | sed 's|^|  |' || echo "  (none)"
    echo
    echo "── Published host ports — reach on 127.0.0.1:<port> (Host header IGNORED) ──"
    echo "  Forgejo git ssh → localhost:2222   (Forgejo HTTP is via Traefik — see the list above)"
    echo "  Agent OpenChamber UIs → http://localhost:3100<index>"
    docker ps 2>/dev/null | grep -vE 'k3d-' | sed 's|^|  |' || true
    echo
    echo "── NodePorts — reach on 127.0.0.1:<nodePort> ──"
    kubectl --context {{context}} get svc -A 2>/dev/null | awk 'NR==1 || /NodePort/ {print "  " $0}' || true

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
