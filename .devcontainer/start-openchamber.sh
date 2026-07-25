#!/bin/bash
set -euo pipefail

# Start the OpenCode server (if not already running), from the workspace dir
if ! pgrep -f "opencode serve" > /dev/null 2>&1; then
  (cd /workspace && nohup opencode serve --port 4096 --hostname 0.0.0.0 > /tmp/opencode.log 2>&1 &)
  sleep 2
  echo "OpenCode server started on port 4096"
else
  echo "OpenCode server already running"
fi

# Start OpenChamber web UI (if not already running)
if ! pgrep -f "openchamber serve" > /dev/null 2>&1; then
  OPENCODE_HOST=http://localhost:4096 OPENCODE_SKIP_START=true \
    OPENCHAMBER_ALLOW_UNAUTHENTICATED_LAN=true \
    nohup openchamber serve --port 3000 --host 0.0.0.0 > /tmp/openchamber.log 2>&1 &
  sleep 3
  echo "OpenChamber running on port 3000"
else
  echo "OpenChamber already running on port 3000"
fi
