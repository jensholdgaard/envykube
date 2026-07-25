import { tool } from "@opencode-ai/plugin";
import { $ } from "bun";

export default tool({
  description:
    "Deploy the full observability stack: Loki, Tempo, Mimir monolithic, Grafana, OTel Collector, and Grafana Ingress. Idempotent — safe to re-run. Returns structured JSON with Grafana URL.",
  args: {},
  async execute(_args, context) {
    const result =
      await $`just observability`.cwd(context.worktree).nothrow().quiet();

    const output = result.stdout + "\n" + result.stderr;

    return {
      success: result.exitCode === 0,
      grafanaUrl: "http://grafana.platform.localhost",
      exitCode: result.exitCode,
      output: output.trim(),
    };
  },
});
