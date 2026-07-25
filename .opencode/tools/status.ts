import { tool } from "@opencode-ai/plugin";
import { $ } from "bun";

export default tool({
  description:
    "Show host cluster health: k3d node list and kubectl node details. Returns structured JSON with node information.",
  args: {},
  async execute(_args, context) {
    const result = await $`just status`.cwd(context.worktree).nothrow().quiet();
    const output = result.stdout;

    const nodes: Array<{
      name: string;
      status: string;
      roles?: string;
      version?: string;
      internalIp?: string;
      externalIp?: string;
      osImage?: string;
      kernel?: string;
    }> = [];

    const sections = output.split("---");
    if (sections.length >= 2) {
      const k3dLines = sections[0].trim().split("\n");
      const k8sLines = sections[1].trim().split("\n");

      // Parse kubectl nodes output (skip header)
      for (let i = 1; i < k8sLines.length; i++) {
        const line = k8sLines[i].trim();
        if (!line) continue;
        const parts = line.split(/\s+/);
        if (parts.length >= 5) {
          nodes.push({
            name: parts[0],
            status: parts[1],
            roles: parts[2],
            version: parts[4],
            internalIp: parts[5] || undefined,
          });
        }
      }
    }

    return {
      success: result.exitCode === 0,
      nodes,
      raw: output.trim(),
    };
  },
});
