//! Keeps un-chaos-tested work out of review.
//!
//! The prompt tells the agent to run `/chaos` before opening a PR, and that is what a model
//! will usually follow — but "usually" is not a gate. This listener is the part that does not
//! depend on the agent cooperating:
//!
//!   * a PR opened without `chaos-passed` gets a blocking `REQUEST_CHANGES` review;
//!   * a push to an already-passing PR clears the label, because otherwise an agent could
//!     pass the suite on a stub and then push the real implementation.
//!
//! The third layer is outside this process: Forgejo branch protection on `main` with "block
//! merge on rejected reviews". The agent merges with its own credentials, so a server-side
//! rule is the only thing it genuinely cannot route around.

use crate::agent::Agent;
use crate::config::Config;
use crate::event::{Event, IssueNumber, PrNumber};
use crate::forgejo::Forgejo;
use crate::state::ChaosStatus;
use super::{EventRx, SharedState};
use std::sync::Arc;
use tracing::{info, warn};

pub async fn run(mut events: EventRx, state: SharedState, cfg: Arc<Config>) {
    let agent = Agent::new();
    let state_path = crate::config::resolve_state_path(&cfg.state_dir);

    loop {
        let event = match events.recv().await {
            Ok(e) => e,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                warn!(skipped = n, "chaos_enforcer fell behind");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };

        let (pr_number, pushed) = match event {
            Event::PullRequestOpened { pr_number, .. } => (pr_number, false),
            Event::PullRequestSynchronized { pr_number } => (pr_number, true),
            Event::Shutdown => break,
            _ => continue,
        };

        let forgejo = Forgejo::new(&cfg);
        let Some(issue) = issue_for_pr(&forgejo, pr_number).await else {
            continue;
        };

        if pushed {
            invalidate(&forgejo, &cfg, &state, &state_path, issue).await;
        }

        if forgejo
            .issue_has_label(issue.0, &cfg.labels.chaos_passed)
            .await
        {
            info!(pr = pr_number.0, "chaos gate green — PR may proceed to review");
            continue;
        }

        block(&forgejo, &agent, &state, pr_number, issue).await;
    }
}

async fn issue_for_pr(forgejo: &Forgejo, pr_number: PrNumber) -> Option<IssueNumber> {
    let prs = forgejo.get_pull_requests("open").await.ok()?;
    let pr = prs.iter().find(|p| p.number == pr_number.0)?;
    let text = format!("{} {}", pr.title, pr.body.as_deref().unwrap_or(""));
    super::extract_issue_ref(&text)
}

/// A push means the tested tree is no longer the proposed tree.
async fn invalidate(
    forgejo: &Forgejo,
    cfg: &Config,
    state: &SharedState,
    state_path: &std::path::Path,
    issue: IssueNumber,
) {
    if !forgejo
        .issue_has_label(issue.0, &cfg.labels.chaos_passed)
        .await
    {
        return;
    }
    info!(issue = issue.0, "new commits pushed — clearing chaos-passed");
    forgejo
        .remove_issue_label(issue.0, &cfg.labels.chaos_passed)
        .await;
    let _ = forgejo
        .post_issue_comment(
            issue.0,
            "New commits were pushed, so the chaos pass no longer applies to this tree. \
             Comment `/chaos` to re-run the suite before this can go to review.",
        )
        .await;

    let name = {
        let st = state.read().await;
        st.find_by_issue(issue).map(|m| m.name.clone())
    };
    if let Some(name) = name {
        let mut st = state.write().await;
        if let Some(m) = st.find_member_mut(&name) {
            m.chaos_status = ChaosStatus::NotRun;
        }
        let _ = st.save(state_path);
    }
}

async fn block(
    forgejo: &Forgejo,
    agent: &Agent,
    state: &SharedState,
    pr_number: PrNumber,
    issue: IssueNumber,
) {
    warn!(pr = pr_number.0, issue = issue.0, "PR opened without a chaos pass — blocking");

    let body = format!(
        "## Chaos gate: not passed\n\n\
         This PR cannot go to review yet — issue #{n} does not carry `chaos-passed`.\n\n\
         Comment **`/chaos`** on issue #{n}. The operator runs the fault suite against your \
         vCluster and posts the report there. Fix anything it names, push, and run it again; \
         once it passes, this review stops blocking.",
        n = issue.0
    );
    if let Err(e) = forgejo
        .post_review(pr_number.0, "REQUEST_CHANGES", &body)
        .await
    {
        warn!(error = %e, "could not post the blocking review");
    }

    let member = {
        let st = state.read().await;
        st.find_by_issue(issue).map(|m| (m.name.clone(), m.index))
    };
    let Some((name, index)) = member else { return };

    let prompt = format!(
        "Your PR for issue #{n} was opened before the chaos suite passed, so it is blocked. \
         Comment `/chaos` on issue #{n} to run the suite, then address whatever it reports. \
         Do not try to merge until the report says PASS.",
        n = issue.0
    );
    agent.create_and_send(index, &name, &prompt).await;
}
