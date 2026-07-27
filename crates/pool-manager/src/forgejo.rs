use crate::config::Config;
use base64::Engine;
use reqwest::Client;
use serde::Deserialize;
use std::os::unix::fs::PermissionsExt;
use tracing::{error, info, warn};

/// Forgejo REST API client.
pub struct Forgejo {
    client: Client,
    api: String,
    repo: String,
    auth64: String,
}

impl Forgejo {
    pub fn new(cfg: &Config) -> Self {
        Self {
            client: Client::new(),
            api: cfg.forgejo_api.clone(),
            repo: cfg.repo.clone(),
            auth64: cfg.forgejo_auth64(),
        }
    }

    fn auth_header(&self) -> String {
        format!("Basic {}", self.auth64)
    }

    fn auth_header_for(&self, user: &str, pass: &str) -> String {
        let creds = format!("{user}:{pass}");
        let b64 = base64::engine::general_purpose::STANDARD.encode(creds);
        format!("Basic {b64}")
    }

    // ── labels ──

    pub async fn load_labels(&self) -> Result<Vec<Label>, reqwest::Error> {
        let resp = self
            .client
            .get(format!("{}/repos/{}/labels", self.api, self.repo))
            .header("Authorization", self.auth_header())
            .send()
            .await?;
        resp.json().await
    }

    pub async fn ensure_labels(&self, labels: &[(String, String)]) {
        let existing = match self.load_labels().await {
            Ok(l) => l,
            Err(e) => {
                error!(error = %e, "failed to load labels");
                return;
            }
        };
        let existing_names: std::collections::HashSet<String> =
            existing.into_iter().map(|l| l.name).collect();

        for (name, desc) in labels {
            if !existing_names.contains(name) {
                info!(name, "creating label");
                let _ = self
                    .client
                    .post(format!("{}/repos/{}/labels", self.api, self.repo))
                    .header("Authorization", self.auth_header())
                    .json(&serde_json::json!({
                        "name": name,
                        "color": "#2b7489",
                        "description": desc,
                    }))
                    .send()
                    .await;
            }
        }
    }

    pub async fn get_label_ids(&self) -> Result<std::collections::HashMap<String, u64>, reqwest::Error> {
        let labels = self.load_labels().await?;
        Ok(labels.into_iter().map(|l| (l.name, l.id)).collect())
    }

    pub async fn set_issue_labels(
        &self,
        issue_number: u64,
        label_names: &[String],
    ) -> Result<(), reqwest::Error> {
        let label_map = self.get_label_ids().await.unwrap_or_default();
        let ids: Vec<u64> = label_names
            .iter()
            .filter_map(|n| label_map.get(n).copied())
            .collect();
        if ids.is_empty() {
            return Ok(());
        }
        self.client
            .put(format!(
                "{}/repos/{}/issues/{}/labels",
                self.api, self.repo, issue_number
            ))
            .header("Authorization", self.auth_header())
            .json(&serde_json::json!({"labels": ids}))
            .send()
            .await?;
        Ok(())
    }

    /// Add labels without disturbing the ones already on the issue.
    ///
    /// `set_issue_labels` REPLACES the whole set, which would drop `in-progress` every time
    /// the chaos gate recorded a verdict. The gate needs add/remove, not replace.
    pub async fn add_issue_labels(
        &self,
        issue_number: u64,
        label_names: &[String],
    ) -> Result<(), reqwest::Error> {
        let label_map = self.get_label_ids().await.unwrap_or_default();
        let ids: Vec<u64> = label_names
            .iter()
            .filter_map(|n| label_map.get(n).copied())
            .collect();
        if ids.is_empty() {
            return Ok(());
        }
        self.client
            .post(format!(
                "{}/repos/{}/issues/{}/labels",
                self.api, self.repo, issue_number
            ))
            .header("Authorization", self.auth_header())
            .json(&serde_json::json!({"labels": ids}))
            .send()
            .await?;
        Ok(())
    }

    pub async fn remove_issue_label(&self, issue_number: u64, label_name: &str) {
        let label_map = self.get_label_ids().await.unwrap_or_default();
        let Some(id) = label_map.get(label_name).copied() else {
            return;
        };
        let _ = self
            .client
            .delete(format!(
                "{}/repos/{}/issues/{}/labels/{}",
                self.api, self.repo, issue_number, id
            ))
            .header("Authorization", self.auth_header())
            .send()
            .await;
    }

    /// True when the issue currently carries `label`. The chaos gate's read side.
    pub async fn issue_has_label(&self, issue_number: u64, label: &str) -> bool {
        match self.get_issue(issue_number).await {
            Ok(Some(issue)) => issue.labels.iter().any(|l| l.name == label),
            _ => false,
        }
    }

    // ── issues ──

    pub async fn get_issue(&self, issue_number: u64) -> Result<Option<Issue>, reqwest::Error> {
        let resp = self
            .client
            .get(format!(
                "{}/repos/{}/issues/{}",
                self.api, self.repo, issue_number
            ))
            .header("Authorization", self.auth_header())
            .send()
            .await?;
        if !resp.status().is_success() {
            return Ok(None);
        }
        Ok(Some(resp.json().await?))
    }

    pub async fn get_issues(
        &self,
        state: &str,
        labels: Option<&str>,
    ) -> Result<Vec<Issue>, reqwest::Error> {
        let mut url = format!(
            "{}/repos/{}/issues?state={state}",
            self.api, self.repo
        );
        if let Some(l) = labels {
            url.push_str(&format!("&labels={l}"));
        }
        let resp = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await?;
        resp.json().await
    }

    // ── pull requests ──

    pub async fn get_pull_requests(
        &self,
        state: &str,
    ) -> Result<Vec<PullRequest>, reqwest::Error> {
        let resp = self
            .client
            .get(format!(
                "{}/repos/{}/pulls?state={state}&sort=desc",
                self.api, self.repo
            ))
            .header("Authorization", self.auth_header())
            .send()
            .await?;
        resp.json().await
    }

    pub async fn get_reviews(
        &self,
        pr_number: u64,
    ) -> Result<Vec<Review>, reqwest::Error> {
        let resp = self
            .client
            .get(format!(
                "{}/repos/{}/pulls/{}/reviews",
                self.api, self.repo, pr_number
            ))
            .header("Authorization", self.auth_header())
            .send()
            .await?;
        resp.json().await
    }

    /// Post a review on a PR as the operator.
    ///
    /// `REQUEST_CHANGES` here is the chaos gate's hard edge: combined with Forgejo branch
    /// protection ("block merge on rejected reviews") it is the one layer the agent cannot
    /// route around, because the agent merges with its own credentials via the API.
    pub async fn post_review(
        &self,
        pr_number: u64,
        state: &str,
        body: &str,
    ) -> Result<(), reqwest::Error> {
        self.client
            .post(format!(
                "{}/repos/{}/pulls/{}/reviews",
                self.api, self.repo, pr_number
            ))
            .header("Authorization", self.auth_header())
            .json(&serde_json::json!({"event": state, "body": body}))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    // ── comments ──

    pub async fn get_issue_comments(
        &self,
        issue_number: u64,
    ) -> Result<Vec<Comment>, reqwest::Error> {
        let resp = self
            .client
            .get(format!(
                "{}/repos/{}/issues/{}/comments",
                self.api, self.repo, issue_number
            ))
            .header("Authorization", self.auth_header())
            .send()
            .await?;
        resp.json().await
    }

    pub async fn post_issue_comment(
        &self,
        issue_number: u64,
        body: &str,
    ) -> Result<(), reqwest::Error> {
        self.client
            .post(format!(
                "{}/repos/{}/issues/{}/comments",
                self.api, self.repo, issue_number
            ))
            .header("Authorization", self.auth_header())
            .json(&serde_json::json!({"body": body}))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// Post a comment as a specific agent identity (not the admin).
    pub async fn post_comment_as_agent(
        &self,
        agent_name: &str,
        index: u32,
        issue_number: u64,
        body: &str,
    ) -> Result<(), reqwest::Error> {
        let pass = format!("pool-agent-{index}-pass");
        let auth = self.auth_header_for(agent_name, &pass);
        self.client
            .post(format!(
                "{}/repos/{}/issues/{}/comments",
                self.api, self.repo, issue_number
            ))
            .header("Authorization", &auth)
            .json(&serde_json::json!({"body": body}))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    // ── agent identity management ──

    pub async fn create_agent_user(&self, index: u32) -> Option<(String, String)> {
        let name = format!("pool-{index}");
        let pass = format!("pool-agent-{index}-pass");

        info!(name, "creating Forgejo user");

        // Delete stale user first (idempotent)
        let _ = self
            .client
            .delete(format!("{}/admin/users/{name}", self.api))
            .header("Authorization", self.auth_header())
            .send()
            .await;

        let status = self
            .client
            .post(format!("{}/admin/users", self.api))
            .header("Authorization", self.auth_header())
            .json(&serde_json::json!({
                "username": name,
                "email": format!("{name}@agent.local"),
                "password": pass,
                "login_name": name,
                "must_change_password": false,
            }))
            .send()
            .await
            .map(|r| r.status().as_u16());

        match status {
            Ok(200) | Ok(201) => {}
            other => {
                error!(name, ?other, "failed to create user");
                return None;
            }
        }

        // Grant write access to repo
        let _ = self
            .client
            .put(format!("{}/repos/{}/collaborators/{name}", self.api, self.repo))
            .header("Authorization", self.auth_header())
            .json(&serde_json::json!({"permission": "write"}))
            .send()
            .await;

        // Create access token
        let agent_auth = self.auth_header_for(&name, &pass);
        let _ = self
            .client
            .post(format!("{}/users/{name}/tokens", self.api))
            .header("Authorization", &agent_auth)
            .json(&serde_json::json!({
                "name": "agent-token",
                "scopes": ["all"],
            }))
            .send()
            .await;

        info!(name, "Forgejo identity ready");
        Some((name, pass))
    }

    pub async fn delete_agent_user(&self, name: &str) {
        info!(name, "deleting Forgejo user");
        match self
            .client
            .delete(format!("{}/admin/users/{name}", self.api))
            .header("Authorization", self.auth_header())
            .send()
            .await
        {
            Ok(resp) if resp.status().as_u16() == 204 => {
                info!(name, "Forgejo user deleted");
            }
            Ok(resp) => {
                warn!(name, status = resp.status().as_u16(), "unexpected status deleting user");
            }
            Err(e) => {
                error!(name, error = %e, "failed to delete user");
            }
        }
    }

    pub async fn write_agent_creds(&self, workspace: &str, name: &str, index: u32) {
        let pass = format!("pool-agent-{index}-pass");
        let creds_path = std::path::Path::new(workspace).join(".forgejo");
        let content = format!(
            "FORGEJO_USER={name}\nFORGEJO_PASSWORD={pass}\nFORGEJO_URL=http://forgejo:3000\n"
        );
        if let Err(e) = std::fs::write(&creds_path, &content) {
            error!(path = %creds_path.display(), error = %e, "failed to write creds");
        } else {
            let _ = std::fs::set_permissions(&creds_path, std::fs::Permissions::from_mode(0o644));
            info!(name, "wrote creds to workspace");
        }

        // Write .gitignore so the agent never commits dev-environment files.
        self.write_workspace_gitignore(workspace);
    }

    fn write_workspace_gitignore(&self, workspace: &str) {
        let path = std::path::Path::new(workspace).join(".gitignore");
        let gitignore = "\
# Agent workspace files — never commit
.forgejo
.env
.agent-name
.devcontainer/
.opencode/
AGENTS.md
";
        if let Err(e) = std::fs::write(&path, gitignore) {
            error!(path = %path.display(), error = %e, "failed to write .gitignore");
        } else {
            info!("wrote .gitignore to workspace");
        }
    }
}

// ── Forgejo API types ──

#[derive(Debug, Clone, Deserialize)]
pub struct Label {
    pub id: u64,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    pub state: String,
    #[serde(default)]
    pub labels: Vec<Label>,
}

#[derive(Debug, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    pub state: String,
    #[serde(default)]
    pub head: Option<BranchRef>,
}

#[derive(Debug, Deserialize)]
pub struct BranchRef {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub r#ref: Option<String>,
    /// Watched by the reconciler: a changed head SHA is a push, which invalidates a chaos pass.
    #[serde(default)]
    pub sha: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Review {
    pub id: u64,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub user: Option<User>,
}

#[derive(Debug, Deserialize)]
pub struct Comment {
    pub id: u64,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub user: Option<User>,
}

#[derive(Debug, Deserialize)]
pub struct User {
    #[serde(default)]
    pub login: String,
}
