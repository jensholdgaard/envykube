FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive

# Base toolchain
RUN apt-get update && apt-get install -y --no-install-recommends \
    curl ca-certificates git unzip jq \
    build-essential python3 python3-pip python3-venv \
    && rm -rf /var/lib/apt/lists/*

# Node.js (for kubectl-mcp-server)
RUN curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
    && apt-get install -y nodejs \
    && rm -rf /var/lib/apt/lists/*

# kubectl
RUN curl -Lo /usr/local/bin/kubectl "https://dl.k8s.io/release/$(curl -Ls https://dl.k8s.io/release/stable.txt)/bin/linux/amd64/kubectl" \
    && chmod +x /usr/local/bin/kubectl

# Helm
RUN curl -fsSL https://raw.githubusercontent.com/helm/helm/main/scripts/get-helm-3 | bash

# vcluster CLI
RUN curl -Lo /tmp/vcluster "https://github.com/loft-sh/vcluster/releases/latest/download/vcluster-linux-amd64" \
    && install -m 0755 /tmp/vcluster /usr/local/bin/vcluster \
    && rm /tmp/vcluster

# kubectl-mcp-server (Kubernetes MCP) — install the real Python package via uv,
# NOT the npm shim which pip-installs at runtime and dies on PEP 668.
# uv (Python package manager — for both MCP servers)
RUN curl -LsSf https://astral.sh/uv/install.sh | sh
ENV UV_TOOL_DIR=/opt/uv/tools UV_TOOL_BIN_DIR=/usr/local/bin
ENV PATH="/root/.local/bin:${PATH}"

# mcp-grafana (Grafana MCP)
RUN uv tool install mcp-grafana

# kubectl-mcp-server (Kubernetes MCP) — installed via pip in a venv, binaries symlinked.
# uv tool install fails on this package (non-standard entry points).
RUN python3 -m venv /opt/uv/tools/kubectl-mcp-tool \
    && /opt/uv/tools/kubectl-mcp-tool/bin/pip install -q kubectl-mcp-tool \
    && ln -s /opt/uv/tools/kubectl-mcp-tool/bin/kubectl-mcp /usr/local/bin/kubectl-mcp \
    && ln -s /opt/uv/tools/kubectl-mcp-tool/bin/kubectl-mcp /usr/local/bin/kubectl-mcp-server \
    && chmod -R a+rX /opt/uv/tools

# Opencode plugin (OTel telemetry export)
RUN npm install -g @devtheops/opencode-plugin-otel

# OpenChamber — web UI for OpenCode (browser-based agent access)
RUN npm install -g @openchamber/web
COPY start-openchamber.sh /usr/local/bin/start-openchamber
RUN chmod +x /usr/local/bin/start-openchamber

# curl shim — injects --resolve for .localhost hosts so the verify loop works.
# curl (since 8.4.0) hardcodes RFC 6761: any host ending in .localhost resolves
# to loopback internally, bypassing /etc/hosts and NSS entirely. This shim
# detects .localhost URLs and rewrites them with --resolve before exec'ing
# the real curl. Non-.localhost URLs pass through unchanged.
COPY curl-shim.sh /usr/local/bin/curl
RUN chmod +x /usr/local/bin/curl

# opencode CLI
RUN OPENCODE_VERSION=$(curl -s https://api.github.com/repos/anomalyco/opencode/releases/latest | grep tag_name | cut -d'"' -f4) \
    && curl -Lo /tmp/opencode.tar.gz "https://github.com/anomalyco/opencode/releases/download/${OPENCODE_VERSION}/opencode-linux-x64.tar.gz" \
    && tar xzf /tmp/opencode.tar.gz -C /usr/local/bin \
    && chmod +x /usr/local/bin/opencode \
    && rm /tmp/opencode.tar.gz
ENV PATH="/root/.opencode/bin:${PATH}"

# Agent user — non-root
RUN useradd -m -s /bin/bash agent \
    && mkdir -p /home/agent/.local/state /home/agent/.local/share/opencode/repos \
    && mkdir -p /home/agent/.config/opencode /home/agent/.kube \
    && chown -R agent:agent /home/agent \
    && echo 'cd /workspace' >> /home/agent/.bashrc
USER agent
RUN git config --global --add safe.directory /workspace \
    && git config --global user.email "agent@agentic-platform.local" \
    && git config --global user.name "Agent"
# Build-time smoke test: verify both MCP binaries are installed and reachable
# by the agent user. mcp-grafana gets a full JSON-RPC test. kubectl-mcp-server
# is a symlink to a venv binary — verify the target is executable.
RUN echo '{"jsonrpc":"2.0","method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke","version":"1"}},"id":1}' \
    | timeout 15 /usr/local/bin/mcp-grafana 2>/dev/null \
    | python3 -c 'import json,sys; r=json.load(sys.stdin); assert "result" in r, f"mcp-grafana: {r}"' \
    && echo "  mcp-grafana OK" \
    && test -x /usr/local/bin/kubectl-mcp-server \
    && echo "  kubectl-mcp-server OK"

ENV KUBECONFIG=/home/agent/.kube/config
ENV PATH="/usr/local/bin:/home/agent/.local/bin:${PATH}"
