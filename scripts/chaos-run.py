#!/usr/bin/env python3
"""Run the chaos suite against one agent's vCluster and report structured PASS/FAIL.

Invoked by the operator plane — `just chaos-suite <name> [scenario...]`, which the
pool-manager calls when an agent comments `/chaos` on its issue. It deliberately does NOT
run inside the agent's container: the agent triggers the suite, the operator executes it, so
the verdict is not something the agent can forge or weaken. Same split as teardown.

Output is JSON on stdout with a ready-to-post `markdown` field, because the whole point is
that a failure reaching the agent names a *behaviour* ("health reported OK while the
dependency was unreachable") rather than an exit code it has to go and interpret.

Exit codes:
  0  every selected scenario passed
  1  at least one scenario failed  (the agent's code is wrong)
  2  the harness could not run     (missing .chaos.yaml, no kubeconfig, chart install failed)

1 vs 2 matters: the agent must be told to fix its code only when its code is actually what
broke.
"""

import argparse
import json
import os
import shlex
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path

import yaml

REPO = Path(__file__).resolve().parent.parent
SCENARIO_DIR = REPO / "chaos" / "scenarios"
KUBECONFIG_DIR = Path.home() / ".local/share/agent-kubeconfigs"
WORKSPACE_DIR = Path.home() / ".local/share/agent-workspaces"
CHAOS_NS = "chaos-system"
PAUSE_IMAGE = "registry.k8s.io/pause:3.10"

# A step that hangs is indistinguishable from a step that failed, and the gate must always
# return a verdict — so every shell-out is bounded.
STEP_TIMEOUT = 300


class HarnessError(Exception):
    """The suite could not be run. Never the agent's fault — exit 2, don't nudge."""


# ── shelling out ────────────────────────────────────────────────────────────────────────


def run(cmd, cwd=None, env=None, timeout=STEP_TIMEOUT, shell=False):
    try:
        p = subprocess.run(
            cmd,
            cwd=cwd,
            env=env,
            shell=shell,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
        return p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired:
        return 124, "", f"timed out after {timeout}s"


class Cluster:
    """kubectl against one agent's vCluster, plus the two in-cluster control APIs.

    The control APIs are reached through the API server's *service proxy* rather than
    port-forward or an Ingress. Both of those are forbidden to agents by AGENTS.md, and a
    tool the agent is not allowed to imitate is a tool that teaches it the wrong pattern.
    """

    def __init__(self, kubeconfig):
        self.kubeconfig = str(kubeconfig)

    def kubectl(self, *args, timeout=STEP_TIMEOUT, check=True):
        code, out, err = run(
            ["kubectl", "--kubeconfig", self.kubeconfig, *args], timeout=timeout
        )
        if check and code != 0:
            raise HarnessError(f"kubectl {' '.join(args)} failed: {err.strip() or out.strip()}")
        return code, out, err

    def _proxy(self, service, port, path):
        return f"/api/v1/namespaces/{CHAOS_NS}/services/http:{service}:{port}/proxy{path}"

    def proxy_get(self, service, port, path):
        _, out, _ = self.kubectl("get", "--raw", self._proxy(service, port, path))
        return json.loads(out) if out.strip() else {}

    def proxy_post(self, service, port, path, payload):
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as fh:
            json.dump(payload, fh)
            tmp = fh.name
        try:
            _, out, _ = self.kubectl(
                "create", "--raw", self._proxy(service, port, path), "-f", tmp
            )
            return json.loads(out) if out.strip() else {}
        finally:
            os.unlink(tmp)


# ── contract ────────────────────────────────────────────────────────────────────────────


REQUIRED_CONTRACT_KEYS = ("deploy", "ready", "verify", "selector")


def load_contract(workspace):
    path = workspace / ".chaos.yaml"
    if not path.exists():
        raise HarnessError(
            "no .chaos.yaml at the repo root. The chaos suite needs to know how your work is "
            "deployed, readied and verified before it can perturb any of it. Add:\n\n"
            "    namespace: default\n"
            '    deploy:  "kubectl apply -k ./deploy"\n'
            '    ready:   "kubectl rollout status deploy/<name> --timeout=120s"\n'
            '    verify:  "curl -fsS http://<name>.$VCLUSTER_NAME.localhost/healthz"\n'
            '    selector: "app=<name>"\n'
            "    dependency:            # optional\n"
            "      name: postgres\n"
            '      selector: "app=postgres"\n'
            "      port: 5432\n"
        )
    try:
        contract = yaml.safe_load(path.read_text()) or {}
    except yaml.YAMLError as e:
        raise HarnessError(f".chaos.yaml is not valid YAML: {e}")

    missing = [k for k in REQUIRED_CONTRACT_KEYS if not contract.get(k)]
    if missing:
        raise HarnessError(f".chaos.yaml is missing required keys: {', '.join(missing)}")
    contract.setdefault("namespace", "default")
    return contract


def load_scenarios(names, suite):
    files = sorted(SCENARIO_DIR.glob("*.yaml"))
    if not files:
        raise HarnessError(f"no scenarios found in {SCENARIO_DIR}")
    loaded = [yaml.safe_load(f.read_text()) for f in files]
    by_name = {s["name"]: s for s in loaded}

    if names:
        unknown = [n for n in names if n not in by_name]
        if unknown:
            raise HarnessError(
                f"unknown scenario(s): {', '.join(unknown)}. "
                f"Available: {', '.join(sorted(by_name))}"
            )
        return [by_name[n] for n in names]
    if suite == "all":
        return loaded
    return [s for s in loaded if s.get("suite", "default") == suite]


# ── the executor ────────────────────────────────────────────────────────────────────────


class Runner:
    def __init__(self, cluster, contract, workspace, agent):
        self.c = cluster
        self.contract = contract
        self.ns = contract["namespace"]
        self.workspace = workspace
        self.agent = agent
        self.captures = {}
        self.restore = []  # callables undoing any mutation, run LIFO after each scenario
        self.env = {
            **os.environ,
            "KUBECONFIG": cluster.kubeconfig,
            "VCLUSTER_NAME": agent,
            "CHAOS": "1",
        }

    # -- contract commands -------------------------------------------------------------

    def contract_cmd(self, key):
        code, out, err = run(
            self.contract[key],
            cwd=str(self.workspace),
            env=self.env,
            shell=True,
        )
        return code, out, err

    # -- target resolution -------------------------------------------------------------

    def selector_for(self, target):
        if target == "dependency":
            dep = self.contract.get("dependency") or {}
            sel = dep.get("selector")
            if not sel:
                raise HarnessError("scenario targets the dependency but .chaos.yaml declares none")
            return sel
        return self.contract["selector"]

    def workloads_for(self, target):
        sel = self.selector_for(target)
        _, out, _ = self.c.kubectl(
            "get", "deploy,statefulset", "-l", sel, "-n", self.ns, "-o", "name"
        )
        names = [line.strip() for line in out.splitlines() if line.strip()]
        if not names:
            raise HarnessError(
                f"no Deployment or StatefulSet matches selector '{sel}' in namespace "
                f"'{self.ns}' — check `selector` in .chaos.yaml"
            )
        return names

    # -- steps -------------------------------------------------------------------------

    def step(self, spec):
        """Execute one step. Returns (ok, detail)."""
        kind = spec["do"]
        handler = getattr(self, f"_do_{kind.replace('-', '_')}", None)
        if handler is None:
            raise HarnessError(f"unknown step '{kind}' — see chaos/README.md for the vocabulary")
        return handler(spec)

    def _contract_step(self, key, spec):
        code, out, err = self.contract_cmd(key)
        want_fail = spec.get("expect") == "fail"
        if spec.get("capture"):
            self.captures[spec["capture"]] = out.strip()
        ok = (code != 0) if want_fail else (code == 0)
        if ok:
            return True, ""
        if want_fail:
            return False, f"`{key}` succeeded but was expected to fail (exit 0)"
        return False, f"`{key}` exited {code}\n{(err or out).strip()[-1500:]}"

    def _do_deploy(self, spec):
        return self._contract_step("deploy", spec)

    def _do_ready(self, spec):
        return self._contract_step("ready", spec)

    def _do_verify(self, spec):
        return self._contract_step("verify", spec)

    def _do_sleep(self, spec):
        time.sleep(float(spec.get("seconds", 5)))
        return True, ""

    def _do_assert_same(self, spec):
        a, b = self.captures.get(spec["a"]), self.captures.get(spec["b"])
        if a == b:
            return True, ""
        return False, f"'{spec['a']}' != '{spec['b']}'\n  before: {a!r}\n  after:  {b!r}"

    def _do_assert_recovered(self, spec):
        deadline = time.time() + float(spec.get("timeout", 180))
        last = ""
        while time.time() < deadline:
            rc, _, rerr = self.contract_cmd("ready")
            if rc == 0:
                vc, vout, verr = self.contract_cmd("verify")
                if vc == 0:
                    return True, ""
                last = (verr or vout).strip()
            else:
                last = rerr.strip()
            time.sleep(5)
        return False, f"still unhealthy after {spec.get('timeout', 180)}s\n{last[-1500:]}"

    # -- API-only faults ---------------------------------------------------------------

    def _do_kill(self, spec):
        sel = self.selector_for(spec.get("target", "app"))
        self.c.kubectl("delete", "pod", "-l", sel, "-n", self.ns, "--wait=false")
        return True, ""

    def _do_scale(self, spec):
        replicas = str(spec.get("replicas", 0))
        for w in self.workloads_for(spec.get("target", "app")):
            self.c.kubectl("scale", w, "-n", self.ns, f"--replicas={replicas}")
        return True, ""

    def _do_stall(self, spec):
        """Swap containers for a pause image: present, scheduled, never Ready, no endpoints."""
        target = spec.get("target", "app")
        for w in self.workloads_for(target):
            _, out, _ = self.c.kubectl(
                "get", w, "-n", self.ns,
                "-o", "jsonpath={.spec.template.spec.containers[*].image}",
            )
            originals = out.split()
            patch = {
                "spec": {"template": {"spec": {"containers": [
                    {"name": n, "image": PAUSE_IMAGE} for n in self._container_names(w)
                ]}}}
            }
            self.c.kubectl("patch", w, "-n", self.ns, "--type=strategic", "-p", json.dumps(patch))
            self.restore.append(lambda w=w, imgs=originals: self._restore_images(w, imgs))
        return True, ""

    def _do_unstall(self, spec):
        self._run_restores()
        return True, ""

    def _container_names(self, workload):
        _, out, _ = self.c.kubectl(
            "get", workload, "-n", self.ns,
            "-o", "jsonpath={.spec.template.spec.containers[*].name}",
        )
        return out.split()

    def _restore_images(self, workload, images):
        names = self._container_names(workload)
        patch = {"spec": {"template": {"spec": {"containers": [
            {"name": n, "image": i} for n, i in zip(names, images)
        ]}}}}
        self.c.kubectl("patch", workload, "-n", self.ns, "--type=strategic", "-p", json.dumps(patch),
                       check=False)

    def _do_squeeze(self, spec):
        """Force a real kernel OOMKill, not a simulated crash."""
        memory = spec.get("memory", "16Mi")
        for w in self.workloads_for(spec.get("target", "app")):
            names = self._container_names(w)
            _, out, _ = self.c.kubectl(
                "get", w, "-n", self.ns,
                "-o", "jsonpath={.spec.template.spec.containers[*].resources.limits.memory}",
            )
            originals = out.split() or [""] * len(names)
            patch = {"spec": {"template": {"spec": {"containers": [
                {"name": n, "resources": {"limits": {"memory": memory}}} for n in names
            ]}}}}
            self.c.kubectl("patch", w, "-n", self.ns, "--type=strategic", "-p", json.dumps(patch))
            self.restore.append(lambda w=w, m=originals: self._restore_memory(w, m))
        return True, ""

    def _do_unsqueeze(self, spec):
        self._run_restores()
        return True, ""

    def _restore_memory(self, workload, originals):
        names = self._container_names(workload)
        containers = []
        for n, m in zip(names, originals):
            containers.append({"name": n, "resources": {"limits": {"memory": m or None}}})
        patch = {"spec": {"template": {"spec": {"containers": containers}}}}
        self.c.kubectl("patch", workload, "-n", self.ns, "--type=strategic", "-p", json.dumps(patch),
                       check=False)

    def _do_partition(self, spec):
        """Cut egress to the dependency with a NetworkPolicy — the only zero-privilege cut.

        Requires sync.toHost.networkPolicies in values/vcluster.yaml. Without it the policy is
        an inert object in the virtual API and the scenario would pass for the wrong reason,
        so the policy's arrival is asserted rather than assumed.
        """
        seconds = float(spec.get("seconds", 30))
        sel = self.contract["selector"]
        key, _, value = sel.partition("=")
        policy = {
            "apiVersion": "networking.k8s.io/v1",
            "kind": "NetworkPolicy",
            "metadata": {"name": "chaos-partition", "namespace": self.ns},
            "spec": {
                "podSelector": {"matchLabels": {key: value}},
                "policyTypes": ["Egress"],
                # DNS only. Everything else — including the dependency — is dropped, so
                # connections hang rather than being refused.
                "egress": [{
                    "ports": [{"protocol": "UDP", "port": 53}, {"protocol": "TCP", "port": 53}]
                }],
            },
        }
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as fh:
            json.dump(policy, fh)
            tmp = fh.name
        try:
            self.c.kubectl("apply", "-f", tmp)
        finally:
            os.unlink(tmp)

        code, out, _ = self.c.kubectl(
            "get", "networkpolicy", "chaos-partition", "-n", self.ns, "-o", "name", check=False
        )
        if code != 0:
            return False, "the NetworkPolicy did not apply — the partition never happened"

        time.sleep(seconds)
        self.c.kubectl("delete", "networkpolicy", "chaos-partition", "-n", self.ns,
                       "--ignore-not-found", check=False)
        return True, ""

    # -- admission faults ---------------------------------------------------------------

    def _do_arm(self, spec):
        payload = {
            "mode": spec["mode"],
            "ordinals": spec.get("ordinals", [1]),
            "operations": spec.get("operations", []),
            "resources": spec.get("resources", []),
            # Scoped to the app's namespace so unrelated writes elsewhere don't consume
            # ordinals and shift which call actually fails.
            "namespaces": spec.get("namespaces", [self.ns]),
        }
        self.c.proxy_post("fault-webhook", 8080, "/arm", payload)
        return True, ""

    def _do_disarm(self, spec):
        self.c.proxy_post("fault-webhook", 8080, "/reset", {})
        return True, ""

    def _do_assert_fired(self, spec):
        state = self.c.proxy_get("fault-webhook", 8080, "/state")
        fired = state.get("firedCount", 0)
        need = int(spec.get("min", 1))
        if fired >= need:
            return True, ""
        return False, (
            f"the armed fault fired {fired} times, expected at least {need}. "
            f"{state.get('seen', 0)} matching writes were seen"
        )

    # -- toxiproxy -----------------------------------------------------------------------

    def _proxy_name(self):
        return (self.contract.get("dependency") or {}).get("name", "dependency")

    def _ensure_toxiproxy(self):
        dep = self.contract.get("dependency") or {}
        name = self._proxy_name()
        upstream = f"{dep['name']}.{self.ns}.svc.cluster.local:{dep['port']}"
        existing = self.c.proxy_get("toxiproxy", 8474, "/proxies")
        if name not in existing:
            self.c.proxy_post("toxiproxy", 8474, "/proxies", {
                "name": name, "listen": "0.0.0.0:21000", "upstream": upstream, "enabled": True,
            })
        return name

    def _do_toxic(self, spec):
        name = self._ensure_toxiproxy()
        attrs = {k: spec[k] for k in ("latency", "jitter", "rate", "timeout") if k in spec}
        self.c.proxy_post("toxiproxy", 8474, f"/proxies/{name}/toxics", {
            "name": "chaos", "type": spec["type"], "stream": "downstream", "attributes": attrs,
        })
        return True, ""

    def _do_untoxic(self, spec):
        name = self._proxy_name()
        self.c.kubectl(
            "delete", "--raw",
            f"/api/v1/namespaces/{CHAOS_NS}/services/http:toxiproxy:8474/proxy"
            f"/proxies/{name}/toxics/chaos",
            check=False,
        )
        return True, ""

    # -- cleanup -------------------------------------------------------------------------

    def _run_restores(self):
        while self.restore:
            try:
                self.restore.pop()()
            except Exception:  # cleanup must never mask the real verdict
                pass

    def cleanup(self):
        self._run_restores()
        self.c.kubectl(
            "delete", "networkpolicy", "chaos-partition", "-n", self.ns,
            "--ignore-not-found", check=False,
        )
        try:
            self.c.proxy_post("fault-webhook", 8080, "/reset", {})
        except HarnessError:
            pass


# ── scenario loop ───────────────────────────────────────────────────────────────────────


def scenario_skip_reason(scenario, contract):
    for req in scenario.get("requires", []):
        if req == "dependency" and not contract.get("dependency"):
            return "no `dependency` declared in .chaos.yaml"
        if req == "proxy" and not (contract.get("dependency") or {}).get("proxy"):
            return "requires `dependency.proxy: true` — the app must address its dependency through toxiproxy"
    return None


def run_scenario(runner, scenario):
    result = {
        "name": scenario["name"],
        "teaches": scenario.get("teaches", ""),
        "steps": [],
        "result": "PASS",
        "failure": None,
    }
    started = time.time()
    for i, spec in enumerate(scenario.get("steps", []), start=1):
        t0 = time.time()
        try:
            ok, detail = runner.step(spec)
        except HarnessError as e:
            ok, detail = False, str(e)
        entry = {
            "n": i,
            "do": spec["do"],
            "result": "pass" if ok else "fail",
            "seconds": round(time.time() - t0, 1),
        }
        if not ok:
            entry["detail"] = detail
        result["steps"].append(entry)
        if not ok:
            result["result"] = "FAIL"
            result["failure"] = {
                "step": i,
                "do": spec["do"],
                # `note` is the sentence the agent actually reads — it names the behaviour,
                # not the exit code.
                "why": spec.get("note", f"step {i} ({spec['do']}) failed"),
                "detail": detail,
            }
            break
    result["seconds"] = round(time.time() - started, 1)
    return result


def render_markdown(report):
    icon = {"PASS": "✅", "FAIL": "❌", "SKIP": "⏭️"}
    lines = [
        f"## Chaos suite: **{report['result']}**",
        "",
        f"`{report['agent']}` · {report['summary']['passed']} passed · "
        f"{report['summary']['failed']} failed · {report['summary']['skipped']} skipped · "
        f"{report['seconds']}s",
        "",
        "| | Scenario | Teaches | |",
        "|---|---|---|---|",
    ]
    for s in report["scenarios"]:
        lines.append(
            f"| {icon[s['result']]} | `{s['name']}` | {s.get('teaches', '')} | "
            f"{s.get('skipReason') or (s['failure']['why'] if s.get('failure') else '')} |"
        )
    failures = [s for s in report["scenarios"] if s["result"] == "FAIL"]
    if failures:
        lines += ["", "### What to fix", ""]
        for s in failures:
            f = s["failure"]
            lines += [
                f"**`{s['name']}`** — failed at step {f['step']} (`{f['do']}`)",
                "",
                f"> {f['why']}",
                "",
                "```",
                (f.get("detail") or "").strip()[:1200] or "(no output)",
                "```",
                "",
            ]
    else:
        lines += ["", "All selected scenarios passed. Open the PR."]
    return "\n".join(lines)


def install_chart(cluster):
    code, out, err = run([
        "helm", "--kubeconfig", cluster.kubeconfig, "upgrade", "--install",
        "chaos", str(REPO / "charts" / "chaos"),
        "--namespace", CHAOS_NS, "--create-namespace", "--wait", "--timeout", "3m",
    ], timeout=300)
    if code != 0:
        raise HarnessError(f"could not install charts/chaos into the vCluster:\n{err or out}")


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("agent", help="pool member name, e.g. pool-20")
    ap.add_argument("scenarios", nargs="*", help="scenario names (default: the `default` suite)")
    ap.add_argument("--suite", default="default", choices=["default", "extended", "all"])
    ap.add_argument("--workspace", default=None)
    ap.add_argument("--kubeconfig", default=None)
    ap.add_argument("--json-out", default=None, help="also write the report to this path")
    args = ap.parse_args()

    started = time.time()
    report = {
        "agent": args.agent,
        "startedAt": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "result": "FAIL",
        "scenarios": [],
        "summary": {"passed": 0, "failed": 0, "skipped": 0},
    }

    try:
        kubeconfig = Path(args.kubeconfig or KUBECONFIG_DIR / f"{args.agent}.host.yaml")
        if not kubeconfig.exists():
            raise HarnessError(
                f"no kubeconfig at {kubeconfig}. `just provision` writes both a "
                f"host.docker.internal and a 127.0.0.1 variant; the suite runs on the host so "
                f"it needs the latter."
            )
        workspace = Path(args.workspace or WORKSPACE_DIR / args.agent)
        if not workspace.exists():
            raise HarnessError(f"no workspace at {workspace} — has this member been claimed?")

        cluster = Cluster(kubeconfig)
        cluster.kubectl("version", "--output=json", timeout=30)

        contract = load_contract(workspace)
        scenarios = load_scenarios(args.scenarios, args.suite)
        install_chart(cluster)

        runner = Runner(cluster, contract, workspace, args.agent)
        report["namespace"] = contract["namespace"]

        for scenario in scenarios:
            skip = scenario_skip_reason(scenario, contract)
            if skip:
                report["scenarios"].append({
                    "name": scenario["name"], "teaches": scenario.get("teaches", ""),
                    "result": "SKIP", "skipReason": skip, "steps": [],
                })
                report["summary"]["skipped"] += 1
                continue
            try:
                result = run_scenario(runner, scenario)
            finally:
                runner.cleanup()
            report["scenarios"].append(result)
            report["summary"]["passed" if result["result"] == "PASS" else "failed"] += 1

        report["result"] = "PASS" if report["summary"]["failed"] == 0 else "FAIL"

    except HarnessError as e:
        report["result"] = "ERROR"
        report["error"] = str(e)
        report["seconds"] = round(time.time() - started, 1)
        report["markdown"] = (
            "## Chaos suite: **could not run**\n\n"
            "This is a harness problem, not a verdict on the code.\n\n"
            f"```\n{e}\n```"
        )
        print(json.dumps(report, indent=2))
        if args.json_out:
            Path(args.json_out).write_text(json.dumps(report, indent=2))
        return 2

    report["seconds"] = round(time.time() - started, 1)
    report["markdown"] = render_markdown(report)
    print(json.dumps(report, indent=2))
    if args.json_out:
        Path(args.json_out).write_text(json.dumps(report, indent=2))
    return 0 if report["result"] == "PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
