use crate::event::IssueNumber;
use crate::config::Config;
use crate::state::PoolState;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

mod chaos_enforcer;
mod chaos_gate;
mod closed_reaper;
mod comment_router;
mod health_monitor;
mod issue_claimer;
mod pool_refiller;
mod pr_comment_router;
mod pr_monitor;
mod reconciler;

pub type SharedState = Arc<RwLock<PoolState>>;
pub type EventRx = tokio::sync::broadcast::Receiver<crate::event::Event>;

use crate::event::Bus;

pub fn spawn_all(bus: &Bus, state: SharedState, cfg: &Config) -> Vec<JoinHandle<()>> {
    let cfg = Arc::new(cfg.clone());
    vec![
        tokio::spawn(pool_refiller::run(bus.subscribe(), state.clone(), cfg.clone())),
        tokio::spawn(issue_claimer::run(bus.clone(), bus.subscribe(), state.clone(), cfg.clone())),
        tokio::spawn(closed_reaper::run(bus.clone(), bus.subscribe(), state.clone(), cfg.clone())),
        tokio::spawn(comment_router::run(bus.subscribe(), state.clone(), cfg.clone())),
        tokio::spawn(pr_comment_router::run(bus.subscribe(), state.clone(), cfg.clone())),
        tokio::spawn(pr_monitor::run(bus.subscribe(), state.clone(), cfg.clone())),
        tokio::spawn(health_monitor::run(bus.subscribe(), state.clone(), cfg.clone())),
        // The chaos gate: chaos_gate runs the suite on `/chaos`, chaos_enforcer keeps
        // un-tested work out of review.
        tokio::spawn(chaos_gate::run(bus.subscribe(), state.clone(), cfg.clone())),
        tokio::spawn(chaos_enforcer::run(bus.subscribe(), state.clone(), cfg.clone())),
    ]
}

pub fn spawn_reconciler(bus: Bus, state: SharedState, cfg: &Config) -> JoinHandle<()> {
    let cfg = Arc::new(cfg.clone());
    tokio::spawn(reconciler::run(bus, state, cfg))
}

// Shared helpers used by multiple listeners.

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..max]
    }
}

fn escape_quotes(s: &str) -> String {
    s.replace('"', "")
}

fn extract_issue_ref(text: &str) -> Option<IssueNumber> {
    for word in text.split_whitespace() {
        if let Some(num) = word.strip_prefix('#') {
            if let Ok(n) = num
                .trim_end_matches(|c: char| !c.is_ascii_digit())
                .parse::<u64>()
            {
                return Some(IssueNumber(n));
            }
        }
    }
    None
}

async fn find_related_pr(forgejo: &crate::forgejo::Forgejo, issue_num: IssueNumber) -> Option<crate::event::PrNumber> {
    let prs = forgejo.get_pull_requests("open").await.ok()?;
    for pr in prs {
        let text = format!("{} {}", pr.title, pr.body.as_deref().unwrap_or(""));
        if text.contains(&format!("#{}", issue_num.0)) {
            return Some(crate::event::PrNumber(pr.number));
        }
    }
    None
}
