use crate::agent::Agent;
use crate::config::Config;
use crate::event::Event;
use crate::forgejo::Forgejo;
use crate::state::MemberStatus;
use crate::event::IssueNumber;
use super::{SharedState, EventRx};
use std::sync::Arc;
use tracing::{info, warn};

pub async fn run(mut events: EventRx, state: SharedState, cfg: Arc<Config>) {
    let agent = Agent::new();
    let state_path = crate::config::resolve_state_path(&cfg.state_dir);
    let mut local_tick: u64 = 0;

    loop {
        let event = match events.recv().await {
            Ok(e) => e,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                warn!(skipped = n, "health_monitor fell behind");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };

        match event {
            Event::ReconciliationTick => {
                local_tick += 1;
                if local_tick % 3 != 0 {
                    continue;
                }
                check_claimed(&agent, &state, &cfg, &state_path, local_tick).await;
            }
            Event::Shutdown => break,
            _ => {}
        }
    }
}

async fn check_claimed(
    agent: &Agent,
    state: &SharedState,
    cfg: &Config,
    state_path: &std::path::Path,
    local_tick: u64,
) {
    let claimed: Vec<_> = {
        let st = state.read().await;
        st.members
            .iter()
            .filter(|m| m.status == MemberStatus::Claimed)
            .map(|m| (m.name.clone(), m.index, m.issue))
            .collect()
    };

    for (name, index, issue_num) in claimed {
        update_tick(state, state_path, &name, local_tick).await;
        let alive = agent.is_alive(index).await;
        if alive {
            continue;
        }

        warn!(name, "agent not responding");
        let Some(issue_num) = issue_num else { continue };
        maybe_nudge_create_pr(agent, state, cfg, state_path, &name, index, issue_num).await;
    }
}

async fn update_tick(state: &SharedState, state_path: &std::path::Path, name: &str, tick: u64) {
    let mut st = state.write().await;
    if let Some(m) = st.find_member_mut(name) {
        m.last_health_tick = tick;
    }
    let _ = st.save(state_path);
}

async fn maybe_nudge_create_pr(
    agent: &Agent,
    state: &SharedState,
    cfg: &Config,
    state_path: &std::path::Path,
    name: &str,
    index: u32,
    issue_num: IssueNumber,
) {
    let forgejo = Forgejo::new(cfg);
    if super::find_related_pr(&forgejo, issue_num).await.is_some() {
        return;
    }

    // Never nudge towards a PR before the chaos gate is green — chaos_enforcer would block
    // that PR the moment it opened, and the agent would be left ping-ponging between two
    // parts of its own harness. If the suite hasn't passed, ask for the suite instead.
    if !forgejo
        .issue_has_label(issue_num.0, &cfg.labels.chaos_passed)
        .await
    {
        nudge_run_chaos(agent, state, cfg, state_path, name, index, issue_num).await;
        return;
    }

    let should_nudge = {
        let st = state.read().await;
        st.find_member(name)
            .map(|m| !m.nudge_create_pr)
            .unwrap_or(false)
    };

    if !should_nudge {
        return;
    }

    {
        let mut st = state.write().await;
        if let Some(m) = st.find_member_mut(name) {
            m.nudge_create_pr = true;
        }
        let _ = st.save(state_path);
    }

    let repo = &cfg.repo;
    let n = issue_num.0;
    let prompt = format!(
        "Create a PR for your work on issue #{n}:\n\
         curl -s -X POST \"http://forgejo:3000/api/v1/repos/{repo}/pulls\" \\\n\
           -u \"$FORGEJO_USER:$FORGEJO_PASSWORD\" -H \"Content-Type: application/json\" \\\n\
           -d '{{\"title\":\"[#{n}] Fix\",\"head\":\"issue-{n}\",\"base\":\"main\",\"body\":\"Closes #{n}\"}}'\n\
         Also comment on the issue saying the PR is ready. Report the PR number when done."
    );
    info!(name, "nudging to create PR");
    agent.create_and_send(index, name, &prompt).await;
}

/// The stalled-agent nudge, chaos-gate edition: the next step is the suite, not a PR.
/// Uses the same one-shot `nudge_create_pr` flag so a quiet agent is prodded once, not
/// every health tick.
async fn nudge_run_chaos(
    agent: &Agent,
    state: &SharedState,
    cfg: &Config,
    state_path: &std::path::Path,
    name: &str,
    index: u32,
    issue_num: IssueNumber,
) {
    let should_nudge = {
        let st = state.read().await;
        st.find_member(name)
            .map(|m| !m.nudge_create_pr && m.chaos_status != crate::state::ChaosStatus::Running)
            .unwrap_or(false)
    };
    if !should_nudge {
        return;
    }
    {
        let mut st = state.write().await;
        if let Some(m) = st.find_member_mut(name) {
            m.nudge_create_pr = true;
        }
        let _ = st.save(state_path);
    }

    let repo = &cfg.repo;
    let n = issue_num.0;
    let prompt = format!(
        "Your work on issue #{n} has not passed the chaos suite yet, so do NOT open a PR.\n\
         Make sure /workspace/.chaos.yaml describes how your work is deployed, readied and \
         verified, commit and push it to branch issue-{n}, then request a run:\n\
         curl -s -X POST \"http://forgejo:3000/api/v1/repos/{repo}/issues/{n}/comments\" \\\n\
           -u \"$FORGEJO_USER:$FORGEJO_PASSWORD\" -H \"Content-Type: application/json\" \\\n\
           -d '{{\"body\":\"/chaos\"}}'\n\
         The report is posted back on the issue. Fix what it names, then run it again."
    );
    info!(name, "nudging to run the chaos suite");
    agent.create_and_send(index, name, &prompt).await;
}
