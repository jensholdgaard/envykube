import { tool } from "@opencode-ai/plugin";
import { $ } from "bun";

export default tool({
  description:
    "Claim an agent — clone its repo from Forgejo, build and launch a DevContainer with per-agent opencode config. Returns structured JSON with workspace path, OpenChamber UI URL, OpenCode API URL, and port numbers. Pre-requisite: agent must already be provisioned via the provision tool.",
  args: {
    name: tool.schema.string().describe("Agent name matching the provisioned agent (e.g. my-agent)"),
    index: tool.schema.number().describe("Numeric index matching the provision step (used for port assignment)"),
    repoUrl: tool.schema.string().describe("Forgejo clone URL (e.g. http://forgejo.platform.localhost/user/repo.git)"),
  },
  async execute(args, context) {
    const result =
      await $`just workspace ${args.name} ${args.index} ${args.repoUrl}`.cwd(context.worktree).nothrow().quiet();

    const output = result.stdout + "\n" + result.stderr;

    const wsMatch = output.match(/Workspace ready at (\S+)/);
    const chamberMatch = output.match(/OpenChamber:\s+(\S+)/);
    const apiMatch = output.match(/OpenCode API:\s+(\S+)/);

    if (result.exitCode !== 0) {
      return {
        success: false,
        error: `Workspace setup failed (exit ${result.exitCode})`,
        output: output.trim(),
      };
    }

    return {
      success: true,
      workspace: wsMatch ? wsMatch[1] : null,
      openchamberUrl: chamberMatch ? chamberMatch[1] : null,
      opencodeApi: apiMatch ? apiMatch[1] : null,
      uiPort: 31000 + args.index,
      apiPort: 32000 + args.index,
      output: output.trim(),
    };
  },
});
