use crate::config::Config;
use crate::event::{Bus, Event, IssueNumber, PrNumber};
use crate::forgejo::Forgejo;
use crate::state::MemberStatus;
use crate::vcluster;
use super::SharedState;
use std::sync::Arc;
use tracing::{error, info};

pub async fn run(bus: Bus, state: SharedState, cfg: Arc<Config>) {
    let forgejo = Forgejo::new(&cfg);
    let state_path = crate::config::resolve_state_path(&cfg.state_dir);
    let mut tick: u64 = 0;

    // Recover stale state before entering the loop. After a nuke+bootstrap
    // cycle, vClusters listed in state.json no longer exist but vcluster's
    // per-instance state on disk may still be present, causing "already exists"
    // errors on re-provision.
    recover_stale_state(&state, &forgejo, &state_path).await;

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(cfg.poll_interval_secs)).await;
        tick += 1;
        bus.publish(Event::ReconciliationTick);

        log_tick(&state, &cfg, tick).await;
        sanitize_vclusters(&state, &forgejo, &state_path).await;
        let changed = refill_pool(&state, &cfg, &state_path, &bus).await;
        check_ready_issues(&forgejo, &cfg, &bus).await;
        poll_conversations(&forgejo, &state, &state_path, &bus).await;

        if changed {
            increment_change_counter(&state, &state_path).await;
        }
    }
}

async fn log_tick(state: &SharedState, cfg: &Config, tick: u64) {
    let st = state.read().await;
    info!(
        tick,
        idle = st.idle_count(),
        claimed = st.claimed_count(),
        total = st.active_count(),
        min = cfg.min_available,
        "reconciler tick"
    );
}

async fn sanitize_vclusters(state: &SharedState, forgejo: &Forgejo, state_path: &std::path::Path) {
    let vanished = find_vanished(state).await;
    if vanished.is_empty() {
        return;
    }

    for name in &vanished {
        info!(name, "vCluster vanished — removing from pool");
        forgejo.delete_agent_user(name).await;
        vcluster::clean_stale_vcluster(name).await;
    }

    let mut st = state.write().await;
    st.members.retain(|m| !vanished.contains(&m.name));
    let _ = st.save(state_path);
}

/// Run on startup: for each member whose state is "provisioning" or "claiming",
/// verify whether the vCluster actually exists and recover a usable state.
async fn recover_stale_state(
    state: &SharedState,
    forgejo: &Forgejo,
    state_path: &std::path::Path,
) {
    let mut changed = false;

    // Collect stale members
    let stale: Vec<_> = {
        let st = state.read().await;
        st.members
            .iter()
            .filter(|m| m.status == MemberStatus::Provisioning || m.status == MemberStatus::Claiming)
            .map(|m| (m.name.clone(), m.status.clone()))
            .collect()
    };

    for (name, status) in stale {
        let exists = vcluster::member_exists(&name).await;

        match (&status, exists) {
            // Provisioning/claiming + vCluster exists → recover as idle
            (_, true) => {
                info!(name, ?status, "vCluster exists but state was stale — recovering as idle");
                let mut st = state.write().await;
                if let Some(m) = st.find_member_mut(&name) {
                    m.status = MemberStatus::Idle;
                    m.issue = None;
                }
                changed = true;
            }
            // Provisioning/claiming + vCluster gone → remove from state, clean vcluster local state
            (_, false) => {
                info!(name, ?status, "vCluster vanished — removing from pool");
                forgejo.delete_agent_user(&name).await;
                vcluster::clean_stale_vcluster(&name).await;
                let mut st = state.write().await;
                st.members.retain(|m| m.name != name);
                changed = true;
            }
        }
    }

    if changed {
        let st = state.read().await;
        let _ = st.save(state_path);
        info!("startup state recovery complete");
    }
}

async fn find_vanished(state: &SharedState) -> Vec<String> {
    let st = state.read().await;
    let mut names = Vec::new();
    for m in &st.members {
        if !vcluster::member_exists(&m.name).await {
            names.push(m.name.clone());
        }
    }
    names
}

async fn refill_pool(state: &SharedState, cfg: &Config, state_path: &std::path::Path, bus: &Bus) -> bool {
    let need = {
        let st = state.read().await;
        cfg.min_available.saturating_sub(st.active_count())
    };

    if need == 0 {
        return false;
    }

    info!(need, "refilling pool");

    for _ in 0..need {
        let success = provision_one(state, cfg, state_path, bus).await;
        if !success {
            error!("failed to provision pool member");
        }
    }
    true
}

async fn provision_one(state: &SharedState, cfg: &Config, state_path: &std::path::Path, bus: &Bus) -> bool {
    let (name, index) = {
        let mut st = state.write().await;
        let idx = st.next_index(cfg.pool_base);
        let name = format!("pool-{idx}");
        st.members.push(crate::state::Member::new(idx, 30000 + idx));
        let _ = st.save(state_path);
        (name, idx)
    };

    if vcluster::provision_member(&name, index).await {
        let mut st = state.write().await;
        if let Some(m) = st.find_member_mut(&name) {
            m.status = MemberStatus::Idle;
        }
        let _ = st.save(state_path);
        bus.publish(Event::MemberReady { name, index });
        true
    } else {
        let mut st = state.write().await;
        if let Some(m) = st.find_member_mut(&name) {
            m.status = MemberStatus::Failed;
        }
        let _ = st.save(state_path);
        false
    }
}

async fn check_ready_issues(forgejo: &Forgejo, cfg: &Config, bus: &Bus) {
    let ready_label = cfg.labels.ready.clone();
    let issues = match forgejo.get_issues("open", Some(&ready_label)).await {
        Ok(i) => i,
        Err(e) => {
            error!(error = %e, "failed to fetch ready issues");
            return;
        }
    };
    if issues.is_empty() {
        return;
    }

    info!(count = issues.len(), "found ready issues");
    for issue in &issues {
        bus.publish(Event::IssueLabeled {
            number: IssueNumber(issue.number),
            label: ready_label.clone(),
        });
    }
}

/// Turn Forgejo conversation state into events by polling.
///
/// `relay.rs` translates Forgejo webhooks into the same events, but a webhook has to be
/// registered on the repo and has to be able to reach the relay — and the Forgejo container
/// sits on the k3d docker network with no stable route back to the host. Without this the
/// manager only ever sees `ready` labels: comments, PRs and reviews never arrive, and the
/// `/chaos` trigger can never fire. Polling costs a handful of requests every `pollInterval`
/// and needs no container networking at all; the webhook relay stays as an accelerator.
///
/// Watermarks are advanced here, at publish time, rather than in the listeners — a listener
/// that is slow or drops the event must not cause the same comment to be replayed every tick.
async fn poll_conversations(
    forgejo: &Forgejo,
    state: &SharedState,
    state_path: &std::path::Path,
    bus: &Bus,
) {
    // Only claimed members have a conversation worth polling.
    let watched: Vec<(String, IssueNumber)> = {
        let st = state.read().await;
        st.members
            .iter()
            .filter(|m| m.status == MemberStatus::Claimed || m.status == MemberStatus::Claiming)
            .filter_map(|m| m.issue.map(|i| (m.name.clone(), i)))
            .collect()
    };
    if watched.is_empty() {
        return;
    }

    let open_prs = forgejo.get_pull_requests("open").await.unwrap_or_default();
    let mut dirty = false;

    for (name, issue) in watched {
        dirty |= poll_comments(forgejo, state, bus, &name, issue).await;
        dirty |= poll_pull_request(forgejo, state, bus, &name, issue, &open_prs).await;
    }

    if dirty {
        let st = state.read().await;
        let _ = st.save(state_path);
    }
}

async fn poll_comments(
    forgejo: &Forgejo,
    state: &SharedState,
    bus: &Bus,
    name: &str,
    issue: IssueNumber,
) -> bool {
    let Ok(comments) = forgejo.get_issue_comments(issue.0).await else {
        return false;
    };
    let key = format!("c-{}", issue.0);

    let last_seen = {
        let st = state.read().await;
        st.find_member(name)
            .and_then(|m| m.last_comment_seen.get(&key).copied())
            .unwrap_or(0)
    };

    let mut highest = last_seen;
    let mut fresh = Vec::new();
    for c in comments {
        if c.id > last_seen {
            highest = highest.max(c.id);
            fresh.push(c);
        }
    }
    if fresh.is_empty() {
        return false;
    }

    // Advance the watermark BEFORE publishing, so a listener panicking or lagging cannot
    // cause the same comment to be re-delivered on every subsequent tick.
    {
        let mut st = state.write().await;
        if let Some(m) = st.find_member_mut(name) {
            m.last_comment_seen.insert(key, highest);
        }
    }

    for c in fresh {
        let user = c.user.as_ref().map(|u| u.login.clone()).unwrap_or_default();
        info!(name, user, "polled new comment on issue #{n}", n = issue.0);
        bus.publish(Event::IssueComment {
            issue_number: issue,
            comment_id: c.id,
            user,
            body: c.body,
        });
    }
    true
}

async fn poll_pull_request(
    forgejo: &Forgejo,
    state: &SharedState,
    bus: &Bus,
    name: &str,
    issue: IssueNumber,
    open_prs: &[crate::forgejo::PullRequest],
) -> bool {
    let Some(pr) = open_prs.iter().find(|p| {
        let text = format!("{} {}", p.title, p.body.as_deref().unwrap_or(""));
        super::extract_issue_ref(&text) == Some(issue)
    }) else {
        return false;
    };

    let head_sha = pr.head.as_ref().and_then(|h| h.sha.clone());
    let (seen_pr, prev_sha, last_review) = {
        let st = state.read().await;
        st.find_member(name)
            .map(|m| (m.seen_pr, m.pr_head_sha.clone(), m.last_review_seen))
            .unwrap_or((None, None, 0))
    };

    let mut dirty = false;

    if seen_pr != Some(pr.number) {
        {
            let mut st = state.write().await;
            if let Some(m) = st.find_member_mut(name) {
                m.seen_pr = Some(pr.number);
                m.pr_head_sha = head_sha.clone();
            }
        }
        info!(name, "polled new PR #{n}", n = pr.number);
        bus.publish(Event::PullRequestOpened {
            pr_number: PrNumber(pr.number),
            title: pr.title.clone(),
            head_branch: pr
                .head
                .as_ref()
                .and_then(|h| h.r#ref.clone())
                .unwrap_or_default(),
        });
        dirty = true;
    } else if head_sha.is_some() && head_sha != prev_sha {
        // New commits on an already-known PR. This is what stops an agent passing the gate
        // on a stub and then pushing the real implementation.
        {
            let mut st = state.write().await;
            if let Some(m) = st.find_member_mut(name) {
                m.pr_head_sha = head_sha.clone();
            }
        }
        info!(name, "polled push to PR #{n}", n = pr.number);
        bus.publish(Event::PullRequestSynchronized {
            pr_number: PrNumber(pr.number),
        });
        dirty = true;
    }

    let Ok(reviews) = forgejo.get_reviews(pr.number).await else {
        return dirty;
    };
    let mut highest = last_review;
    let mut fresh = Vec::new();
    for r in reviews {
        // Forgejo emits a PENDING/COMMENT review for every review comment; only the two
        // decisions drive the lifecycle.
        if r.id > last_review && (r.state.contains("APPROVED") || r.state.contains("REQUEST")) {
            highest = highest.max(r.id);
            fresh.push(r);
        }
    }
    if fresh.is_empty() {
        return dirty;
    }
    {
        let mut st = state.write().await;
        if let Some(m) = st.find_member_mut(name) {
            m.last_review_seen = highest;
        }
    }
    for r in fresh {
        info!(name, state = %r.state, "polled review on PR #{n}", n = pr.number);
        bus.publish(Event::PullRequestReview {
            pr_number: PrNumber(pr.number),
            state: r.state,
            body: r.body,
        });
    }
    true
}

async fn increment_change_counter(state: &SharedState, state_path: &std::path::Path) {
    let mut st = state.write().await;
    for m in &mut st.members {
        m.reconciler_changes += 1;
    }
    let _ = st.save(state_path);
}
