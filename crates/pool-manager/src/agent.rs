use reqwest::Client;
use tracing::{error, info, warn};

/// OpenCode serve API client — creates sessions and sends prompts to agent instances.
pub struct Agent {
    client: Client,
}

impl Agent {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    /// Port for the opencode serve instance of a given pool member.
    pub fn port(index: u32) -> u16 {
        (32000 + index) as u16
    }

    /// Create a new session on the agent's opencode serve.
    /// Returns the session ID on success.
    pub async fn create_session(&self, index: u32) -> Option<String> {
        let port = Self::port(index);
        let url = format!("http://localhost:{port}/api/session");

        for attempt in 0..6 {
            match self
                .client
                .post(&url)
                .json(&serde_json::json!({}))
                .send()
                .await
            {
                Ok(resp) => {
                    if let Ok(body) = resp.json::<serde_json::Value>().await {
                        let session_id = body
                            .get("data")
                            .and_then(|d| d.get("id"))
                            .or_else(|| body.get("id"))
                            .and_then(|v| v.as_str())
                            .map(String::from);
                        if session_id.is_some() {
                            return session_id;
                        }
                    }
                }
                Err(e) => {
                    let wait = (attempt + 1) * 5;
                    warn!(port, attempt, wait, "session not ready: {e}");
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs((attempt + 1) * 5)).await;
        }

        error!(port, "could not create session after 6 attempts");
        None
    }

    /// Send a prompt to the agent's existing session.
    pub async fn send_prompt(
        &self,
        index: u32,
        session_id: &str,
        text: &str,
    ) -> bool {
        let port = Self::port(index);
        let url = format!("http://localhost:{port}/api/session/{session_id}/prompt");

        match self
            .client
            .post(&url)
            .json(&serde_json::json!({"prompt": {"text": text}}))
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
        {
            Ok(_) => {
                info!(port, session_id, "prompt sent");
                true
            }
            Err(e) => {
                error!(port, session_id, error = %e, "failed to send prompt");
                false
            }
        }
    }

    /// Convenience: create session and send prompt in one call.
    pub async fn create_and_send(
        &self,
        index: u32,
        _name: &str,
        text: &str,
    ) -> bool {
        let session_id = match self.create_session(index).await {
            Some(id) => id,
            None => return false,
        };
        self.send_prompt(index, &session_id, text).await
    }

    /// Check if an agent instance is alive.
    pub async fn is_alive(&self, index: u32) -> bool {
        let port = Self::port(index);
        let url = format!("http://localhost:{port}/api/session");
        match self
            .client
            .post(&url)
            .json(&serde_json::json!({}))
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
        {
            Ok(resp) => {
                if let Ok(body) = resp.json::<serde_json::Value>().await {
                    body.get("data")
                        .and_then(|d| d.get("id"))
                        .or_else(|| body.get("id"))
                        .is_some()
                } else {
                    false
                }
            }
            Err(_) => false,
        }
    }
}

impl Default for Agent {
    fn default() -> Self {
        Self::new()
    }
}
