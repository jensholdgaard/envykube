import { tool } from "@opencode-ai/plugin";
import { $ } from "bun";

export default tool({
  description:
    "⚠️ DESTRUCTIVE — Delete the entire host k3d cluster, including all agent vClusters, namespaces, and the observability stack. Static kubeconfigs on disk remain. Use with caution. Returns structured JSON confirming the deletion.",
  args: {},
  async execute(_args, context) {
    const result = await $`just nuke`.cwd(context.worktree).nothrow().quiet();

    const output = result.stdout + "\n" + result.stderr;
    const deleted = output.includes("Host cluster deleted");

    return {
      success: deleted || result.exitCode === 0,
      action: "nuke",
      status: deleted ? "host cluster deleted" : "may have partially failed — check output",
      note: "Static kubeconfigs on disk remain — delete manually under ~/.local/share/agent-kubeconfigs/ if needed.",
      output: output.trim(),
    };
  },
});
