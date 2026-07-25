# Harness Fixes — post-Astroshop retrospective

Derived from the first end-to-end agent run (OTel Astroshop demo → LGTM → dashboard).
Every root cause below was **re-verified against the live cluster**, not taken from the
agent's report. Three of the agent's own diagnoses were wrong — see *Corrections*.

Status: ⬜ todo · ✅ done

---

## Corrections to the agent's report

Worth reading first, because two of the agent's proposed platform fixes would not have worked.

1. **`*.localhost` is not a DNS problem.** `/etc/hosts` in the DevContainer is already
   correct (`172.17.0.1 grafana.platform.localhost`), and `getent ahostsv4` returns
   `172.17.0.1`. **curl** (8.5.0) hard-codes RFC 6761: any host ending in `.localhost`
   resolves internally to `::1`/`127.0.0.1`, bypassing `/etc/hosts` and NSS entirely.
   Adding hosts entries — the agent's suggested fix — changes nothing.

   ```
   getent ahostsv4 grafana.platform.localhost  → 172.17.0.1
   curl -4 http://grafana.platform.localhost   → Trying [::1]:80 … refused
   curl --resolve '*:80:172.17.0.1' …          → 200
   ```

2. **Both MCP servers are broken, for two unrelated reasons.** The agent guessed the
   Grafana one; the Kubernetes one has an independent failure it never found:
   - `mcp-grafana` → installed under `/root/.local/share/uv/tools/…`, and `/root` is
     `drwx------`. The `agent` user cannot traverse it; `which mcp-grafana` is empty.
   - `kubectl-mcp-server` → the npm package is a shim that **pip-installs
     `kubectl-mcp-tool` at first run**, which dies on Ubuntu 24.04's PEP 668
     `externally-managed-environment`. It exits 0 having installed nothing.

3. **Mimir's label schema is a Mimir setting, not a fact of life.** `service_name` *does*
   exist in Mimir — but only for `flagd`, which emits it as a datapoint attribute. Every
   other service has only `job`, because Mimir's OTLP ingest collapses `service.name` →
   `job` and drops the rest of the resource attributes. `-distributor.otel-promote-resource-attributes`
   fixes this at the source.

Two of the nine issues were **already known and unfixed** — `improvements.md` #4 (quota too
tight for Astroshop) and #7 (host-cluster DNS is fragile). The agent paid full price for
findings the platform had already written down. Closing the loop from `improvements.md` to
the justfile matters as much as any individual fix here.

---

## F1 — Stop the LimitRange from manufacturing CPU demand ✅

**Highest leverage. Kills issues #1 and #2.**

Verified on the live namespace:

```
requests.cpu    610m    / 4      ← what the workload actually asks for
limits.cpu      5050m   / 6      ← what the LimitRange invented   (8× inflation)
```

`provision` sets a quota on `limits.cpu` **and** a LimitRange `default: {cpu: 500m}`.
Kubernetes requires that any resource tracked by a quota be set on every container, so the
LimitRange must supply a default — and that default, times ~25 containers (including init
containers and sidecars, which the LimitRanger also defaults), is what exhausts the quota.
The workload was never near its real budget.

**Fix as shipped** — govern CPU by *requests*, keep hard limits only on memory
(justfile `provision`):

```yaml
# ResourceQuota
hard:
  requests.cpu: "4"
  requests.memory: 8Gi
  limits.memory: 24Gi          # dropped: limits.cpu   (see "memory has the same trap")
  pods: "50"
  persistentvolumeclaims: "10"

# LimitRange
limits:
  - type: Container
    default:        { memory: 512Mi }    # no cpu → no CPU limit, no throttling, no phantom quota
    defaultRequest: { cpu: 50m, memory: 128Mi }
    max:            { memory: 4Gi }      # NO max.cpu — see "the max trap"
```

Once `limits.cpu` is off the quota, containers need no CPU limit at all, so nothing has to
be invented and nothing gets CFS-throttled during JVM startup.

**Tradeoff, stated explicitly:** a runaway pod can then burn host CPU up to the ceiling
`just harden` sets (`CPUQuota=1400%` across all of Docker). Scheduling fairness between
agents still holds via `requests.cpu: 4`. For a dev platform this is the right trade —
CPU is compressible, memory is not, and memory keeps its hard cap.

### Two things the implementation turned up

**The `max` trap.** The first attempt set `max: { cpu: "2", memory: 4Gi }`. The API server
came back with `default: { cpu: "2", memory: 512Mi }` — the **LimitRanger back-fills
`default` from `max`** for any resource `default` omits. Setting a CPU ceiling therefore
silently re-creates the invented CPU limit the whole change exists to remove, four times
larger than the 500m it replaced. `max.cpu` had to be dropped entirely; total CPU stays
bounded by `requests.cpu` and by `just harden`.

**Memory has the identical trap, and it bites at realistic scale.** A probe of 20 pods ×
(1 init + 2 containers) with *zero* resource declarations — the exact shape that failed
originally — stalled at 11 of 20 pods against `limits.memory: 12Gi`, because the 512Mi
default is charged per container: ~1Gi booked per pod against ~256Mi actually reserved.
CPU was a non-issue by then (`requests.cpu 1120m/4`). Memory can't be dropped from the
quota the way `limits.cpu` was — it isn't compressible — so the quota is sized at the
overcommit the defaults inevitably produce (`24Gi` = 3× `requests.memory`) rather than at
the real footprint. The actual protections remain `requests.memory`, node capacity, and the
`harden` ceiling.

**Verified live** on a throwaway vCluster (`fixtest`, since destroyed). Same 20-pod probe,
stock config, no per-container resource overrides:

```
20 Running / 20            (was: 9 Running, 11 Pending)
requests.cpu     2220m  / 4
requests.memory  5440Mi / 8Gi
limits.memory   22698Mi / 24Gi
```

Under the old config the same workload needed 30 CPU of limits against a 6 CPU quota —
dead on arrival. **Still worth doing:** the real Astroshop chart end-to-end with stock
values; the probe is a deliberately worst-case synthetic.

## F2 — Make the budget discoverable *inside* the vCluster ⬜

The quota and LimitRange live in the host namespace `vc-<name>`. vCluster does not sync
them, so `kubectl describe quota agent-quota` — which `SKILL.md:220` and `:307` explicitly
instruct the agent to run — returns `not found`. The only signal is a `pod-syncer` event.
The skill documents a command that cannot work.

**Fix:**

1. In `provision`, after the vCluster is up, apply through the *agent* kubeconfig into the
   virtual `default` namespace: the same LimitRange, plus a mirrored ResourceQuota sized
   ~10% tighter than the host's. The vCluster API then rejects at `kubectl apply`/`helm
   install` time with a legible `exceeded quota` message instead of a silent Pending.
2. Add a `platform-info` ConfigMap in the vCluster carrying budget, endpoints, ingress
   convention, and the Mimir label schema — one object an agent can read to orient itself.
3. Ship a `budget-check` helper in the agent image: `helm template … | budget-check` sums
   requests/limits across containers **and initContainers/sidecars**, applies LimitRange
   defaults, and diffs against the quota. Turns 4 install/uninstall cycles into one
   pre-flight command.
4. Update `SKILL.md` (and `AGENTS.md`) with the real numbers and a "check your budget
   before installing a chart" step.

## F3 — Bake host-service DNS into the vCluster ✅

vCluster CoreDNS knows nothing about the host cluster, so
`otel-collector.observability.svc.cluster.local` — the endpoint both `SKILL.md:74` and
`AGENTS.md` tell agents to use — does not resolve. The agent hand-patched the CoreDNS
ConfigMap (that patch is live now, and dies with the vCluster).

Confirmed against the vcluster 0.35.1 values schema:

| Option | Available? | Note |
|---|---|---|
| `networking.replicateServices.fromHost` | ✅ OSS | explicit `from: ns/name` → `to: ns/name` |
| `networking.advanced.fallbackHostCluster` | ✅ OSS | blanket fallback to host DNS |
| `networking.resolveDNS` | ❌ **PRO** | requires `coredns.embedded`, a paid feature — rule it out |
| `controlPlane.coredns.overwriteConfig` | ✅ OSS | full Corefile replacement; last resort |

**Fix** — in the `provision` values file:

```yaml
networking:
  replicateServices:
    fromHost:
      - from: observability/otel-collector
        to:   observability/otel-collector
  advanced:
    fallbackHostCluster: true
```

`provision` also pre-creates the `observability` namespace **inside** the vCluster —
`replicateServices.fromHost` silently does nothing if the target namespace is absent — and
then polls for the replicated Service so a failure is reported at provision time instead of
surfacing later as an unexplained telemetry gap.

**Verified live** from a workload pod in a fresh vCluster, with no CoreDNS patching:

```
otel-collector.observability.svc.cluster.local → 10.43.151.112   (= host collector ClusterIP)
nc -z otel-collector.observability.svc.cluster.local 4317 → TCP 4317 OPEN
mimir.observability.svc.cluster.local          → 10.43.114.57
grafana.observability.svc.cluster.local        → 10.43.123.217   (TCP 80 OPEN)
```

`fallbackHostCluster` rewrites the vCluster Corefile to `forward . 10.43.0.10` with
`fallthrough cluster.local` — declaratively, exactly what the agent hand-patched by hand.

**One caveat worth documenting for agents:** it resolves **FQDNs only**. `mimir.observability`
NXDOMAINs; `mimir.observability.svc.cluster.local` resolves. The vCluster's search-domain
list doesn't cover the short form. `SKILL.md` now says so explicitly.

## F4 — Generate the Grafana token at provisioning time ✅

`.env` still has `GRAFANA_SERVICE_ACCOUNT_TOKEN=` empty, and the running container confirms
`len=0`. The `{env:…}` placeholder in `opencode.jsonc` was correct; nothing ever populated
it. The agent burned 8 approaches before asking.

**Fix:**

1. New recipe `just grafana-token` — wait for Grafana ready → read the admin password from
   the `grafana` secret → `POST /api/serviceaccounts` (name `agent-platform`, role
   **Editor**, required for dashboard writes) → `POST /api/serviceaccounts/{id}/tokens` →
   write into `.env`. Idempotent: reuse the SA if it exists.
2. Call it at the tail of `just observability`.
3. `workspace` fails fast on an empty token, exactly as it already does for
   `DEEPSEEK_API_KEY` (justfile:360) — and bakes the value into the generated per-agent
   `opencode.json` rather than relying on `{env:…}` plumbing, matching how the DeepSeek key
   is already handled (justfile:388).

**Verified live** against the running Grafana:

```
run 1 → created service account agent-platform (id=3)
run 2 → reusing service account agent-platform (id=3, role forced to Editor)
GET  /api/dashboards/home  → 200        (token authenticates)
POST /api/dashboards/db    → success    (Editor role — dashboard writes work)
service accounts: agent-platform role=Editor tokens=1   ← no accumulation across runs
```

The pre-existing manually-created `test-agent` service account is left untouched. `.env`
keeps its other keys and is chmod 600; the token value is never echoed.

Note `improvements.md` flags that this token is unscoped — any agent holding it can query
all telemetry. Out of scope here; worth a Grafana LBAC follow-up.

## F5 — MCP servers that actually start ⬜

Neither MCP server has ever run. See *Corrections* #2 for the two independent root causes.

**Fix (`.devcontainer/Dockerfile`):**

1. Install uv tools to a world-readable prefix — `ENV UV_TOOL_DIR=/opt/uv/tools
   UV_TOOL_BIN_DIR=/usr/local/bin` before `uv tool install mcp-grafana`.
2. Drop every `/root/...` entry from the final `PATH` (currently three, all unreachable by
   the `agent` user, and duplicated).
3. Replace the `kubectl-mcp-server` npm shim with a self-contained binary that does not
   install anything at runtime.
4. **Add a build-time smoke test that runs as the `agent` user** and fails the build:
   send each MCP command a JSON-RPC `initialize` over stdio and assert a response. This is
   the durable fix — it catches this whole class of regression at build, not on the agent's
   clock.
5. Add `platform-doctor` (in-container) and `just doctor` (host) that check MCP reachability,
   the Grafana token, host-service DNS, and quota headroom in one shot.

## F6 — Make `curl` work against `*.localhost` ⬜

See *Corrections* #1. This is not cosmetic: `SKILL.md:209` tells the agent to validate its
own deployment with `curl http://<app>.<your-name>.localhost/healthz`, and that command
**can never succeed**. Proven against the agent's live Astroshop ingress:

```
curl http://astroshop.my-agent.localhost/                        → 000 (loopback, refused)
curl --resolve '*:80:172.17.0.1' http://astroshop.my-agent.localhost/  → 200
```

**Fix:** ship `/usr/local/bin/curl` as a shim ahead of `/usr/bin/curl` that injects
`--resolve <host>:<port>:<host-gateway>` when the target host ends in `.localhost`, and
`exec`s the real curl otherwise. A blanket `~/.curlrc` with `resolve = *:80:172.17.0.1` is
one line but hijacks *all* plain-HTTP traffic including external calls — prefer the targeted
shim. Document `--resolve` in `SKILL.md` as the manual escape hatch.

Only curl is affected: Go (`mcp-grafana`) and Python resolvers read `/etc/hosts` normally,
so `GRAFANA_URL=http://grafana.platform.localhost` stays correct for the MCP server.

**Also fix while in here:** stale hostnames — `SKILL.md:72` and `:191` still say
`forgejo.localhost:3000`; the live container's `/etc/hosts` has `forgejo.localhost` from a
stale image build while `devcontainer.json:14` now says `forgejo.platform.localhost`.

## F7 — Fix the metric label schema and the dashboard defaults ⬜

Live label inventory: `job` carries the service name (17 values: `frontend`, `cart`, …);
there is no `service_namespace`; `service_name` exists only for `flagd`.

**Fix:**

1. `manifests/mimir-monolithic.yaml` — add to the config:
   ```yaml
   limits:
     promote_otel_resource_attributes:
       - service.name
       - service.namespace
       - service.version
       - k8s.namespace.name
       - k8s.pod.name
       - deployment.environment
   ```
   (`-distributor.otel-promote-resource-attributes`, present in Mimir 3.1.4, experimental.)
   The agent's first instinct — `service_name` — then simply works, and `job` keeps working.
2. `manifests/grafana-datasources.yaml` — pin **stable datasource UIDs** (`uid: mimir`,
   `uid: tempo`, `uid: loki`). This is the whole of the agent's "problem round 3": it had to
   hardcode a random generated UID (`PAE45454D0EDB9216`) into every panel because the
   default reference didn't resolve. With fixed UIDs, dashboards are portable and
   reproducible across rebuilds.
3. Document the label schema and a known-good PromQL example in `SKILL.md`, including the
   `$__rate_interval` caveat (it resolves against the panel's `interval`/scrape assumptions;
   an explicit `[5m]` is the reliable default for OTLP-pushed metrics).
4. Ship one **known-good starter dashboard** JSON in `manifests/` as a copyable template.

## F8 — Guardrails against self-inflicted damage ⬜

Lower value, cheap to add.

- `kubectl delete rs,deploy,svc,ingress --all` took out the `kubernetes` ClusterIP service.
  Add a `permission.bash` deny for bare `--all` deletes and for `kubectl delete svc
  kubernetes` in `opencode.jsonc` + the generated per-agent config, and a "never delete
  `svc/kubernetes`" line in `SKILL.md`.
- Stuck-pod recovery (issue #3) — **the mechanism is now confirmed, and it is not merely
  downstream of F1.** Reproduced during F1 testing: the vCluster `pod-syncer` attempts the
  host-side create ~2 times, records a `SyncError`, and then **abandons the pod**. It does
  not re-reconcile when the blocker clears. After raising the quota mid-test, 11 pods stayed
  Pending indefinitely citing the *old* limit (`limited: limits.memory=12Gi`); deleting them
  so the ReplicaSet recreated them brought all 20 up immediately. That is exactly the "new
  values aren't being applied" death spiral from the report — the fix had applied, the
  abandoned pods just never retried. Documented in `SKILL.md` triage (✅);
  `/diagnose` should grow the same check.

## F9 — Guard the destructive recipes ✅

Not from the agent's report — surfaced while implementing the above, and worse than
anything in it.

`deprovision` and `workspace` both interpolate an unvalidated agent name straight into an
`rm -rf`. `just deprovision ""` expanded to `rm -rf $HOME/.local/share/agent-workspaces/`,
destroying **every** agent's workspace rather than one; a name containing `/` or `..` could
leave the storage root altogether. `just` requires the argument to be present, but not to be
non-empty or path-free.

**Fix:** two private recipes, `_check-name` (requires a DNS label:
`^[a-z0-9]([a-z0-9-]*[a-z0-9])?$`) and `_check-roots` (rejects a storage root that resolved
to `""`, `/`, or bare `$HOME`), wired as dependencies of `provision`, `workspace` and
`deprovision` so they run **before** any recipe body. `deprovision` also became a shebang
recipe with explicit `|| true` per step and `rm -rf --` over fixed strings, replacing the
`-`-prefixed line that hid failures.

**Verified:** `''`, `.`, `..`, `/`, `a/b`, `../etc`, `-rf`, `UPPER`, `trailing-` are all
rejected before any destructive step; valid names still tear down cleanly.

Also fixed here: `deprovision` never removed the `<name>.host.yaml` kubeconfig that
`provision` creates, leaking one file per agent lifecycle.

---

## Ordering

**F1 + F3 + F4 are done** (plus F9, and the parts of F2/F8 that documented the behaviour
those changes produced).

**Then F5 + F6**, which are both `.devcontainer/Dockerfile` changes and share one image
rebuild — bundle them.

**Then the rest of F2, F7, F8.** F2 is the one that most directly serves the "better harness
→ smaller models iterate" thesis: it converts a silent Pending into a legible error at apply
time. Note the F1 testing raised its value — the `pod-syncer` `SyncError` event is currently
the *only* signal an agent gets, and it is easy to miss.

## Validation

The fixes are only real if a fresh run confirms them. After F1–F6:

```
just nuke && just bootstrap && just observability && just grafana-token
just provision test-agent 1 && just workspace test-agent 1 <repo>
```

then, as the agent, install Astroshop with **stock chart values** and no CoreDNS patching.
That single run exercises F1 (quota), F2 (visible quota), F3 (DNS), F4 (token), F5 (MCP),
F6 (curl validation) and F7 (dashboard labels) end to end. Anything still requiring a manual
workaround has not been fixed.
