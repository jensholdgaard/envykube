use crate::agent::Agent;
use crate::config::Config;
use crate::event::Event;
use crate::forgejo::Forgejo;
use super::{SharedState, EventRx};
use std::sync::Arc;
use tracing::{info, warn};

/// Routes PR comments to the agent that owns the related issue.
/// Unlike issue comments, the response is posted on the PR, not the issue.
pub async fn run(mut events: EventRx, state: SharedState, cfg: Arc<Config>) {
    let agent = Agent::new();

    loop {
        let event = match events.recv().await {
            Ok(e) => e,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                warn!(skipped = n, "pr_comment_router fell behind");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };

        let (pr_number, _comment_id, user, body) = match event {
            Event::PrComment {
                pr_number,
                comment_id,
                user,
                body,
            } => (pr_number, comment_id, user, body),
            Event::Shutdown => break,
            _ => continue,
        };

        let forgejo = Forgejo::new(&cfg);
        let issue_num = find_issue_for_pr(&forgejo, pr_number).await;

        let Some(issue_num) = issue_num else {
            warn!(pr = pr_number.0, "could not determine which issue PR belongs to, ignoring comment");
            continue;
        };

        let member = {
            let st = state.read().await;
            st.find_by_issue(issue_num).map(|m| (m.name.clone(), m.index))
        };

        let Some((name, index)) = member else {
            continue;
        };

        let pr = pr_number.0;
        let prompt = format!(
            "A new comment was posted on PR #{pr} (for issue #{issue}) by {user}:\n\
             > {body_limited}\n\n\
             Read it carefully. If it contains a requirement change, correction, or request for updates:\n\
             1. Implement the requested changes\n\
             2. Commit and push to branch issue-{issue}\n\
             3. **Post your response as a comment on PR #{pr}**, not on issue #{issue}\n\n\
             If it's a question, answer it by posting a comment on PR #{pr}.",
            issue = issue_num.0,
            body_limited = &super::truncate(&body, 500),
        );

        info!(name, "routing PR comment from {user} on PR #{pr} (issue #{issue})", issue = issue_num.0);
        agent.create_and_send(index, &name, &prompt).await;
    }
}

/// Given a PR number, find which issue it references by searching PR title/body
/// for issue references like "#14" or "Closes #14".
async fn find_issue_for_pr(forgejo: &Forgejo, pr_number: crate::event::PrNumber) -> Option<crate::event::IssueNumber> {
    let prs = forgejo.get_pull_requests("open").await.ok()?;
    let pr = prs.into_iter().find(|p| p.number == pr_number.0)?;
    let text = format!("{} {}", pr.title, pr.body.as_deref().unwrap_or(""));
    super::extract_issue_ref(&text)
}
