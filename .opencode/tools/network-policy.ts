import { tool } from "@opencode-ai/plugin";
import { $ } from "bun";

export default tool({
  description:
    "Apply default-deny cross-agent NetworkPolicy to isolate an agent's vCluster. Works with any CNI. Returns structured JSON with status.",
  args: {
    name: tool.schema.string().describe("Agent name (e.g. my-agent)"),
  },
  async execute(args, context) {
    const result =
      await $`just network-policy ${args.name}`.cwd(context.worktree).nothrow().quiet();

    const output = result.stdout + "\n" + result.stderr;
    const applied = output.includes("NetworkPolicy applied");

    return {
      success: applied || result.exitCode === 0,
      agent: args.name,
      status: applied ? "applied" : "failed",
      output: output.trim(),
    };
  },
});
