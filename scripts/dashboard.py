#!/usr/bin/env python3
"""Generate an agent dashboard HTML page listing all running agent DevContainers."""
import subprocess, json, os, sys, time

WORKSPACES = os.path.expanduser("~/.local/share/agent-workspaces")

def running_agents():
    """Find running DevContainers and return list of {name, port, workspace}."""
    proc = subprocess.run(
        ["docker", "ps", "--filter", "label=devcontainer.local_folder",
         "--format", '{{.Label "devcontainer.local_folder"}}\t{{.Ports}}'],
        capture_output=True, text=True
    )
    agents = []
    for line in proc.stdout.strip().split("\n"):
        if not line:
            continue
        parts = line.split("\t")
        workspace = parts[0]
        ports = parts[1] if len(parts) > 1 else ""
        name = os.path.basename(workspace)

        # Extract host port from "0.0.0.0:31001->3000/tcp"
        ui_port = None
        for mapping in ports.split(", "):
            if "->3000/tcp" in mapping:
                ui_port = mapping.split(":")[1].split("->")[0]
                break

        agents.append({"name": name, "port": ui_port, "workspace": workspace})
    return sorted(agents, key=lambda a: a["name"])


def html(agents):
    rows = []
    for i, a in enumerate(agents):
        link = f'http://localhost:{a["port"]}' if a["port"] else None
        row = f'''
            <tr>
                <td class="n">{i + 1}</td>
                <td>{a["name"]}</td>
                <td>{a["port"] or "—"}</td>
                <td>
                    {f'<a href="{link}" target="_blank">open &rarr;</a>' if link else '<span class="dim">not exposed</span>'}
                </td>
            </tr>'''
        rows.append(row)

    return f'''<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Agent Dashboard</title>
<style>
  * {{ margin: 0; padding: 0; box-sizing: border-box; }}
  body {{
    background: #0d1117; color: #c9d1d9; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", monospace;
    padding: 2rem;
  }}
  h1 {{ font-size: 1.2rem; color: #58a6ff; margin-bottom: 1.5rem; }}
  table {{ width: 100%; border-collapse: collapse; }}
  th, td {{ text-align: left; padding: 0.6rem 0.8rem; border-bottom: 1px solid #21262d; }}
  th {{ color: #8b949e; font-weight: 600; font-size: 0.8rem; text-transform: uppercase; }}
  .n {{ color: #484f58; }}
  a {{ color: #58a6ff; text-decoration: none; }}
  a:hover {{ text-decoration: underline; }}
  .dim {{ color: #484f58; }}
  .footer {{ margin-top: 2rem; font-size: 0.75rem; color: #484f58; }}
  .footer span {{ color: #3fb950; }}
</style>
<meta http-equiv="refresh" content="30">
</head>
<body>
<h1>&#9630; Agentic Kubernetes Platform — Dashboard</h1>
<table>
  <thead><tr><th>#</th><th>Agent</th><th>Port</th><th>OpenChamber</th></tr></thead>
  <tbody>
    {"".join(rows) if rows else '<tr><td colspan="4" class="dim">No agents running. Run <code>just workspace</code> to launch one.</td></tr>'}
  </tbody>
</table>
<div class="footer"><span>&#9679;</span> {len(agents)} agent{"s" if len(agents) != 1 else ""} running &middot; auto-refreshes every 30s</div>
</body>
</html>'''


if __name__ == "__main__":
    agents = running_agents()
    path = os.path.join(WORKSPACES, "dashboard.html")
    with open(path, "w") as f:
        f.write(html(agents))
    print(f"Dashboard → file://{path}")
    print(f"  {len(agents)} agent(s) running")
    for a in agents:
        print(f"    {a['name']} → http://localhost:{a['port']}")
