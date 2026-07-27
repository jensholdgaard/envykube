# Documentation Conventions

Rules for coding agents that update this repository's documentation.

## Context strategy

Documents are split into 39 small files (avg 95 lines, largest 236). Never read the entire
`docs/` directory. Always use this two-step approach:

1. **Read `docs/SUMMARY.md`** — one file (~60 lines). Each entry has a one-line description
   of what the file covers. Find the file you need from the description.
2. **Read only that file**. Cross-references within a file point to other specific files —
   follow only those you actually need.

## When to update which file

| Code change | Doc file to check |
|---|---|
| Listener in `crates/pool-manager/src/listeners/` | `docs/03-components/listeners/<name>.md` |
| Chaos scenario in `chaos/scenarios/` | `docs/06-reference/chaos-scenarios.md` |
| Justfile recipe | `docs/04-operations/justfile-reference.md` |
| Event variant in `event.rs` | `docs/06-reference/event-catalog.md` |
| Config field in `config.rs` | `docs/03-components/pool-manager.md` |
| Forgejo API method | `docs/03-components/forgejo-client.md` |
| Chart template | `docs/03-components/charts.md` |

## Style rules

- **One concept per file**: Split, don't append. A new listener gets its own file and a
  line in `SUMMARY.md` with a one-line description.
- **Mermaid for flows**: New interactions get a Mermaid diagram. Mermaid 11 is stricter
  about syntax: no `()` in edge labels, no `:` immediately before digits in node labels.
- **Verify with `just docs-serve`**: After any doc change, confirm the page renders and
  Mermaid diagrams load at `http://localhost:3000`.
- **Validate claims**: Every claim about code behavior must be verified against the
  actual source before writing. Do not document what you think the code does.
- **Add to SUMMARY.md**: New files need a line in `docs/SUMMARY.md` with a one-line
  description so agents can navigate without opening each file.
