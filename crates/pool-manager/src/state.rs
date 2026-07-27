use crate::event::IssueNumber;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PoolState {
    pub members: Vec<Member>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Member {
    pub index: u32,
    pub name: String,
    pub status: MemberStatus,
    #[serde(rename = "nodePort")]
    pub node_port: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<IssueNumber>,
    #[serde(default, skip_serializing_if = "nudge_default")]
    pub nudge_create_pr: bool,
    #[serde(default, skip_serializing_if = "nudge_default")]
    pub nudge_changes_requested: bool,
    #[serde(default, skip_serializing_if = "nudge_default")]
    pub nudge_merge: bool,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub last_comment_seen: HashMap<String, u64>,
    #[serde(default, skip_serializing_if = "zero")]
    pub last_health_tick: u64,
    #[serde(default, skip_serializing_if = "zero")]
    pub reconciler_changes: u64,

    // ── chaos gate ──
    /// Verdict of the last chaos run. Persisted so a pool-manager restart mid-card doesn't
    /// silently re-open the gate on work that had already failed it.
    #[serde(default, skip_serializing_if = "ChaosStatus::is_not_run")]
    pub chaos_status: ChaosStatus,
    /// How many times the suite has run for this card. Capped (see `MAX_CHAOS_RUNS`) so a
    /// model that cannot fix the failure escalates to a human instead of looping forever.
    #[serde(default, skip_serializing_if = "zero")]
    pub chaos_runs: u64,

    // ── poll watermarks (no Forgejo webhook is registered; the reconciler polls) ──
    /// PR number already announced for this member's issue, so a poll doesn't re-announce it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen_pr: Option<u64>,
    /// Highest review id already turned into an event.
    #[serde(default, skip_serializing_if = "zero")]
    pub last_review_seen: u64,
    /// Head SHA of the open PR. A change means someone pushed — that's a synchronize, and it
    /// invalidates a chaos pass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_head_sha: Option<String>,
}

fn nudge_default(b: &bool) -> bool {
    !*b
}

fn zero(n: &u64) -> bool {
    *n == 0
}

impl Member {
    /// A fresh, unclaimed pool member. Both the reconciler and the pool_refiller create
    /// members; a constructor keeps them from drifting apart every time a field is added.
    pub fn new(index: u32, node_port: u32) -> Self {
        Self {
            index,
            name: format!("pool-{index}"),
            status: MemberStatus::Provisioning,
            node_port,
            issue: None,
            nudge_create_pr: false,
            nudge_changes_requested: false,
            nudge_merge: false,
            last_comment_seen: HashMap::new(),
            last_health_tick: 0,
            reconciler_changes: 0,
            chaos_status: ChaosStatus::NotRun,
            chaos_runs: 0,
            seen_pr: None,
            last_review_seen: 0,
            pr_head_sha: None,
        }
    }

    /// Reset everything card-specific. Called when a member is reserved for a new issue so no
    /// nudge flag, watermark or chaos verdict leaks from the previous card.
    pub fn reset_for_issue(&mut self, issue: IssueNumber) {
        self.issue = Some(issue);
        self.nudge_create_pr = false;
        self.nudge_changes_requested = false;
        self.nudge_merge = false;
        self.last_comment_seen.clear();
        self.last_health_tick = 0;
        self.chaos_status = ChaosStatus::NotRun;
        self.chaos_runs = 0;
        self.seen_pr = None;
        self.last_review_seen = 0;
        self.pr_head_sha = None;
    }
}

/// How many times one card may run the chaos suite before a human is asked to look.
/// A weak model that cannot work out the fix must escalate, not spin.
pub const MAX_CHAOS_RUNS: u64 = 5;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ChaosStatus {
    #[default]
    NotRun,
    Running,
    Passed,
    Failed,
}

impl ChaosStatus {
    fn is_not_run(&self) -> bool {
        *self == ChaosStatus::NotRun
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MemberStatus {
    Provisioning,
    Idle,
    Claiming,
    Claimed,
    Failed,
}

impl PoolState {
    pub fn load_or_new(path: &Path) -> anyhow::Result<Self> {
        if path.exists() {
            let raw = std::fs::read_to_string(path)?;
            Ok(serde_json::from_str::<Self>(&raw)?)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(self)?;
        std::fs::write(path, raw + "\n")?;
        Ok(())
    }

    pub fn idle_count(&self) -> usize {
        self.members
            .iter()
            .filter(|m| m.status == MemberStatus::Idle)
            .count()
    }

    pub fn active_count(&self) -> usize {
        self.members
            .iter()
            .filter(|m| m.status != MemberStatus::Failed)
            .count()
    }

    pub fn claimed_count(&self) -> usize {
        self.members
            .iter()
            .filter(|m| m.status == MemberStatus::Claimed)
            .count()
    }

    pub fn find_member(&self, name: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.name == name)
    }

    pub fn find_member_mut(&mut self, name: &str) -> Option<&mut Member> {
        self.members.iter_mut().find(|m| m.name == name)
    }

    pub fn find_by_issue(&self, issue: IssueNumber) -> Option<&Member> {
        self.members
            .iter()
            .filter(|m| m.status == MemberStatus::Claimed || m.status == MemberStatus::Claiming)
            .find(|m| m.issue == Some(issue))
    }

    pub fn next_index(&self, pool_base: u32) -> u32 {
        let used: std::collections::HashSet<u32> =
            self.members.iter().map(|m| m.index).collect();
        let mut i = pool_base;
        while used.contains(&i) {
            i += 1;
        }
        i
    }
}

impl Default for PoolState {
    fn default() -> Self {
        Self {
            members: Vec::new(),
        }
    }
}
