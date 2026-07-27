use crate::agent::Agent;
use crate::config::Config;
use crate::event::{Event, IssueNumber, PrNumber};
use crate::forgejo::Forgejo;
use super::{SharedState, EventRx};
use std::sync::Arc;
use tracing::{info, warn};

pub async fn run(mut events: EventRx, state: SharedState, cfg: Arc<Config>) {
    let agent = Agent::new();

    loop {
        let event = match events.recv().await {
            Ok(e) => e,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                warn!(skipped = n, "pr_monitor fell behind");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };

        let (pr_number, review_state) = match event {
            Event::PullRequestReview {
                pr_number,
                state: review_state,
                ..
            } => (pr_number, review_state),
            Event::Shutdown => break,
            _ => continue,
        };

        let forgejo = Forgejo::new(&cfg);
        let prs = match forgejo.get_pull_requests("open").await {
            Ok(p) => p,
            Err(_) => continue,
        };

        let pr = prs.iter().find(|p| p.number == pr_number.0);
        let issue_num = pr.and_then(|p| {
            let text = format!("{} {}", p.title, p.body.as_deref().unwrap_or(""));
            super::extract_issue_ref(&text)
        });

        let Some(issue_num) = issue_num else {
            continue;
        };

        let member = {
            let st = state.read().await;
            st.find_by_issue(issue_num).map(|m| (m.name.clone(), m.index))
        };

        let Some((name, index)) = member else {
            continue;
        };

        if review_state.contains("REQUEST_CHANGES") {
            handle_changes_requested(&agent, &state, &name, index, pr_number, issue_num).await;
        } else if review_state.contains("APPROVED") {
            // Second line of defence for the chaos gate. A human can approve a PR before the
            // suite has run; the merge nudge must still not go out, or approval alone would
            // carry untested work to main.
            if !forgejo
                .issue_has_label(issue_num.0, &cfg.labels.chaos_passed)
                .await
            {
                warn!(
                    pr = pr_number.0,
                    "PR approved but the chaos gate is not green — withholding the merge nudge"
                );
                let _ = forgejo
                    .post_issue_comment(
                        issue_num.0,
                        "This PR is approved but the chaos suite has not passed for the current \
                         commits. Comment `/chaos` to run it; the merge instruction follows a pass.",
                    )
                    .await;
                continue;
            }
            handle_approved(&agent, &state, &name, index, pr_number, issue_num, &cfg).await;
        }
    }
}

async fn handle_changes_requested(
    agent: &Agent,
    state: &SharedState,
    name: &str,
    index: u32,
    pr_number: PrNumber,
    issue_num: IssueNumber,
) {
    let should_nudge = {
        let mut st = state.write().await;
        let Some(m) = st.find_member_mut(name) else { return };
        if m.nudge_changes_requested {
            false
        } else {
            m.nudge_changes_requested = true;
            true
        }
    };

    if !should_nudge {
        return;
    }

    let prompt = format!(
        "Your PR #{pr} has changes requested. Read the review comments, \
         make the requested changes, commit and push to branch issue-{issue}. \
         Then comment on issue #{issue} that you've addressed the feedback.",
        pr = pr_number.0,
        issue = issue_num.0,
    );
    info!(name, "nudging for changes requested on PR #{pr}", pr = pr_number.0);
    agent.create_and_send(index, name, &prompt).await;
}

async fn handle_approved(
    agent: &Agent,
    state: &SharedState,
    name: &str,
    index: u32,
    pr_number: PrNumber,
    issue_num: IssueNumber,
    cfg: &Config,
) {
    let should_nudge = {
        let mut st = state.write().await;
        let Some(m) = st.find_member_mut(name) else { return };
        if m.nudge_merge {
            false
        } else {
            m.nudge_merge = true;
            true
        }
    };

    if !should_nudge {
        return;
    }

    let repo = &cfg.repo;
    let pr = pr_number.0;
    let issue = issue_num.0;
    let prompt = format!(
        "Your PR #{pr} is approved. Merge it:\n\
         curl -s -X POST \"http://forgejo:3000/api/v1/repos/{repo}/pulls/{pr}/merge\" \\\n\
           -u \"$FORGEJO_USER:$FORGEJO_PASSWORD\" -H \"Content-Type: application/json\" \\\n\
           -d '{{\"Do\":\"merge\"}}'\n\
         Then label issue #{issue} \"review\" and comment that the PR is merged."
    );
    info!(name, "nudging for merge on approved PR #{pr}");
    agent.create_and_send(index, name, &prompt).await;
}
