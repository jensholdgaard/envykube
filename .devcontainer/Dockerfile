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

# kubectl-mcp-server (Kubernetes MCP)
RUN npm install -g kubectl-mcp-server

# uv (Python package manager — for mcp-grafana)
RUN curl -LsSf https://astral.sh/uv/install.sh | sh
ENV PATH="/root/.local/bin:${PATH}"

# mcp-grafana (Grafana MCP)
RUN uv tool install mcp-grafana
ENV PATH="/root/.local/share/uv/tools/mcp-grafana/bin:${PATH}"

# Rust toolchain + mnemoria CLI (for oc-mnemoria shared agent memory)
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"
RUN cargo install mnemoria

# Opencode plugins (OTel telemetry export + shared agent memory)
RUN npm install -g @devtheops/opencode-plugin-otel oc-mnemoria

# OpenChamber — web UI for OpenCode (browser-based agent access)
RUN npm install -g @openchamber/web
COPY start-openchamber.sh /usr/local/bin/start-openchamber
RUN chmod +x /usr/local/bin/start-openchamber

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
    && chown -R agent:agent /home/agent
USER agent
WORKDIR /workspace

ENV KUBECONFIG=/home/agent/.kube/config
ENV PATH="/root/.local/bin:/root/.local/share/uv/tools/mcp-grafana/bin:/root/.cargo/bin:/root/.opencode/bin:/home/agent/.local/bin:${PATH}"
