#!/usr/bin/env python3
"""Extract dashboard ConfigMaps from kubectl JSON stdin, output one JSON line per dashboard.
Includes the Grafana uid from the dashboard model for delete-tracking."""
import json
import sys

dashboards = {}
for cm in json.load(sys.stdin).get("items", []):
    ann = cm.get("metadata", {}).get("annotations", {})
    if ann.get("platform.agentic-k8s.dev/dashboard") != "true":
        continue
    data = cm.get("data", {})
    if "json" not in data or "title" not in data:
        print(f'  ⚠ {cm["metadata"]["name"]}: missing title or json key', file=sys.stderr)
        continue
    djson = json.loads(data["json"])
    uid = djson.get("uid", "")
    if not uid:
        print(f'  ⚠ {cm["metadata"]["name"]}: dashboard json is missing "uid"', file=sys.stderr)
        continue
    dashboards[data["title"]] = {"uid": uid, "json": djson}

for title, d in dashboards.items():
    print(json.dumps({"title": title, "uid": d["uid"], "json": d["json"]}))
