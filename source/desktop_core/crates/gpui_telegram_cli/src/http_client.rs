use crate::error::CliError;
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Debug, Clone)]
pub enum ActionHttpOutcome {
    Executed(Value),
    ApprovalRequired {
        gate_id: String,
        expires_at_ms: u64,
        message: String,
    },
}

pub struct ActionHttpClient {
    base_url: String,
    client: Client,
}

impl ActionHttpClient {
    pub fn new(base_url: &str) -> Result<Self, CliError> {
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| CliError::Http(format!("Failed to initialize HTTP client: {e}")))?;

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
        })
    }

    pub async fn list_actions(&self) -> Result<Value, CliError> {
        let url = format!("{}/api/v1/actions", self.base_url);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| CliError::Http(format!("GET {url} failed: {e}")))?;

        let status = resp.status().as_u16();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| CliError::Http(format!("Failed to parse JSON from {url}: {e}")))?;

        if (200..300).contains(&status) {
            Ok(body)
        } else {
            Err(CliError::ServerError { status, body })
        }
    }

    pub async fn get_action_schema(&self, action_name: &str) -> Result<Value, CliError> {
        let url = format!("{}/api/v1/actions/{}/schema", self.base_url, action_name);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| CliError::Http(format!("GET {url} failed: {e}")))?;

        let status = resp.status().as_u16();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| CliError::Http(format!("Failed to parse JSON from {url}: {e}")))?;

        if (200..300).contains(&status) {
            Ok(body)
        } else if status == 404 {
            Err(CliError::UnknownCommand(
                body.get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Action not found")
                    .to_string(),
            ))
        } else {
            Err(CliError::ServerError { status, body })
        }
    }

    pub async fn post_action(
        &self,
        action_name: &str,
        token: Option<&str>,
        approval_token: Option<&str>,
        input: &Value,
    ) -> Result<ActionHttpOutcome, CliError> {
        let url = format!("{}/api/v1/actions/{}", self.base_url, action_name);
        let mut req = self.client.post(&url).json(input);
        if let Some(tok) = token {
            req = req.bearer_auth(tok);
        }
        if let Some(app_tok) = approval_token {
            req = req.header("X-Approval-Token", app_tok);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| CliError::Http(format!("POST {url} failed: {e}")))?;

        let status = resp.status().as_u16();
        let body: Value = resp
            .json()
            .await
            .unwrap_or_else(|_| json!({ "error": "invalid_response_body" }));

        if (200..300).contains(&status) {
            return Ok(ActionHttpOutcome::Executed(body));
        }

        if status == 401 {
            let msg = body
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Authentication required or token rejected")
                .to_string();
            return Err(CliError::Unauthorized(msg));
        }

        if status == 403 {
            if body.get("error").and_then(|v| v.as_str()) == Some("Approval required") {
                if let Some(gate_id) = body.get("gate_id").and_then(|v| v.as_str()) {
                    let expires_at_ms = body
                        .get("expires_at_ms")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let message = body
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Action requires explicit approval")
                        .to_string();
                    return Ok(ActionHttpOutcome::ApprovalRequired {
                        gate_id: gate_id.to_string(),
                        expires_at_ms,
                        message,
                    });
                }
            }
            return Err(CliError::Forbidden(body.to_string()));
        }

        Err(CliError::ServerError { status, body })
    }

    pub async fn approve_gate(
        &self,
        gate_id: &str,
        token: &str,
        decision: &str,
        reason: Option<&str>,
    ) -> Result<Value, CliError> {
        let url = format!(
            "{}/api/v1/actions/approvals/{}/approve",
            self.base_url, gate_id
        );
        let mut payload = json!({ "decision": decision });
        if let Some(r) = reason {
            payload["reason"] = Value::String(r.to_string());
        }

        let resp = self
            .client
            .post(&url)
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await
            .map_err(|e| CliError::Http(format!("POST {url} failed: {e}")))?;

        let status = resp.status().as_u16();
        let body: Value = resp
            .json()
            .await
            .unwrap_or_else(|_| json!({ "error": "invalid_response_body" }));

        if (200..300).contains(&status) {
            Ok(body)
        } else if status == 401 {
            let msg = body
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Authentication required or token rejected")
                .to_string();
            Err(CliError::Unauthorized(msg))
        } else if status == 403 {
            Err(CliError::Forbidden(body.to_string()))
        } else {
            Err(CliError::ServerError { status, body })
        }
    }
}
