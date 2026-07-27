use crate::config::Config;
use crate::event::{Bus, Event};
use crate::forgejo::Forgejo;
use crate::vcluster;
use super::{SharedState, EventRx};
use std::sync::Arc;
use tracing::{info, warn};

pub async fn run(bus: Bus, mut events: EventRx, state: SharedState, cfg: Arc<Config>) {
    let forgejo = Forgejo::new(&cfg);
    let state_path = crate::config::resolve_state_path(&cfg.state_dir);

    loop {
        let event = match events.recv().await {
            Ok(e) => e,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                warn!(skipped = n, "closed_reaper fell behind");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };

        let number = match event {
            Event::IssueClosed { number } => number,
            Event::Shutdown => break,
            _ => continue,
        };

        let member = {
            let st = state.read().await;
            st.find_by_issue(number).map(|m| (m.name.clone(), m.index))
        };

        let Some((name, _index)) = member else {
            continue;
        };

        info!(name, "issue #{number} closed — destroying member");
        let _ = forgejo
            .post_issue_comment(number.0, &format!("Issue #{number} closed. Tearing down {name}."))
            .await;
        vcluster::destroy_member(&name).await;
        forgejo.delete_agent_user(&name).await;

        let active = {
            let mut st = state.write().await;
            st.members.retain(|m| m.name != name);
            let _ = st.save(&state_path);
            st.active_count()
        };

        if active < cfg.min_available {
            bus.publish(Event::PoolNeedsRefill {
                current: active,
                target: cfg.min_available,
            });
        }
    }
}
