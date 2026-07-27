//! The chaos gate: `/chaos` on an issue runs the fault suite against that agent's vCluster.
//!
//! Agent-triggered, operator-executed — the same split as teardown. The agent asks for a run
//! by commenting; the operator shells out to `just chaos-suite`, posts the report **as admin**
//! and sets the verdict label. The agent can read every word of the verdict and cannot forge,
//! weaken or skip one.
//!
//! This exists because the three things agents get wrong — foreseeing errors, idempotency,
//! recovering from invariant states — are all invisible to a happy-path verify loop. A model
//! that has never observed a mid-apply pod kill cannot reason its way to idempotency; one that
//! observes it and gets a named failure back can.

use crate::agent::Agent;
use crate::config::Config;
use crate::event::{Event, IssueNumber};
use crate::forgejo::Forgejo;
use crate::state::{ChaosStatus, MAX_CHAOS_RUNS};
use super::{EventRx, SharedState};
use std::sync::Arc;
use tokio::process::Command;
use tracing::{error, info, warn};

/// A suite run involves several full deploys under fault; it is minutes, not seconds.
const SUITE_TIMEOUT_SECS: u64 = 1800;

pub fn parse_command(body: &str) -> Option<Vec<String>> {
    let first = body.trim().lines().next()?.trim();
    let rest = first.strip_prefix("/chaos")?;
    // "/chaosbeep" is not the command.
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        return None;
    }
    Some(rest.split_whitespace().map(str::to_string).collect())
}

pub async fn run(mut events: EventRx, state: SharedState, cfg: Arc<Config>) {
    let agent = Agent::new();
    let state_path = crate::config::resolve_state_path(&cfg.state_dir);

    loop {
        let event = match events.recv().await {
            Ok(e) => e,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                warn!(skipped = n, "chaos_gate fell behind");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };

        let (issue_number, body) = match event {
            Event::IssueComment {
                issue_number, body, ..
            } => (issue_number, body),
            Event::Shutdown => break,
            _ => continue,
        };

        let Some(scenarios) = parse_command(&body) else {
            continue;
        };

        let member = {
            let st = state.read().await;
            st.find_by_issue(issue_number)
                .map(|m| (m.name.clone(), m.index, m.chaos_status, m.chaos_runs))
        };
        let Some((name, index, status, runs)) = member else {
            let forgejo = Forgejo::new(&cfg);
            let _ = forgejo
                .post_issue_comment(
                    issue_number.0,
                    "`/chaos` ignored — no agent is currently working this issue.",
                )
                .await;
            continue;
        };

        if status == ChaosStatus::Running {
            info!(name, "chaos run already in flight — ignoring duplicate /chaos");
            continue;
        }

        if runs >= MAX_CHAOS_RUNS {
            escalate(&cfg, issue_number, runs).await;
            continue;
        }

        execute(
            &agent, &cfg, &state, &state_path, &name, index, issue_number, scenarios,
        )
        .await;
    }
}

/// A model that cannot work out the fix must hand back to a human rather than burn the pool.
async fn escalate(cfg: &Config, issue: IssueNumber, runs: u64) {
    let forgejo = Forgejo::new(cfg);
    warn!(issue = issue.0, runs, "chaos run cap reached");
    let _ = forgejo
        .post_issue_comment(
            issue.0,
            &format!(
                "## Chaos gate: stopped\n\nThe suite has run **{runs}** times on this issue \
                 without passing, which is the cap. Further `/chaos` requests are ignored \
                 until a human looks at it.\n\nThe failures above are repeating, so the next \
                 attempt is unlikely to differ — this needs a change of approach, not another \
                 iteration."
            ),
        )
        .await;
}

#[allow(clippy::too_many_arguments)]
async fn execute(
    agent: &Agent,
    cfg: &Config,
    state: &SharedState,
    state_path: &std::path::Path,
    name: &str,
    index: u32,
    issue: IssueNumber,
    scenarios: Vec<String>,
) {
    let forgejo = Forgejo::new(cfg);

    {
        let mut st = state.write().await;
        if let Some(m) = st.find_member_mut(name) {
            m.chaos_status = ChaosStatus::Running;
            m.chaos_runs += 1;
        }
        let _ = st.save(state_path);
    }

    let scope = if scenarios.is_empty() {
        "the default suite".to_string()
    } else {
        scenarios.join(", ")
    };
    info!(name, %scope, "running chaos suite for issue #{n}", n = issue.0);
    let _ = forgejo
        .post_issue_comment(
            issue.0,
            &format!("Running the chaos suite against `{name}` ({scope}). This takes a few minutes."),
        )
        .await;

    // Same shell-out convention as vcluster::provision_member — the justfile stays the single
    // definition of what an operation actually does.
    let mut cmd = Command::new("just");
    cmd.arg("chaos-suite").arg(name);
    for s in &scenarios {
        cmd.arg(s);
    }
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(SUITE_TIMEOUT_SECS),
        cmd.output(),
    )
    .await;

    let (code, stdout, stderr) = match output {
        Ok(Ok(out)) => (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        ),
        Ok(Err(e)) => (-1, String::new(), format!("could not run the suite: {e}")),
        Err(_) => (
            -1,
            String::new(),
            format!("the suite did not finish within {SUITE_TIMEOUT_SECS}s"),
        ),
    };

    let report: Option<serde_json::Value> = serde_json::from_str(stdout.trim()).ok();
    let markdown = report
        .as_ref()
        .and_then(|r| r.get("markdown"))
        .and_then(|m| m.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!(
                "## Chaos suite: **could not run**\n\nThis is a harness problem, not a verdict \
                 on the code.\n\n```\n{}\n```",
                stderr.trim().chars().take(1500).collect::<String>()
            )
        });

    // Posted as admin, never via post_comment_as_agent: the verdict must not be attributable
    // to — or forgeable by — the agent being judged.
    let _ = forgejo.post_issue_comment(issue.0, &markdown).await;

    match code {
        0 => {
            set_verdict(&forgejo, cfg, issue, true).await;
            record(state, state_path, name, ChaosStatus::Passed).await;
            info!(name, "chaos suite PASSED for issue #{n}", n = issue.0);
            let prompt = format!(
                "The chaos suite PASSED for issue #{n}. Proceed to open the pull request now \
                 (step 5 of your instructions) and then comment on the issue that it is ready.\n\n\
                 Create the PR:\n\
                 curl -s -X POST \"http://forgejo:3000/api/v1/repos/{repo}/pulls\" \\\n\
                   -u \"$FORGEJO_USER:$FORGEJO_PASSWORD\" -H \"Content-Type: application/json\" \\\n\
                   -d '{{\"title\":\"[#{n}] ...\",\"head\":\"issue-{n}\",\"base\":\"main\",\"body\":\"Closes #{n}\"}}'\n\n\
                 Then comment the PR number on the issue.\n\
                 Remember: pushing new commits after this pass invalidates it — the PR is what locks the pass in.",
                repo = cfg.repo, n = issue.0
            );
            agent.create_and_send(index, name, &prompt).await;
        }
        1 => {
            set_verdict(&forgejo, cfg, issue, false).await;
            record(state, state_path, name, ChaosStatus::Failed).await;
            info!(name, "chaos suite FAILED for issue #{n}", n = issue.0);
            let failures = summarise_failures(report.as_ref());
            let prompt = format!(
                "The chaos suite FAILED for issue #{n}. Do NOT open a pull request yet — \
                 it would be blocked.\n\n\
                 {failures}\n\
                 The full report is a comment on issue #{n}. Read it carefully — each failure \
                 names the behaviour that was wrong, not just an exit code.\n\n\
                 Fix the cause in your code or manifests, then:\n\
                 1. git add . && git commit -m \"Fixes #{n}: ...\" && git push origin issue-{n}\n\
                 2. Trigger the suite again by commenting `/chaos` on the issue:\n\
                   source /workspace/.forgejo\n\
                   curl -s -X POST \"{fj_url}/api/v1/repos/{repo}/issues/{n}/comments\" \\\n\
                     -u \"$FORGEJO_USER:$FORGEJO_PASSWORD\" -H \"Content-Type: application/json\" \\\n\
                     -d '{{\"body\":\"/chaos\"}}'\n\n\
                 If two attempts change nothing, change the approach rather than the details. \
                 After 5 failed runs the gate stops and asks for a human.",
                fj_url = "http://forgejo:3000", repo = cfg.repo, n = issue.0
            );
            agent.create_and_send(index, name, &prompt).await;
        }
        _ => {
            // Exit 2 (or a crash) is the harness failing, not the code. Do not label the work
            // failed and do not tell the agent to go and fix something that isn't broken.
            error!(name, code, stderr = %stderr.trim(), "chaos suite could not run");
            record(state, state_path, name, ChaosStatus::NotRun).await;
            let mut st = state.write().await;
            if let Some(m) = st.find_member_mut(name) {
                m.chaos_runs = m.chaos_runs.saturating_sub(1);
            }
            let _ = st.save(state_path);
        }
    }
}

fn summarise_failures(report: Option<&serde_json::Value>) -> String {
    let Some(scenarios) = report.and_then(|r| r.get("scenarios")).and_then(|s| s.as_array()) else {
        return String::new();
    };
    let mut lines = Vec::new();
    for s in scenarios {
        if s.get("result").and_then(|r| r.as_str()) != Some("FAIL") {
            continue;
        }
        let name = s.get("name").and_then(|n| n.as_str()).unwrap_or("?");
        let why = s
            .get("failure")
            .and_then(|f| f.get("why"))
            .and_then(|w| w.as_str())
            .unwrap_or("");
        lines.push(format!("- **{name}**: {why}"));
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!("What failed:\n{}\n", lines.join("\n"))
    }
}

async fn set_verdict(forgejo: &Forgejo, cfg: &Config, issue: IssueNumber, passed: bool) {
    let (add, remove) = if passed {
        (&cfg.labels.chaos_passed, &cfg.labels.chaos_failed)
    } else {
        (&cfg.labels.chaos_failed, &cfg.labels.chaos_passed)
    };
    forgejo.remove_issue_label(issue.0, remove).await;
    // add, not set: set_issue_labels replaces the whole set and would drop `in-progress`.
    let _ = forgejo.add_issue_labels(issue.0, &[add.clone()]).await;
}

async fn record(
    state: &SharedState,
    state_path: &std::path::Path,
    name: &str,
    status: ChaosStatus,
) {
    let mut st = state.write().await;
    if let Some(m) = st.find_member_mut(name) {
        m.chaos_status = status;
    }
    let _ = st.save(state_path);
}

#[cfg(test)]
mod tests {
    use super::parse_command;

    #[test]
    fn recognises_the_command_and_its_arguments() {
        assert_eq!(parse_command("/chaos"), Some(vec![]));
        assert_eq!(
            parse_command("/chaos reapply-twice oom-squeeze"),
            Some(vec!["reapply-twice".into(), "oom-squeeze".into()])
        );
        // Trailing prose after the first line is ignored, not treated as scenario names.
        assert_eq!(
            parse_command("/chaos\nplease run this"),
            Some(vec![])
        );
    }

    #[test]
    fn ignores_everything_else() {
        assert_eq!(parse_command("running /chaos now"), None);
        assert_eq!(parse_command("/chaosbeep"), None);
        assert_eq!(parse_command("the chaos suite failed"), None);
    }
}
