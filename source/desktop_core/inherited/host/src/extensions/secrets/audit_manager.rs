use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretFindingKind {
    PlaintextResidue,
    RedactedValue,
    UnresolvedSecretRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretAuditFinding {
    pub key_path: String,
    pub finding_kind: SecretFindingKind,
    pub message: String,
    pub suggested_secret_ref: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SecretsAuditManager {
    channel_env_vars: HashMap<String, Vec<String>>,
}

impl SecretsAuditManager {
    pub fn new() -> Self {
        let mut mgr = Self::default();
        // Initialize default channel env mappings
        mgr.register_channel_env_vars("discord", &["DISCORD_BOT_TOKEN"]);
        mgr.register_channel_env_vars("slack", &["SLACK_BOT_TOKEN", "SLACK_APP_TOKEN"]);
        mgr.register_channel_env_vars("telegram", &["TELEGRAM_BOT_TOKEN"]);
        mgr
    }

    pub fn register_channel_env_vars(&mut self, channel_id: &str, vars: &[&str]) {
        self.channel_env_vars.insert(
            channel_id.to_ascii_lowercase(),
            vars.iter().map(|s| s.to_string()).collect(),
        );
    }

    pub fn get_channel_env_vars(&self, channel_id: &str) -> Vec<String> {
        self.channel_env_vars
            .get(&channel_id.to_ascii_lowercase())
            .cloned()
            .unwrap_or_default()
    }

    pub fn audit_json_value(&self, root: &Value, prefix: &str) -> Vec<SecretAuditFinding> {
        let mut findings = Vec::new();
        Self::scan_recursive(root, prefix, &mut findings);
        findings
    }

    fn scan_recursive(val: &Value, path: &str, findings: &mut Vec<SecretAuditFinding>) {
        match val {
            Value::Object(map) => {
                for (k, v) in map {
                    let current_path = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };

                    if k.to_ascii_lowercase().contains("token")
                        || k.to_ascii_lowercase().contains("secret")
                        || k.to_ascii_lowercase().contains("api_key")
                        || k.to_ascii_lowercase().contains("apikey")
                    {
                        if let Some(s) = v.as_str() {
                            if s == "[REDACTED]" || s == "***" {
                                findings.push(SecretAuditFinding {
                                    key_path: current_path.clone(),
                                    finding_kind: SecretFindingKind::RedactedValue,
                                    message: format!("Key '{current_path}' contains placeholder redacted marker"),
                                    suggested_secret_ref: None,
                                });
                            } else if Self::is_likely_plaintext_secret(s) {
                                findings.push(SecretAuditFinding {
                                    key_path: current_path.clone(),
                                    finding_kind: SecretFindingKind::PlaintextResidue,
                                    message: format!("Key '{current_path}' contains plaintext secret residue"),
                                    suggested_secret_ref: Some(format!("secret:{k}")),
                                });
                            }
                        }
                    }

                    Self::scan_recursive(v, &current_path, findings);
                }
            }
            Value::Array(arr) => {
                for (idx, item) in arr.iter().enumerate() {
                    let current_path = format!("{path}[{idx}]");
                    Self::scan_recursive(item, &current_path, findings);
                }
            }
            _ => {}
        }
    }

    pub fn is_likely_plaintext_secret(val: &str) -> bool {
        let trimmed = val.trim();
        if trimmed.starts_with("sk-")
            || trimmed.starts_with("ghp_")
            || trimmed.starts_with("xoxb-")
            || trimmed.starts_with("xoxp-")
            || trimmed.starts_with("AIzaSy")
            || (trimmed.len() >= 32 && !trimmed.contains(' '))
        {
            return true;
        }
        false
    }
}
