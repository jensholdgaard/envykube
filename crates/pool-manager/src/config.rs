use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    #[serde(rename = "minAvailable")]
    pub min_available: usize,
    #[serde(rename = "poolBase")]
    pub pool_base: u32,
    pub repo: String,
    #[serde(rename = "repoUrl")]
    pub repo_url: String,
    #[serde(rename = "forgejoApi")]
    pub forgejo_api: String,
    #[serde(rename = "pollInterval")]
    pub poll_interval_secs: u64,
    #[serde(rename = "stateDir")]
    pub state_dir: String,
    pub labels: Labels,
    #[serde(rename = "webhookPort", default = "default_webhook_port")]
    pub webhook_port: u16,
    #[serde(default)]
    pub webhook_secret: Option<String>,
    #[serde(rename = "otlpEndpoint", default)]
    pub otlp_endpoint: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Labels {
    pub ready: String,
    #[serde(rename = "inProgress")]
    pub in_progress: String,
    pub review: String,
    /// The chaos gate's verdict. Defaulted so an older pool-config.json still parses —
    /// `main::ensure_labels` creates whatever is missing in Forgejo on boot.
    #[serde(rename = "chaosPassed", default = "default_chaos_passed")]
    pub chaos_passed: String,
    #[serde(rename = "chaosFailed", default = "default_chaos_failed")]
    pub chaos_failed: String,
}

fn default_chaos_passed() -> String {
    "chaos-passed".into()
}

fn default_chaos_failed() -> String {
    "chaos-failed".into()
}

fn default_webhook_port() -> u16 {
    30990
}

impl Config {
    pub fn forgejo_auth64(&self) -> String {
        use base64::Engine;
        let user = std::env::var("FORGEJO_USER").unwrap_or_else(|_| "adminadmin".into());
        let pass = std::env::var("FORGEJO_PASSWORD").unwrap_or_else(|_| "adminadmin".into());
        let creds = format!("{user}:{pass}");
        base64::engine::general_purpose::STANDARD.encode(creds)
    }
}

pub fn load(path: &Path) -> anyhow::Result<Config> {
    let raw = std::fs::read_to_string(path)?;
    let cfg: Config = serde_json::from_str(&raw)?;
    Ok(cfg)
}

pub fn resolve_state_path(state_dir: &str) -> PathBuf {
    if let Some(home) = dirs_next(state_dir) {
        PathBuf::from(home).join("state.json")
    } else {
        PathBuf::from(state_dir).join("state.json")
    }
}

fn dirs_next(path: &str) -> Option<String> {
    if path.starts_with('~') {
        let home = std::env::var("HOME").ok()?;
        Some(path.replacen('~', &home, 1))
    } else {
        None
    }
}
