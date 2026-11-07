use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DmConfigKeys {
    pub policy_key: String,
    pub allow_from_key: String,
}

#[derive(Debug, Clone)]
pub struct ChannelDmPolicyDescriptor {
    pub label: String,
    pub channel: String,
    pub policy_key: String,
    pub allow_from_key: String,
    pub policy_path: String,
    pub allow_from_path: String,
}

impl ChannelDmPolicyDescriptor {
    pub fn new(label: impl Into<String>, channel: impl Into<String>) -> Self {
        let ch = channel.into();
        Self {
            label: label.into(),
            policy_key: format!("channels.{ch}.dmPolicy"),
            allow_from_key: format!("channels.{ch}.allowFrom"),
            policy_path: "dmPolicy".to_string(),
            allow_from_path: "allowFrom".to_string(),
            channel: ch,
        }
    }

    pub fn with_custom_paths(
        mut self,
        policy_path: impl Into<String>,
        allow_from_path: impl Into<String>,
    ) -> Self {
        let pp = policy_path.into();
        let ap = allow_from_path.into();
        self.policy_key = format!("channels.{}.{pp}", self.channel);
        self.allow_from_key = format!("channels.{}.{ap}", self.channel);
        self.policy_path = pp;
        self.allow_from_path = ap;
        self
    }

    /// Resolve config keys for root or account-scoped configurations.
    pub fn resolve_config_keys(&self, _cfg: &Value, account_id: Option<&str>) -> DmConfigKeys {
        let root = format!("channels.{}", self.channel);
        match account_id {
            Some(acc) if acc != "default" && !acc.is_empty() => DmConfigKeys {
                policy_key: format!("{root}.accounts.{acc}.{}", self.policy_path),
                allow_from_key: format!("{root}.accounts.{acc}.{}", self.allow_from_path),
            },
            _ => DmConfigKeys {
                policy_key: format!("{root}.{}", self.policy_path),
                allow_from_key: format!("{root}.{}", self.allow_from_path),
            },
        }
    }

    /// Read current DM policy setting ("open", "pairing", "allowlist", "disabled"). Defaults to "pairing".
    pub fn get_current(&self, cfg: &Value, account_id: Option<&str>) -> String {
        let ch_cfg = cfg.get("channels").and_then(|c| c.get(&self.channel));

        let acc_cfg = match account_id {
            Some(acc) if acc != "default" && !acc.is_empty() => {
                ch_cfg.and_then(|c| c.get("accounts")).and_then(|a| a.get(acc))
            }
            _ => ch_cfg,
        };

        let read_policy = |obj: &Value| -> Option<String> {
            obj.get(&self.policy_path)
                .and_then(|v| v.as_str())
                .map(String::from)
                .or_else(|| {
                    if self.policy_path.contains('.') {
                        let parts: Vec<&str> = self.policy_path.split('.').collect();
                        let mut curr = obj;
                        for part in parts {
                            curr = curr.get(part)?;
                        }
                        curr.as_str().map(String::from)
                    } else {
                        None
                    }
                })
        };

        acc_cfg
            .and_then(read_policy)
            .or_else(|| ch_cfg.and_then(read_policy))
            .unwrap_or_else(|| "pairing".to_string())
    }

    /// Update policy and automatically apply wildcard allowlist when switching to "open".
    pub fn set_policy(&self, cfg: &mut Value, policy: &str, account_id: Option<&str>) {
        if !cfg.is_object() {
            *cfg = json!({});
        }

        let channels = cfg
            .as_object_mut()
            .unwrap()
            .entry("channels".to_string())
            .or_insert_with(|| json!({}));

        let channel_obj = channels
            .as_object_mut()
            .unwrap()
            .entry(self.channel.clone())
            .or_insert_with(|| json!({}));

        let target_obj = match account_id {
            Some(acc) if acc != "default" && !acc.is_empty() => {
                let accounts = channel_obj
                    .as_object_mut()
                    .unwrap()
                    .entry("accounts".to_string())
                    .or_insert_with(|| json!({}));
                accounts
                    .as_object_mut()
                    .unwrap()
                    .entry(acc.to_string())
                    .or_insert_with(|| json!({}))
            }
            _ => channel_obj,
        };

        if let Some(obj) = target_obj.as_object_mut() {
            obj.insert("enabled".to_string(), json!(true));

            // Set policy
            if self.policy_path.contains('.') {
                let parts: Vec<&str> = self.policy_path.split('.').collect();
                if parts.len() == 2 {
                    let nested = obj.entry(parts[0].to_string()).or_insert_with(|| json!({}));
                    if let Some(nested_obj) = nested.as_object_mut() {
                        nested_obj.insert(parts[1].to_string(), json!(policy));
                    }
                }
            } else {
                obj.insert(self.policy_path.clone(), json!(policy));
            }

            // Open policy automatically injects wildcard "*"
            if policy == "open" {
                let allow_from_field = if self.allow_from_path.contains('.') {
                    let parts: Vec<&str> = self.allow_from_path.split('.').collect();
                    if parts.len() == 2 {
                        let nested = obj.entry(parts[0].to_string()).or_insert_with(|| json!({}));
                        nested.as_object_mut().and_then(|no| no.get_mut(parts[1]))
                    } else {
                        obj.get_mut(&self.allow_from_path)
                    }
                } else {
                    obj.get_mut(&self.allow_from_path)
                };

                let mut list: Vec<Value> = match allow_from_field {
                    Some(Value::Array(arr)) => arr.clone(),
                    _ => Vec::new(),
                };

                if !list.iter().any(|v| v.as_str() == Some("*")) {
                    list.push(json!("*"));
                }

                if self.allow_from_path.contains('.') {
                    let parts: Vec<&str> = self.allow_from_path.split('.').collect();
                    if parts.len() == 2 {
                        let nested = obj.entry(parts[0].to_string()).or_insert_with(|| json!({}));
                        if let Some(nested_obj) = nested.as_object_mut() {
                            nested_obj.insert(parts[1].to_string(), Value::Array(list));
                        }
                    }
                } else {
                    obj.insert(self.allow_from_path.clone(), Value::Array(list));
                }
            }
        }
    }
}
