use crate::agent::Agent;
use crate::config::Config;
use crate::event::{Bus, Event, IssueNumber};
use crate::forgejo::Forgejo;
use crate::state::MemberStatus;
use crate::vcluster;
use super::{SharedState, EventRx};
use std::sync::Arc;
use tracing::{info, warn};

pub async fn run(bus: Bus, mut events: EventRx, state: SharedState, cfg: Arc<Config>) {
    let forgejo = Forgejo::new(&cfg);
    let agent = Agent::new();
    let state_path = crate::config::resolve_state_path(&cfg.state_dir);

    loop {
        let event = match events.recv().await {
            Ok(e) => e,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                warn!(skipped = n, "issue_claimer fell behind");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };

        let number = match event {
            Event::IssueLabeled { number, label } if label == cfg.labels.ready => number,
            Event::Shutdown => break,
            _ => continue,
        };

        claim_if_idle(&bus, &forgejo, &agent, &state, &cfg, &state_path, number).await;
    }
}

async fn claim_if_idle(
    bus: &Bus,
    forgejo: &Forgejo,
    agent: &Agent,
    state: &SharedState,
    cfg: &Config,
    state_path: &std::path::Path,
    number: IssueNumber,
) {
    // Guard: already claimed?
    {
        let st = state.read().await;
        if st.find_by_issue(number).is_some() {
            return;
        }
    }

    // Reserve an idle member
    let (name, index) = match reserve_idle(state, state_path, number) {
        Some(r) => r,
        None => {
            let st = state.read().await;
            warn!(issue = number.0, "no idle members available");
            let _ = forgejo
                .post_issue_comment(
                    number.0,
                    &format!("No idle agent available for issue #{num}. Requesting pool expansion...", num = number.0),
                )
                .await;
            bus.publish(Event::PoolNeedsRefill {
                current: st.idle_count(),
                target: cfg.min_available,
            });
            return;
        }
    };

    info!(name, index, "claiming issue #{num}", num = number.0);
    // Posted as admin — agent identity doesn't exist yet.
    let _ = forgejo
        .post_issue_comment(
            number.0,
            &format!("Claimed by {name} for issue #{num}.", num = number.0),
        )
        .await;

    // Set in-progress label
    let _ = forgejo
        .set_issue_labels(number.0, &[cfg.labels.in_progress.clone()])
        .await;

    // Claim workspace
    let ok = claim_workspace_and_identity(forgejo, &name, index, cfg, state, state_path).await;

    if !ok {
        // Agent identity may not exist yet — post as admin.
        let _ = forgejo
            .post_issue_comment(
                number.0,
                &format!("Failed to claim issue #{num} — workspace setup failed.", num = number.0),
            )
            .await;
        return;
    }

    bus.publish(Event::MemberClaimed {
        name: name.clone(),
        index,
        issue: number,
    });

    // Agent identity now exists — post as the agent for traceability.
    let _ = forgejo
        .post_comment_as_agent(
            &name,
            index,
            number.0,
            &format!("Issue #{num} claimed by {name}. Workspace ready, agent starting work.", num = number.0),
        )
        .await;

    send_initial_prompt(agent, forgejo, cfg, &name, index, number).await;
}

fn reserve_idle(
    state: &SharedState,
    state_path: &std::path::Path,
    number: IssueNumber,
) -> Option<(String, u32)> {
    let mut st = tokio::task::block_in_place(|| state.blocking_write());
    let member = st.members.iter_mut().find(|m| m.status == MemberStatus::Idle)?;
    member.status = MemberStatus::Claiming;
    member.reset_for_issue(number);
    let name = member.name.clone();
    let index = member.index;
    let _ = st.save(state_path);
    Some((name, index))
}

async fn claim_workspace_and_identity(
    forgejo: &Forgejo,
    name: &str,
    index: u32,
    cfg: &Config,
    state: &SharedState,
    state_path: &std::path::Path,
) -> bool {
    let (ok, ws_path) = vcluster::claim_member(name, index, &cfg.repo_url).await;

    if !ok {
        let mut st = state.write().await;
        if let Some(m) = st.find_member_mut(name) {
            m.status = MemberStatus::Failed;
        }
        let _ = st.save(state_path);
        return false;
    }

    if let Some((agent_user, _)) = forgejo.create_agent_user(index).await {
        if let Some(ref ws) = ws_path {
            forgejo.write_agent_creds(ws, &agent_user, index).await;
        }
    }

    let mut st = state.write().await;
    if let Some(m) = st.find_member_mut(name) {
        m.status = MemberStatus::Claimed;
    }
    let _ = st.save(state_path);
    true
}

async fn send_initial_prompt(
    agent: &Agent,
    forgejo: &Forgejo,
    cfg: &Config,
    name: &str,
    index: u32,
    issue_num: IssueNumber,
) {
    let issues = forgejo.get_issues("open", None).await.unwrap_or_default();
    let issue = issues.into_iter().find(|i| i.number == issue_num.0);

    let title = issue.as_ref().map(|i| i.title.clone()).unwrap_or_default();
    let body = issue.and_then(|i| i.body).unwrap_or_default();
    let escaped_title = super::escape_quotes(&title);
    let n = issue_num.0;

    let repo = &cfg.repo;
    let fj_url = "http://forgejo:3000";
    let agent_user = format!("pool-{index}");

    let prompt = format!(
        r#"You are {agent_user}, assigned to issue #{n} in {repo}: "{title}".

Issue description:
{body_limited}

Steps — complete each step before moving on:
1. Source your credentials: source /workspace/.forgejo
2. Create a branch: cd /workspace; git checkout -b issue-{n}
3. Implement the fix or feature described in the issue. Commit with: git add . && git commit -m "Fixes #{n}: {title}" && git push origin issue-{n}
4. PASS THE CHAOS GATE — do this BEFORE creating a PR. You trigger it, not a human.
   Your work is tested under deliberate faults: the same deploy run twice, pods killed
   mid-apply, the Kubernetes API rejecting a write with 409/500/429, the dependency scaled
   away underneath you. Code that only works on the happy path fails here.
   a. Make sure /workspace/.chaos.yaml exists and describes how YOUR work is deployed,
      readied and verified — the suite cannot test what it cannot drive:
        namespace: default
        deploy:  "kubectl apply -k ./deploy"     # must be safe to run twice
        ready:   "kubectl rollout status deploy/<name> --timeout=120s"
        verify:  "curl -fsS http://<name>.$VCLUSTER_NAME.localhost/healthz"
        selector: "app=<name>"
        dependency:            # only if your app has one
          name: postgres
          selector: "app=postgres"
          port: 5432
      Commit and push it with your work.
   b. Trigger the suite by commenting `/chaos` on issue #{n}:
       source /workspace/.forgejo
       curl -s -X POST "{fj_url}/api/v1/repos/{repo}/issues/{n}/comments" \
         -u "$FORGEJO_USER:$FORGEJO_PASSWORD" -H "Content-Type: application/json" \
         -d '{{"body":"/chaos"}}'
   c. The operator runs the suite (this takes a few minutes) and sends you a prompt with the
      result. The full report is also posted as a comment on issue #{n}.
      If it FAILS: each failure names the behaviour that was wrong — read it, fix the cause,
      commit and push to issue-{n}, then repeat step 4b to re-run. After 5 failed runs the
      gate stops and asks for a human, so if two attempts change nothing, change the approach.
      If it PASSES: proceed to step 5.
5. ONLY after the chaos suite PASSES, create a PR via curl:
    curl -s -X POST "{fj_url}/api/v1/repos/{repo}/pulls" \
      -u "$FORGEJO_USER:$FORGEJO_PASSWORD" -H "Content-Type: application/json" \
      -d '{{"title":"[#{n}] {escaped_title}","head":"issue-{n}","base":"main","body":"Closes #{n}"}}'
    Save the returned PR number — you'll need it.
    A PR opened before the gate is green is blocked with a REQUEST_CHANGES review, and pushing
    new commits after a pass invalidates it — so run the suite last, not early.
6. Post a comment on the issue:
    curl -s -X POST "{fj_url}/api/v1/repos/{repo}/issues/{n}/comments" \
      -u "$FORGEJO_USER:$FORGEJO_PASSWORD" -H "Content-Type: application/json" \
      -d '{{"body":"PR ready at {fj_url}/{repo}/pulls/<PR_NUM> — waiting for review."}}'

After completing all steps, report "DONE: steps 1-6 completed, chaos passed, PR #<num> created."

Credentials are at /workspace/.forgejo. This repo is already cloned at /workspace. Start working."#,
        body_limited = &super::truncate(&body, 2000),
    );

    info!(name, "sending initial prompt for issue #{n}");
    agent.create_and_send(index, name, &prompt).await;
}
