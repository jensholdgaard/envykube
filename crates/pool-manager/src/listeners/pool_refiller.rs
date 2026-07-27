use crate::config::Config;
use crate::event::Event;
use crate::state::MemberStatus;
use crate::vcluster;
use super::{SharedState, EventRx};
use std::sync::Arc;
use tracing::{error, info, warn};

pub async fn run(mut events: EventRx, state: SharedState, cfg: Arc<Config>) {
    let state_path = crate::config::resolve_state_path(&cfg.state_dir);

    loop {
        let event = match events.recv().await {
            Ok(e) => e,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                warn!(skipped = n, "pool_refiller fell behind");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };

        match event {
            Event::PoolNeedsRefill { current, target } => {
                info!(current, target, "refilling pool on demand");
                fill_to(state.clone(), &cfg, &state_path, target).await;
            }
            Event::Shutdown => break,
            _ => {}
        }
    }
}

async fn fill_to(state: SharedState, cfg: &Config, state_path: &std::path::Path, target: usize) {
    let need = {
        let st = state.read().await;
        target.saturating_sub(st.active_count())
    };

    for _ in 0..need {
        let (name, index) = {
            let mut st = state.write().await;
            let idx = st.next_index(cfg.pool_base);
            let name = format!("pool-{idx}");
            st.members.push(crate::state::Member::new(idx, 30000 + idx));
            let _ = st.save(state_path);
            (name, idx)
        };

        let success = vcluster::provision_member(&name, index).await;
        let mut st = state.write().await;
        if let Some(m) = st.find_member_mut(&name) {
            if success {
                m.status = MemberStatus::Idle;
            } else {
                m.status = MemberStatus::Failed;
                error!(name, "provision failed");
            }
        }
        let _ = st.save(state_path);
    }
}
