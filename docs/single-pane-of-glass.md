# Single Pane of Glass — Tool Evaluation

## Requirement

One browser page that shows all running AI coding agents across isolated DevContainers,
each with their own vCluster. Click an agent to open its web UI.

## Evaluated & Rejected

| Tool | Stars | Why rejected |
|------|-------|-------------|
| **CodeNomad** | 450+ | Spawns its own `opencode serve` subprocesses locally. Cannot connect to external/remote OpenCode servers. Per-DevContainer only — same model as OpenChamber. Multi-instance tabs = within one workspace, not across DevContainers. |
| **TermHive** | 64 | Connects to multiple agents via PTY/tmux sessions. xterm.js terminals in browser. But: designed for a single machine running multiple CLIs in different directories, not Docker-isolated DevContainers. |
| **Stoneforge** | 160 | Director/Worker/Steward orchestration model. OpenCode provider support. But: autonomous (no human approval gates), experimental, designed for orchestrated pipelines not independent agents. |
| **JAT** | 243 | Full web IDE with multi-agent task management. SvelteKit monolith, 4300+ commits. Overkill — adds task scheduling, Slack/Telegram, code editor. More of a workflow tool than an agent dashboard. |
| **amux** | 293 | Kanban + mobile app + self-healing watchdog. Claude/Codex/Gemini only — no OpenCode support. Would need CLI adapter. |
| **faber** | 8 | TUI fleet manager for OpenCode in git worktrees. Terminal-only, no browser UI. |
| **agent-of-empires** | — | tmux-based TUI for multiple OpenCode sessions. Terminal-only, no browser UI. |
| **Deck** | — | Docker sandboxes + noVNC monitoring. Overhead of per-agent VNC sessions. |

## What we built instead

A 50-line Python script that:

1. Queries `docker ps` for running DevContainers with `devcontainer.local_folder` labels
2. Extracts the host port from each container's `-p` mapping (`op_base + index → localhost:31001`, etc.)
3. Generates a self-refreshing HTML page with agent names and clickable OpenChamber links
4. OpenChamber provides the per-agent UI (chat, diffs, git, terminal)

Recipe: `just dashboard`

## Why this is the right fit

- **Each agent is independent** — they deploy to different vClusters, work on different branches. There's no shared task queue, no orchestration dependency, no swarm logic.
- **No multi-session tool needed** — the agents don't collaborate in real-time. They push to Forgejo when done. The human reviews and merges.
- **The dashboard is coordination, not orchestration** — it answers "which agents are running and where do I click to talk to them?" Nothing more.
- **All evaluated tools add orchestration complexity without benefit** for independent trunk-based agents.
