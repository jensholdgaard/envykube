import { tool } from "@opencode-ai/plugin";
import { $ } from "bun";

export default tool({
  description:
    "Provision a new agent vCluster. Creates an isolated virtual cluster with ResourceQuota, LimitRange, pinned NodePort, and static kubeconfig. Returns structured JSON with the agent's name, namespace, nodePort, and kubeconfig path.",
  args: {
    name: tool.schema.string().describe("DNS-label-safe agent name (e.g. my-agent)"),
    index: tool.schema.number().describe("Numeric index for port assignment (NodePort = 30000 + index)"),
  },
  async execute(args, context) {
    const result =
      await $`just provision ${args.name} ${args.index}`.cwd(context.worktree).nothrow().quiet();

    const output = result.stdout + "\n" + result.stderr;

    const nameMatch = output.match(/Provisioning (\S+)/);
    const nsMatch = output.match(/ns=([^\s,)]+)/);
    const portMatch = output.match(/NodePort=(\d+)/);
    const kcMatch = output.match(/Kubeconfig:\s*(\S+)/);

    if (result.exitCode !== 0) {
      return {
        success: false,
        error: `Provisioning failed (exit ${result.exitCode})`,
        output: output.trim(),
      };
    }

    return {
      success: true,
      agent: nameMatch ? nameMatch[1] : args.name,
      namespace: nsMatch ? nsMatch[1] : `vc-${args.name}`,
      nodePort: portMatch ? Number(portMatch[1]) : 30000 + args.index,
      kubeconfig: kcMatch ? kcMatch[1] : null,
    };
  },
});
