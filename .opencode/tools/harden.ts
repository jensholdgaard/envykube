import { tool } from "@opencode-ai/plugin";
import { $ } from "bun";

export default tool({
  description:
    "Cap Docker daemon resources with systemd slice limits (MemoryMax, CPUQuota, MemorySwapMax=0). Non-destructive — never kills a running cluster. Returns structured JSON.",
  args: {
    mem: tool.schema.string().optional().describe("Memory limit (default: 48G)"),
    cpu: tool.schema.string().optional().describe("CPU quota percentage (default: 1400)"),
  },
  async execute(args, context) {
    const memArg = args.mem || "48G";
    const cpuArg = args.cpu || "1400";
    const result =
      await $`just harden ${memArg} ${cpuArg}`.cwd(context.worktree).nothrow().quiet();

    const output = result.stdout + "\n" + result.stderr;
    const capped = output.includes("capped") || output.includes("slice");

    return {
      success: capped || result.exitCode === 0,
      memoryLimit: memArg,
      cpuQuota: cpuArg,
      status: capped ? "applied" : "check output",
      output: output.trim(),
    };
  },
});
