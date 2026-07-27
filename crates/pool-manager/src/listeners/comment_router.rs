use crate::agent::Agent;
use crate::config::Config;
use crate::event::{Event, IssueNumber};
use crate::forgejo::Forgejo;
use super::{SharedState, EventRx};
use std::sync::Arc;
use tracing::{info, warn};

pub async fn run(mut events: EventRx, state: SharedState, cfg: Arc<Config>) {
    let agent = Agent::new();
    let state_path = crate::config::resolve_state_path(&cfg.state_dir);

    loop {
        let event = match events.recv().await {
            Ok(e) => e,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                warn!(skipped = n, "comment_router fell behind");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };

        let (issue_number, comment_id, user, body) = match event {
            Event::IssueComment {
                issue_number,
                comment_id,
                user,
                body,
            } => (issue_number, comment_id, user, body),
            Event::Shutdown => break,
            _ => continue,
        };

        let member = {
            let st = state.read().await;
            st.find_by_issue(issue_number).map(|m| (m.name.clone(), m.index))
        };

        let Some((name, index)) = member else {
            continue;
        };

        // Do not prompt an agent about its own comment. `post_comment_as_agent` posts as
        // `pool-{index}`, so without this an agent announcing progress — or asking for a
        // chaos run — prompts itself, and the loop never settles.
        if user == format!("pool-{index}") {
            track_comment(&state, &state_path, &name, issue_number, comment_id).await;
            continue;
        }

        // Slash commands are operator commands, routed to their own listeners (`/chaos` →
        // chaos_gate). Forwarding them here too would run the command AND ask the agent to
        // interpret its own request as feedback.
        if body.trim_start().starts_with('/') {
            track_comment(&state, &state_path, &name, issue_number, comment_id).await;
            continue;
        }

        track_comment(&state, &state_path, &name, issue_number, comment_id).await;

        let forgejo = Forgejo::new(&cfg);
        let pr_num = super::find_related_pr(&forgejo, issue_number).await;

        let prompt = format!(
            "A new comment was posted on issue #{num} by {user}:\n\
             > {body_limited}\n\n\
             Read it carefully. If it contains a requirement change, correction, or request for updates:\n\
             1. Implement the requested changes\n\
             2. Commit and push to branch issue-{num}\n\
             3. Comment on the issue that you've updated the PR\n\n\
             If it's a question, answer it by posting a comment on the issue.\n\n\
             PR #: {pr_info}",
            num = issue_number.0,
            body_limited = &super::truncate(&body, 500),
            pr_info = pr_num.map(|n| n.0.to_string()).unwrap_or_else(|| "not created yet".into()),
        );

        info!(name, "routing comment from {user} on issue #{num}", num = issue_number.0);
        agent.create_and_send(index, &name, &prompt).await;
    }
}

async fn track_comment(
    state: &SharedState,
    state_path: &std::path::Path,
    name: &str,
    issue_number: IssueNumber,
    comment_id: u64,
) {
    let mut st = state.write().await;
    if let Some(m) = st.find_member_mut(name) {
        m.last_comment_seen
            .insert(format!("c-{}", issue_number.0), comment_id);
    }
    let _ = st.save(state_path);
}