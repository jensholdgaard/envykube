use std::path::PathBuf;
use tokio::process::Command;
use tracing::{error, info, warn};

/// Ensure a vCluster exists for the named member. Idempotent: if the namespace
/// already exists, the provision is skipped and the call succeeds immediately.
/// Returns false only if the vCluster genuinely cannot be created.
pub async fn provision_member(name: &str, index: u32) -> bool {
    // Idempotency guard: if the namespace already exists, we're done.
    if member_exists(name).await {
        info!(name, "vCluster namespace already exists — provision skipped");
        return true;
    }

    // Namespace doesn't exist, but vcluster may have stale local state from a
    // previous nuke. Clean that before attempting creation.
    clean_stale_vcluster(name).await;

    info!(name, index, "provisioning vCluster");
    let output = Command::new("just")
        .args(["provision", name, &index.to_string()])
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            info!(name, "vCluster provisioned");
            true
        }
        Ok(out) => {
            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr),
            );
            error!(name, %combined, "provision failed");
            false
        }
        Err(e) => {
            error!(name, error = %e, "provision command failed");
            false
        }
    }
}

/// Clean up vCluster's local per-instance state on disk.
/// Idempotent — safe to call even if the state is already gone.
pub async fn clean_stale_vcluster(name: &str) {
    let ns = format!("vc-{name}");
    info!(name, "cleaning vcluster local state");
    let _ = Command::new("vcluster")
        .args(["delete", name, "-n", &ns])
        .output()
        .await;
    let _ = Command::new("kubectl")
        .args(["delete", "ns", &ns, "--ignore-not-found"])
        .output()
        .await;
}

/// Tear down a vCluster and its workspace. Idempotent — `just deprovision` uses
/// `|| true` internally, so double-destroy is harmless.
pub async fn destroy_member(name: &str) {
    info!(name, "destroying vCluster");
    match Command::new("just")
        .args(["deprovision", name])
        .output()
        .await
    {
        Ok(out) => {
            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr),
            );
            if combined.contains("torn down") || out.status.success() {
                info!(name, "vCluster destroyed");
            } else {
                warn!(name, output = %combined, "deprovision uncertain");
                clean_stale_vcluster(name).await;
            }
        }
        Err(e) => {
            error!(name, error = %e, "deprovision command failed");
            clean_stale_vcluster(name).await;
        }
    }
}

/// Bind a workspace (DevContainer + git clone) to a vCluster.
///
/// Idempotent, but the guard checks the **DevContainer**, not just the directory: a leftover
/// clone from a previous card would otherwise skip `just workspace` entirely, and the claim
/// would "succeed" with no container — so every prompt sent to the agent hits a refused
/// connection on its opencode port and the card silently goes nowhere.
pub async fn claim_member(name: &str, index: u32, repo_url: &str) -> (bool, Option<String>) {
    let existing = existing_workspace_path(name);
    if let Some(ref ws) = existing {
        if devcontainer_running(ws).await {
            info!(name, ?ws, "workspace and DevContainer already up — claim skipped");
            return (true, existing.clone());
        }
        warn!(
            name,
            ?ws, "workspace exists but its DevContainer is not running — re-claiming"
        );
    }

    info!(name, index, repo_url, "claiming vCluster with workspace");
    match Command::new("just")
        .args(["workspace", name, &index.to_string(), repo_url])
        .output()
        .await
    {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let ws_path = stdout
                .lines()
                .find(|l| l.contains("Workspace ready at"))
                .and_then(|l| l.split("Workspace ready at").nth(1))
                .map(|s| s.trim().to_string());
            info!(name, ?ws_path, "workspace claimed");
            (true, ws_path)
        }
        Ok(out) => {
            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr),
            );
            error!(name, %combined, "workspace claim failed");
            (false, None)
        }
        Err(e) => {
            error!(name, error = %e, "workspace command failed");
            (false, None)
        }
    }
}

/// Check if a vCluster namespace exists via kubectl.
pub async fn member_exists(name: &str) -> bool {
    match Command::new("kubectl")
        .args([
            "get",
            "ns",
            &format!("vc-{name}"),
            "--no-headers",
            "--ignore-not-found",
        ])
        .output()
        .await
    {
        Ok(out) => !out.stdout.is_empty(),
        Err(_) => false,
    }
}

fn workspace_root(name: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home)
        .join(".local/share/agent-workspaces")
        .join(name)
}

/// Returns the workspace path if it already contains a `.git` directory.
fn existing_workspace_path(name: &str) -> Option<String> {
    let root = workspace_root(name);
    if root.join(".git").is_dir() {
        Some(root.display().to_string())
    } else {
        None
    }
}

/// Is a DevContainer actually running for this workspace? `just workspace` labels the
/// container with `devcontainer.local_folder`, which is what `just deprovision` matches on too.
async fn devcontainer_running(workspace: &str) -> bool {
    let abs = std::fs::canonicalize(workspace)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| workspace.to_string());
    match Command::new("docker")
        .args([
            "ps",
            "-q",
            "--filter",
            &format!("label=devcontainer.local_folder={abs}"),
        ])
        .output()
        .await
    {
        Ok(out) => !out.stdout.is_empty(),
        Err(_) => false,
    }
}
