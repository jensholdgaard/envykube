use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// Strongly-typed issue identifier. Cannot be accidentally used as a PR number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IssueNumber(pub u64);

/// Strongly-typed pull-request identifier. Cannot be accidentally used as an issue number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PrNumber(pub u64);

impl std::fmt::Display for IssueNumber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}

impl std::fmt::Display for PrNumber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}

impl From<u64> for IssueNumber {
    fn from(n: u64) -> Self {
        Self(n)
    }
}

impl From<u64> for PrNumber {
    fn from(n: u64) -> Self {
        Self(n)
    }
}

/// Events flow from the relay (Forgejo webhooks) and reconciler (periodic polls)
/// to listeners that act on them.
///
/// Issue / PR ambiguity is prevented at the type level: `IssueNumber` and `PrNumber`
/// are distinct newtypes — the compiler will reject passing one where the other is
/// expected.
#[derive(Clone, Debug)]
pub enum Event {
    // ── Forgejo webhook / polling events ──
    IssueOpened {
        number: IssueNumber,
        title: String,
    },
    IssueLabeled {
        number: IssueNumber,
        label: String,
    },
    IssueUnlabeled {
        number: IssueNumber,
        label: String,
    },
    IssueClosed {
        number: IssueNumber,
    },
    IssueComment {
        issue_number: IssueNumber,
        comment_id: u64,
        user: String,
        body: String,
    },
    PrComment {
        pr_number: PrNumber,
        comment_id: u64,
        user: String,
        body: String,
    },
    PullRequestOpened {
        pr_number: PrNumber,
        title: String,
        head_branch: String,
    },
    PullRequestReview {
        pr_number: PrNumber,
        state: String,
        body: String,
    },
    /// New commits were pushed to an open PR's head branch.
    ///
    /// The chaos gate depends on this: without it an agent could pass the suite on a stub,
    /// push the real implementation, and merge work that was never tested. Any push clears
    /// `chaos-passed`.
    PullRequestSynchronized {
        pr_number: PrNumber,
    },
    PullRequestMerged {
        pr_number: PrNumber,
    },

    // ── vCluster lifecycle events ──
    MemberProvisioned {
        name: String,
        index: u32,
    },
    MemberReady {
        name: String,
        index: u32,
    },
    MemberDestroyed {
        name: String,
    },
    MemberClaimed {
        name: String,
        index: u32,
        issue: IssueNumber,
    },

    // ── Agent communication events ──
    AgentPromptSent {
        name: String,
        issue: IssueNumber,
    },

    // ── Internal / reconciliation ──
    PoolNeedsRefill {
        current: usize,
        target: usize,
    },
    ReconciliationTick,
    Shutdown,
}

/// Wraps a `broadcast::Sender` for fanout pub/sub within the process.
#[derive(Clone)]
pub struct Bus {
    tx: broadcast::Sender<Event>,
}

impl Bus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    pub fn publish(&self, event: Event) {
        let _ = self.tx.send(event);
    }
}
