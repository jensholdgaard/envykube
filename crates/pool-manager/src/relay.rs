use crate::event::{Bus, Event, IssueNumber, PrNumber};
use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use serde::Deserialize;
use std::net::SocketAddr;
use tracing::{debug, error, info};

#[derive(Clone)]
struct AppState {
    bus: Bus,
    webhook_secret: Option<String>,
}

pub fn spawn(bus: Bus, cfg: &crate::config::Config) -> tokio::task::JoinHandle<()> {
    let addr = SocketAddr::from(([0, 0, 0, 0], cfg.webhook_port));
    let state = AppState {
        bus,
        webhook_secret: cfg.webhook_secret.clone(),
    };

    let app = Router::new()
        .route("/webhook", post(handle_webhook))
        .with_state(state);

    info!(port = cfg.webhook_port, "webhook receiver listening");

    tokio::spawn(async move {
        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                error!(%addr, error = %e, "failed to bind webhook listener");
                return;
            }
        };
        if let Err(e) = axum::serve(listener, app).await {
            error!(error = %e, "webhook server error");
        }
    })
}

#[derive(Debug, Deserialize)]
struct WebhookPayload {
    #[serde(default)]
    action: String,
    #[serde(default)]
    issue: Option<IssuePayload>,
    #[serde(default)]
    comment: Option<CommentPayload>,
    #[serde(default)]
    pull_request: Option<PrPayload>,
    #[serde(default)]
    review: Option<ReviewPayload>,
    #[serde(default)]
    label: Option<LabelPayload>,
    #[serde(default)]
    repository: Option<RepoPayload>,
}

#[derive(Debug, Deserialize)]
struct IssuePayload {
    number: u64,
    title: String,
}

#[derive(Debug, Deserialize)]
struct CommentPayload {
    id: u64,
    #[serde(default)]
    body: String,
    #[serde(default)]
    user: Option<UserPayload>,
}

#[derive(Debug, Deserialize)]
struct PrPayload {
    number: u64,
    title: String,
    #[serde(default)]
    head: Option<BranchPayload>,
}

#[derive(Debug, Deserialize)]
struct BranchPayload {
    #[serde(default)]
    label: String,
}

#[derive(Debug, Deserialize)]
struct ReviewPayload {
    #[serde(default)]
    state: String,
    #[serde(default)]
    body: String,
}

#[derive(Debug, Deserialize)]
struct LabelPayload {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize)]
struct UserPayload {
    #[serde(default)]
    login: String,
}

#[derive(Debug, Deserialize)]
struct RepoPayload {
    #[serde(default)]
    full_name: String,
}

async fn handle_webhook(
    State(state): State<AppState>,
    Json(payload): Json<WebhookPayload>,
) -> StatusCode {
    debug!(?payload.action, "webhook received");

    let event = translate(&payload);
    if let Some(e) = event {
        debug!(?e, "publishing event");
        state.bus.publish(e);
    }

    StatusCode::OK
}

fn translate(payload: &WebhookPayload) -> Option<Event> {
    match payload.action.as_str() {
        // ── issues ──
        "opened" if payload.pull_request.is_none() => {
            let issue = payload.issue.as_ref()?;
            Some(Event::IssueOpened {
                number: IssueNumber(issue.number),
                title: issue.title.clone(),
            })
        }
        "labeled" if payload.pull_request.is_none() => {
            let issue = payload.issue.as_ref()?;
            let label = payload.label.as_ref().map(|l| l.name.clone()).unwrap_or_default();
            Some(Event::IssueLabeled {
                number: IssueNumber(issue.number),
                label,
            })
        }
        "unlabeled" if payload.pull_request.is_none() => {
            let issue = payload.issue.as_ref()?;
            let label = payload.label.as_ref().map(|l| l.name.clone()).unwrap_or_default();
            Some(Event::IssueUnlabeled {
                number: IssueNumber(issue.number),
                label,
            })
        }
        "closed" if payload.pull_request.is_none() => {
            let issue = payload.issue.as_ref()?;
            Some(Event::IssueClosed {
                number: IssueNumber(issue.number),
            })
        }
        // ── issue comments (NOT pull-request comments) ──
        "created" if payload.comment.is_some() && payload.pull_request.is_none() => {
            let comment = payload.comment.as_ref()?;
            let issue_number = payload
                .issue
                .as_ref()
                .map(|i| IssueNumber(i.number))
                .unwrap_or(IssueNumber(0));
            Some(Event::IssueComment {
                issue_number,
                comment_id: comment.id,
                user: comment
                    .user
                    .as_ref()
                    .map(|u| u.login.clone())
                    .unwrap_or_default(),
                body: comment.body.clone(),
            })
        }
        // ── pull-request comments (routed separately — reply stays on the PR) ──
        "created" if payload.comment.is_some() && payload.pull_request.is_some() => {
            let comment = payload.comment.as_ref()?;
            let pr_number = payload
                .pull_request
                .as_ref()
                .map(|p| PrNumber(p.number))
                .unwrap_or(PrNumber(0));
            Some(Event::PrComment {
                pr_number,
                comment_id: comment.id,
                user: comment
                    .user
                    .as_ref()
                    .map(|u| u.login.clone())
                    .unwrap_or_default(),
                body: comment.body.clone(),
            })
        }
        // ── pull requests ──
        "opened" if payload.pull_request.is_some() => {
            let pr = payload.pull_request.as_ref()?;
            Some(Event::PullRequestOpened {
                pr_number: PrNumber(pr.number),
                title: pr.title.clone(),
                head_branch: pr
                    .head
                    .as_ref()
                    .map(|h| h.label.clone())
                    .unwrap_or_default(),
            })
        }
        // New commits pushed to an open PR. Invalidates a chaos pass — see chaos_enforcer.
        "synchronized" | "synchronize" if payload.pull_request.is_some() => {
            let pr = payload.pull_request.as_ref()?;
            Some(Event::PullRequestSynchronized {
                pr_number: PrNumber(pr.number),
            })
        }
        "merged" if payload.pull_request.is_some() => {
            let pr = payload.pull_request.as_ref()?;
            Some(Event::PullRequestMerged {
                pr_number: PrNumber(pr.number),
            })
        }
        // ── pull request reviews ──
        "submitted" if payload.review.is_some() => {
            let review = payload.review.as_ref()?;
            let pr_number = payload
                .pull_request
                .as_ref()
                .map(|p| PrNumber(p.number))
                .unwrap_or(PrNumber(0));
            Some(Event::PullRequestReview {
                pr_number,
                state: review.state.clone(),
                body: review.body.clone(),
            })
        }
        _ => {
            debug!(action = payload.action, "unhandled webhook action");
            None
        }
    }
}
