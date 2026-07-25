import { tool } from "@opencode-ai/plugin";
import { $ } from "bun";

export default tool({
  description:
    "List all active agent vClusters and their namespaces. Returns structured JSON with cluster names, namespaces, and status.",
  args: {},
  async execute(_args, context) {
    const result = await $`just list`.cwd(context.worktree).nothrow().quiet();
    const output = result.stdout;

    // Parse vcluster list output
    const lines = output.trim().split("\n");
    const vclusters: Array<{ name: string; namespace: string; status?: string }> = [];
    let inVclusterTable = true;

    for (const line of lines) {
      if (line === "---" || line.startsWith("NAME")) {
        if (line === "---") inVclusterTable = false;
        continue;
      }
      if (line === "no agent namespaces" || line.trim() === "") continue;

      if (inVclusterTable) {
        const parts = line.trim().split(/\s+/);
        if (parts.length >= 2) {
          vclusters.push({ name: parts[0], namespace: parts[1], status: parts[2] });
        }
      }
    }

    const namespaceLines = output.includes("---")
      ? output.split("---")[1]?.trim() || ""
      : "";

    return {
      clusters: vclusters,
      namespaceLines: namespaceLines.split("\n").filter((l) => l.trim() && l !== "no agent namespaces"),
      count: vclusters.length,
      raw: output.trim(),
    };
  },
});
