import { tool } from "@opencode-ai/plugin";
import { $ } from "bun";

export default tool({
  description:
    "Tear down an agent's vCluster, namespace, DevContainer, workspace, kubeconfig, and opencode config. Best-effort — partial state cleans up cleanly. Returns structured JSON with status.",
  args: {
    name: tool.schema.string().describe("Agent name to deprovision (e.g. my-agent)"),
  },
  async execute(args, context) {
    const result =
      await $`just deprovision ${args.name}`.cwd(context.worktree).nothrow().quiet();

    const output = result.stdout + "\n" + result.stderr;
    const tornDown = output.includes("torn down");

    return {
      success: tornDown || result.exitCode === 0,
      agent: args.name,
      status: tornDown ? "torn down" : "partial — check output",
      output: output.trim(),
    };
  },
});
