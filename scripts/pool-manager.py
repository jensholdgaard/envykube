#!/usr/bin/env python3
"""
Warm-pool reconciler — maintains a pool of idle vClusters, claims one per
Forgejo issue labelled "ready", destroys the vCluster when the issue is
closed, and automatically refills to keep idle >= minAvailable.

Each agent gets an ephemeral Forgejo identity (pool-<index>) for commenting,
PR creation, and issue management. The identity is deleted on teardown.

Start:  just pool

State file: ~/.local/share/agent-pool/state.json
Config:    scripts/pool-config.json
Secrets:   .env (FORGEJO_USER, FORGEJO_PASSWORD)
"""

import base64
import json
import os
import subprocess
import sys
import time
import urllib.request
import urllib.error
from pathlib import Path


# ── helpers ──────────────────────────────────────────────────────────────────

def sh(cmd: list[str], **kwargs) -> subprocess.CompletedProcess:
    """Run a command, return CompletedProcess. Fatal on error unless check=False.
    All kubectl calls get a 30s timeout to prevent pool-manager hangs."""
    check = kwargs.pop("check", True)
    if "cwd" not in kwargs:
        kwargs["cwd"] = SCRIPT_DIR.parent
    if "timeout" not in kwargs and "kubectl" in cmd[0]:
        kwargs["timeout"] = 30
    return subprocess.run(cmd, capture_output=True, text=True, check=check, **kwargs)


def _api(method: str, path: str, data: dict | None = None,
         cfg: dict | None = None, auth64: str | None = None):
    """Call Forgejo API. Returns (status, body_dict)."""
    c = cfg or CONFIG
    url = f"{c['forgejoApi']}{path}"
    body = json.dumps(data).encode() if data else None
    req = urllib.request.Request(url, data=body, method=method)
    req.add_header("Content-Type", "application/json")
    req.add_header("Authorization", f"Basic {auth64 or c['forgejoAuth64']}")
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            raw = resp.read().decode() or "{}"
            try:
                return resp.status, json.loads(raw)
            except json.JSONDecodeError:
                return resp.status, {"_raw": raw}
    except urllib.error.HTTPError as e:
        raw = e.read().decode() or "{}"
        try:
            return e.code, json.loads(raw)
        except json.JSONDecodeError:
            return e.code, {"_raw": raw}


# ── config ───────────────────────────────────────────────────────────────────

SCRIPT_DIR = Path(__file__).resolve().parent
CONFIG = {}


def load_config():
    global CONFIG

    cfg_path = SCRIPT_DIR / "pool-config.json"
    with open(cfg_path) as f:
        cfg = json.load(f)

    cfg["stateDir"] = Path(os.path.expanduser(cfg["stateDir"]))

    env_file = SCRIPT_DIR.parent / ".env"
    if env_file.exists():
        for line in env_file.read_text().splitlines():
            line = line.strip()
            if line and not line.startswith("#") and "=" in line:
                k, v = line.split("=", 1)
                os.environ.setdefault(k, v)

    user = os.environ.get("FORGEJO_USER", "adminadmin")
    passwd = os.environ.get("FORGEJO_PASSWORD", "adminadmin")
    cfg["forgejoAuth64"] = base64.b64encode(f"{user}:{passwd}".encode()).decode()

    if os.environ.get("POOL_MIN_AVAILABLE"):
        cfg["minAvailable"] = int(os.environ["POOL_MIN_AVAILABLE"])
    if os.environ.get("POOL_BASE"):
        cfg["poolBase"] = int(os.environ["POOL_BASE"])
    if os.environ.get("POOL_REPO"):
        cfg["repo"] = os.environ["POOL_REPO"]
    if os.environ.get("POOL_POLL_INTERVAL"):
        cfg["pollInterval"] = int(os.environ["POOL_POLL_INTERVAL"])

    CONFIG = cfg
    return cfg


# ── state ────────────────────────────────────────────────────────────────────

def state_path() -> Path:
    return CONFIG["stateDir"] / "state.json"


def load_state() -> dict:
    p = state_path()
    if p.exists():
        return json.loads(p.read_text())
    return {"members": []}


def save_state(state: dict):
    p = state_path()
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(json.dumps(state, indent=2) + "\n")


def idle_count(state: dict) -> int:
    return sum(1 for m in state["members"] if m["status"] == "idle")


def next_index(state: dict) -> int:
    base = CONFIG["poolBase"]
    used = {m["index"] for m in state["members"]}
    i = base
    while i in used:
        i += 1
    return i


# ── Forgejo agent identity ───────────────────────────────────────────────────

def _agent_auth(index: int) -> str:
    """Basic auth header for pool-{index} agent user."""
    pw = f"pool-agent-{index}-pass"
    return base64.b64encode(f"pool-{index}:{pw}".encode()).decode()


def create_agent_identity(index: int) -> tuple[str, str]:
    """Create a Forgejo user for the agent and grant write access to the repo.
    Returns (username, password)."""
    name = f"pool-{index}"
    pw = f"pool-agent-{index}-pass"
    print(f"  → creating Forgejo identity: {name}")

    # delete stale user first if exists (idempotent)
    _api("DELETE", f"/admin/users/{name}", cfg=CONFIG)

    s, d = _api("POST", "/admin/users", {
        "username": name,
        "email": f"{name}@agent.local",
        "password": pw,
        "login_name": name,
        "must_change_password": False,
    }, cfg=CONFIG)

    if s not in (200, 201):
        print(f"  ✗ failed to create user {name}: {s} {d}")
        return name, pw

    # grant write access to the repo
    owner, repo = CONFIG["repo"].split("/")
    s, _ = _api("PUT", f"/repos/{owner}/{repo}/collaborators/{name}",
                {"permission": "write"}, cfg=CONFIG)
    if s in (200, 201, 204):
        print(f"  ✓ write access granted to {owner}/{repo}")
    else:
        print(f"  ! collab add for {name}: {s}")

    # create access token with all scopes
    agent_auth = _agent_auth(index)
    s, d = _api("POST", f"/users/{name}/tokens",
                {"name": "agent-token", "scopes": ["all"]},
                cfg=CONFIG, auth64=agent_auth)
    if s == 201:
        print(f"  ✓ identity {name} ready")
    else:
        print(f"  ! token creation for {name}: {s}")

    return name, pw


def delete_agent_identity(member: dict):
    """Delete the Forgejo user for a pool member."""
    name = member.get("name", f"pool-{member['index']}")
    s, _ = _api("DELETE", f"/admin/users/{name}", cfg=CONFIG)
    if s == 204:
        print(f"  ✓ deleted Forgejo user {name}")


def write_agent_creds(workspace: str, name: str, index: int):
    """Write .forgejo credential file into the agent workspace."""
    pw = f"pool-agent-{index}-pass"
    creds_path = Path(workspace) / ".forgejo"
    creds_path.write_text(
        f"FORGEJO_USER={name}\n"
        f"FORGEJO_PASSWORD={pw}\n"
        f"FORGEJO_URL=http://forgejo:3000\n"
    )
    # make readable by container agent user
    os.chmod(creds_path, 0o644)
    print(f"  ✓ wrote creds to {creds_path}")


# ── vCluster ops ─────────────────────────────────────────────────────────────

def member_exists(name: str) -> bool:
    r = sh(
        ["kubectl", "get", "ns", f"vc-{name}", "--no-headers", "--ignore-not-found"],
        check=False,
    )
    return bool(r.stdout.strip())


def provision_member(name: str, index: int):
    print(f"  → provisioning {name} (index={index})...")
    r = sh(["just", "provision", name, str(index)])
    if r.returncode != 0:
        print(f"  ✗ provision {name} failed:\n{r.stdout}{r.stderr}")
        return False
    print(f"  ✓ {name} provisioned")
    return True


def destroy_member(member: dict):
    """Teardown a pool member: vCluster + namespace + kubeconfig + workspace + Forgejo user."""
    name = member["name"]
    print(f"  → destroying {name}...")
    r = sh(["just", "deprovision", name], check=False)
    out = r.stdout + r.stderr
    print(f"  {'✓' if 'torn down' in out else '?'} vCluster {name}")
    delete_agent_identity(member)


def claim_member(name: str, index: int, issue: dict) -> tuple[bool, str]:
    """Run just workspace to bind vCluster to repo. Returns (success, workspace_path)."""
    repo_url = CONFIG["repoUrl"]
    print(f"  → claiming {name} (index={index}) with issue #{issue['number']}...")
    r = sh(["just", "workspace", name, str(index), repo_url], check=False)
    out = r.stdout + r.stderr
    ws_path = ""
    for line in out.splitlines():
        if "Workspace ready at" in line:
            ws_path = line.split("Workspace ready at")[-1].strip()
    if r.returncode != 0:
        print(f"  ✗ claim {name} failed:\n{out}")
        return False, ws_path
    print(f"  ✓ {name} claimed (workspace: {ws_path})")
    return True, ws_path


# ── Forgejo issue ops ────────────────────────────────────────────────────────

_label_map: dict[str, int] | None = None

def _load_labels() -> dict[str, int]:
    global _label_map
    _, data = _api("GET", f"/repos/{CONFIG['repo']}/labels", cfg=CONFIG)
    if isinstance(data, list):
        _label_map = {l["name"]: l["id"] for l in data}
    return _label_map or {}


def ensure_labels():
    existing = _load_labels()
    needed = [
        ("ready", "ready to be claimed by an agent"),
        ("in-progress", "agent is working on this"),
        ("review", "agent is done, awaiting human review"),
    ]
    for name, desc in needed:
        if name not in existing:
            print(f"  + creating label '{name}'")
            _api("POST", f"/repos/{CONFIG['repo']}/labels",
                 {"name": name, "color": "#2b7489", "description": desc}, cfg=CONFIG)
    _load_labels()


def get_issues_matching(state: str, labels: list[str]) -> list[dict]:
    label_param = ",".join(labels) if labels else "none"
    status, data = _api(
        "GET",
        f"/repos/{CONFIG['repo']}/issues?state={state}&labels={label_param}",
        cfg=CONFIG,
    )
    if status != 200 or not isinstance(data, list):
        return []
    return data


def label_issue(issue_number: int, label_names: list[str]):
    labels = _load_labels()
    ids = [labels[n] for n in label_names if n in labels]
    if not ids:
        return
    _api("PUT", f"/repos/{CONFIG['repo']}/issues/{issue_number}/labels",
         {"labels": ids}, cfg=CONFIG)


# ── handoff ──────────────────────────────────────────────────────────────────

def send_prompt_to_agent(agent_name: str, oc_port: int, issue: dict, index: int,
                         prompt_override: str | None = None):
    """Send a prompt to the child agent's opencode serve."""
    issue_num = issue["number"]
    issue_title = issue["title"]
    issue_body = issue.get("body", "") or ""
    repo = CONFIG["repo"]
    fj_url = "http://forgejo:3000"
    agent_user = f"pool-{index}"

    if prompt_override:
        prompt = prompt_override
    else:
        # Initial claim: implement the issue, push a branch, create a PR
        prompt = f"""You are {agent_user}, assigned to issue #{issue_num} in {repo}: "{issue_title}".

Issue description:
{issue_body[:2000]}

Steps — complete each step before moving on:
1. Source your credentials: source /workspace/.forgejo
2. Create a branch: cd /workspace; git checkout -b issue-{issue_num}
3. Implement the fix or feature described in the issue. Commit with: git add . && git commit -m "Fixes #{issue_num}: {issue_title.replace(chr(34), '')}" && git push origin issue-{issue_num}
4. Create a PR via curl:
   curl -s -X POST "{fj_url}/api/v1/repos/{repo}/pulls" \\
     -u "$FORGEJO_USER:$FORGEJO_PASSWORD" -H "Content-Type: application/json" \\
     -d '{{"title":"[#{issue_num}] {issue_title.replace(chr(34), '')}","head":"issue-{issue_num}","base":"main","body":"Closes #{issue_num}"}}'
   Save the returned PR number — you'll need it.
5. Post a comment on the issue:
   curl -s -X POST "{fj_url}/api/v1/repos/{repo}/issues/{issue_num}/comments" \\
     -u "$FORGEJO_USER:$FORGEJO_PASSWORD" -H "Content-Type: application/json" \\
     -d '{{"body":"PR ready at {fj_url}/{repo}/pulls/<PR_NUM> — waiting for review."}}'

After completing all steps, report "DONE: steps 1-5 completed, PR #<num> created."

Credentials are at /workspace/.forgejo. This repo is already cloned at /workspace. Start working."""

    print(f"  → sending prompt to {agent_name} for issue #{issue_num}...")

    # retry creating session
    session_id = None
    for attempt in range(6):
        try:
            req = urllib.request.Request(
                f"http://localhost:{oc_port}/api/session",
                data=json.dumps({}).encode(),
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            with urllib.request.urlopen(req, timeout=10) as resp:
                body = json.loads(resp.read().decode())
                session_id = body.get("data", {}).get("id") or body.get("id")
                if session_id:
                    break
        except Exception:
            pass
        wait = (attempt + 1) * 5
        print(f"    session not ready, retrying in {wait}s ({attempt + 1}/6)")
        time.sleep(wait)

    if not session_id:
        print(f"  ✗ could not create session for {agent_name}")
        return False

    prompt_data = {"prompt": {"text": prompt}}
    try:
        req = urllib.request.Request(
            f"http://localhost:{oc_port}/api/session/{session_id}/prompt",
            data=json.dumps(prompt_data).encode(),
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        with urllib.request.urlopen(req, timeout=30) as resp:
            pass
        print(f"  ✓ prompt sent to {agent_name} (session {session_id})")
        return True
    except Exception as e:
        print(f"  ✗ failed to send prompt: {e}")
        return False


# ── pool logic ───────────────────────────────────────────────────────────────

def sanitize_state(state: dict):
    """Remove dead members. Recover stalled 'provisioning' members to 'idle'."""
    dead = []
    for m in state["members"]:
        if not member_exists(m["name"]):
            print(f"  - {m['name']} (vCluster gone, removing from pool)")
            # also clean up orphaned Forgejo user
            delete_agent_identity(m)
            dead.append(m)
        elif m["status"] == "provisioning":
            print(f"  ~ {m['name']} vCluster exists but state is 'provisioning' → marking idle")
            m["status"] = "idle"
            m.pop("issue", None)
    for m in dead:
        state["members"].remove(m)


def reap_closed_issues(state: dict):
    """For every claimed member, if its tracked issue is now closed,
    destroy the vCluster and Forgejo identity."""
    claimed = [m for m in state["members"] if m["status"] == "claimed"]
    if not claimed:
        return

    closed = get_issues_matching("closed", [])
    for m in claimed:
        issue_num = m["issue"]
        name = m["name"]

        is_closed = any(i["number"] == issue_num for i in closed)
        if is_closed:
            print(f"→ issue #{issue_num} is closed — destroying {name}")
            destroy_member(m)
            state["members"].remove(m)
            save_state(state)
            return

        if not member_exists(name):
            print(f"  - {name} vCluster disappeared (issue #{issue_num}) — removing from pool")
            delete_agent_identity(m)
            state["members"].remove(m)
            save_state(state)
            return


def health_check_claimed(state: dict):
    """For claimed members, check PR status, reviews, and new issue comments.
    Send small, targeted prompts for the next action the agent needs to take."""
    claimed = [m for m in state["members"] if m["status"] == "claimed"]
    for m in claimed:
        name = m["name"]
        issue_num = m["issue"]
        idx = m["index"]
        oc_port = 32000 + idx

        # throttle: only check every 3 ticks
        last_check = m.get("lastHealthTick", 0)
        m["lastHealthTick"] = last_check + 1
        if last_check % 3 != 0:
            continue

        # is the agent alive?
        alive = False
        try:
            req = urllib.request.Request(
                f"http://localhost:{oc_port}/api/session",
                data=json.dumps({}).encode(),
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            with urllib.request.urlopen(req, timeout=5) as resp:
                if json.loads(resp.read().decode()).get("data", {}).get("id"):
                    alive = True
        except Exception:
            pass

        if not alive:
            print(f"  ⚠ {name} agent not responding on :{oc_port}")
            continue

        # find open PRs referencing this issue
        s, prs = _api("GET",
                       f"/repos/{CONFIG['repo']}/pulls?state=open&sort=desc",
                       cfg=CONFIG)
        related_pr = None
        if isinstance(prs, list):
            for pr in prs:
                title = pr.get("title", "")
                body = pr.get("body", "")
                if f"#{issue_num}" in f"{title} {body}":
                    related_pr = pr
                    break

        pr_num = related_pr["number"] if related_pr else None

        # --- No PR yet ---
        if not pr_num and not m.get("nudgeCreatePr"):
            print(f"  ⚠ {name} has no PR for issue #{issue_num} — nudging to create one")
            repo = CONFIG["repo"]
            prompt = f"""Create a PR for your work on issue #{issue_num}:
curl -s -X POST "http://forgejo:3000/api/v1/repos/{repo}/pulls" \\
  -u "$FORGEJO_USER:$FORGEJO_PASSWORD" -H "Content-Type: application/json" \\
  -d '{{"title":"[#{issue_num}] Fix","head":"issue-{issue_num}","base":"main","body":"Closes #{issue_num}"}}'
Also comment on the issue saying the PR is ready. Report the PR number when done."""
            send_prompt_to_agent(name, oc_port,
                                 {"number": issue_num, "title": "", "body": ""},
                                 idx, prompt_override=prompt)
            m["nudgeCreatePr"] = True
            save_state(state)
            return

        # --- PR exists, check reviews ---
        if pr_num:
            s, reviews = _api("GET",
                              f"/repos/{CONFIG['repo']}/pulls/{pr_num}/reviews",
                              cfg=CONFIG)
            review_states = []
            if isinstance(reviews, list):
                review_states = [r.get("state", "") for r in reviews]

            # Changes requested
            has_changes_requested = any("REQUEST_CHANGES" in s for s in review_states)
            if has_changes_requested and not m.get("nudgeChangesRequested"):
                print(f"  ⚠ PR #{pr_num} has CHANGES_REQUESTED — nudging {name}")
                prompt = f"""Your PR #{pr_num} has changes requested. Read the review comments, make the requested changes, commit and push to branch issue-{issue_num}. Then comment on issue #{issue_num} that you've addressed the feedback."""
                send_prompt_to_agent(name, oc_port,
                                     {"number": issue_num, "title": "", "body": ""},
                                     idx, prompt_override=prompt)
                m["nudgeChangesRequested"] = True
                save_state(state)
                return

            # Approved
            has_approved = any("APPROVED" in s for s in review_states)
            if has_approved and not m.get("nudgeMerge"):
                print(f"  ✓ PR #{pr_num} is APPROVED — nudging {name} to merge")
                prompt = f"""Your PR #{pr_num} is approved. Merge it:
curl -s -X POST "http://forgejo:3000/api/v1/repos/{repo}/pulls/{pr_num}/merge" \\
  -u "$FORGEJO_USER:$FORGEJO_PASSWORD" -H "Content-Type: application/json" \\
  -d '{{"Do":"merge"}}'
Then label issue #{issue_num} "review" and comment that the PR is merged."""
                send_prompt_to_agent(name, oc_port,
                                     {"number": issue_num, "title": "", "body": ""},
                                     idx, prompt_override=prompt)
                m["nudgeMerge"] = True
                save_state(state)
                return

        # --- Check for new comments (issue + PR) ---
        agent_name = f"pool-{idx}"
        all_external = []

        for comment_src in ([issue_num] + ([pr_num] if pr_num else [])):
            s, comments = _api("GET",
                               f"/repos/{CONFIG['repo']}/issues/{comment_src}/comments",
                               cfg=CONFIG)
            if isinstance(comments, list):
                for c in comments:
                    if c.get("user", {}).get("login", "") != agent_name:
                        all_external.append((f"c-{comment_src}", c))

        # also check PR reviews for general-discussion comments
        if pr_num and isinstance(reviews, list):
            for r in reviews:
                state = r.get("state", "")
                body = r.get("body", "") or ""
                if state not in ("APPROVED", "REQUEST_CHANGES") and body.strip():
                    all_external.append((f"r-{pr_num}", {
                        "id": r.get("id", 0),
                        "user": r.get("user", {}),
                        "body": f"[PR review] {body}",
                    }))

        # track last seen IDs per source
        seen = m.get("lastCommentSeen", {})
        new_comments = []
        for key, c in all_external:
            cid = c["id"]
            if seen.get(key, 0) < cid:
                new_comments.append(c)
                seen[key] = cid

        if new_comments:
            m["lastCommentSeen"] = seen

        if new_comments:
            latest = new_comments[-1]
            m["lastExternalCommentId"] = latest["id"]
            body = latest["body"]
            user = latest["user"]["login"]
            print(f"  → new comment from {user} on issue #{issue_num}: {body[:60]}...")

            prompt = f"""A new comment was posted on issue #{issue_num} by {user}:
> {body[:500]}

Read it carefully. If it contains a requirement change, correction, or request for updates:
1. Implement the requested changes
2. Commit and push to branch issue-{issue_num}
3. Comment on the issue that you've updated the PR

If it's a question, answer it by posting a comment on the issue.

PR #: {pr_num or 'not created yet'}"""
            send_prompt_to_agent(name, oc_port,
                                 {"number": issue_num, "title": body[:60], "body": body},
                                 idx, prompt_override=prompt)
            save_state(state)
            return


def refill_pool(state: dict):
    """Provision new members until total active pool size >= minAvailable."""
    active = sum(1 for m in state["members"] if m["status"] != "failed")
    need = CONFIG["minAvailable"] - active
    if need <= 0:
        return
    print(f"→ refilling: need {need} more (have {active}, want {CONFIG['minAvailable']})")
    for _ in range(need):
        idx = next_index(state)
        name = f"pool-{idx}"
        state["members"].append(
            {"index": idx, "name": name, "status": "provisioning", "nodePort": 30000 + idx}
        )
        save_state(state)
        if provision_member(name, idx):
            for m in state["members"]:
                if m["name"] == name:
                    m["status"] = "idle"
                    break
        else:
            for m in state["members"]:
                if m["name"] == name:
                    m["status"] = "failed"
                    break
        save_state(state)


def process_ready_issues(state: dict, issues: list[dict]):
    """For each ready issue, claim an idle pool member."""
    idle_members = [m for m in state["members"] if m["status"] == "idle"]
    for issue in issues:
        issue_num = issue["number"]
        already = any(
            m.get("issue") == issue_num and m["status"] in ("claimed", "claiming")
            for m in state["members"]
        )
        if already:
            continue
        if not idle_members:
            print(f"  ! no idle members for issue #{issue_num} — will retry after refill")
            continue
        member = idle_members.pop(0)
        member["status"] = "claiming"
        member["issue"] = issue_num
        for k in ("nudgeCreatePr", "nudgeChangesRequested", "nudgeMerge",
                  "lastCommentSeen", "lastHealthTick"):
            member.pop(k, None)
        save_state(state)

        print(f"→ claiming issue #{issue_num} ({issue['title'][:50]}) → {member['name']}")
        label_issue(issue_num, [CONFIG["labels"]["inProgress"]])

        ok, ws_path = claim_member(member["name"], member["index"], issue)
        if ok:
            # create Forgejo identity and write creds to workspace
            agent_user, _ = create_agent_identity(member["index"])
            if ws_path:
                write_agent_creds(ws_path, agent_user, member["index"])
            member["status"] = "claimed"
            save_state(state)
            send_prompt_to_agent(member["name"], 32000 + member["index"],
                                 issue, member["index"])
        else:
            member["status"] = "failed"
            save_state(state)


# ── main ─────────────────────────────────────────────────────────────────────

def run_once():
    """Single reconciler tick: reap → sanitize → refill → process ready."""
    state = load_state()

    claimed = sum(1 for m in state["members"] if m["status"] == "claimed")
    total = sum(1 for m in state["members"] if m["status"] != "failed")
    print(f"\n{'─'*60}")
    print(f"tick  idle={idle_count(state)}  claimed={claimed}  "
          f"total={total}/{CONFIG['minAvailable']}  repo={CONFIG['repo']}")
    print(f"{'─'*60}")

    reap_closed_issues(state)
    sanitize_state(state)
    save_state(state)
    refill_pool(state)
    health_check_claimed(state)
    save_state(state)

    ready_issues = get_issues_matching("open", [CONFIG["labels"]["ready"]])
    if ready_issues:
        print(f"→ found {len(ready_issues)} ready issue(s): " +
              ", ".join(f"#{i['number']} {i['title'][:40]}" for i in ready_issues))
        process_ready_issues(state, ready_issues)
    else:
        print("  no ready issues")

    return True


def main():
    load_config()
    state_dir = CONFIG["stateDir"]
    state_dir.mkdir(parents=True, exist_ok=True)

    # PID file — prevent duplicate pool managers
    pid_file = state_dir / "pool.pid"
    if pid_file.exists():
        old_pid = pid_file.read_text().strip()
        try:
            os.kill(int(old_pid), 0)
            print(f"ERROR: pool manager already running (pid {old_pid}). Stop it first.", file=sys.stderr)
            sys.exit(1)
        except (OSError, ValueError):
            pid_file.unlink(missing_ok=True)
    pid_file.write_text(str(os.getpid()))

    print(f"Pool manager: minAvailable={CONFIG['minAvailable']} "
          f"poolBase={CONFIG['poolBase']} repo={CONFIG['repo']}")
    print(f"State dir: {state_dir}")
    print(f"Poll interval: {CONFIG['pollInterval']}s")

    ensure_labels()

    interval = CONFIG["pollInterval"]
    try:
        while True:
            try:
                run_once()
            except KeyboardInterrupt:
                print("\nshutting down.")
                break
            except subprocess.TimeoutExpired as e:
                print(f"\n  ⚠ command timed out: {e.cmd} — will retry next tick", file=sys.stderr)
            except Exception as e:
                print(f"\n✗ error in reconciler tick: {e}", file=sys.stderr)
                import traceback
                traceback.print_exc(file=sys.stderr)
            time.sleep(interval)
    finally:
        pid_file.unlink(missing_ok=True)


if __name__ == "__main__":
    main()
