use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalBinding {
    pub token: String,
    pub action_name: String,
    pub session_id: String,
    pub approver_id: Option<String>,
    pub expires_at_ms: u64,
}

#[derive(Clone, Default)]
pub struct NativeApprovalControlRegistry {
    bindings: Arc<Mutex<HashMap<String, ApprovalBinding>>>,
    resolving: Arc<Mutex<HashSet<String>>>,
}

impl NativeApprovalControlRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn generate_token() -> Result<String, String> {
        let mut buf = [0u8; 18];
        getrandom::fill(&mut buf).map_err(|e| format!("Random generation failed: {e}"))?;
        Ok(URL_SAFE_NO_PAD.encode(buf))
    }

    pub fn register(&self, binding: ApprovalBinding) -> bool {
        let now_ms = current_time_ms();
        if binding.expires_at_ms <= now_ms {
            return false;
        }

        let mut bindings = self.bindings.lock().unwrap();
        bindings.insert(binding.token.clone(), binding);
        true
    }

    pub fn get(&self, token: &str) -> Option<ApprovalBinding> {
        let mut bindings = self.bindings.lock().unwrap();
        if let Some(binding) = bindings.get(token) {
            let now_ms = current_time_ms();
            if binding.expires_at_ms <= now_ms {
                bindings.remove(token);
                let mut resolving = self.resolving.lock().unwrap();
                resolving.remove(token);
                None
            } else {
                Some(binding.clone())
            }
        } else {
            None
        }
    }

    pub fn complete(&self, token: &str) -> Option<ApprovalBinding> {
        let mut bindings = self.bindings.lock().unwrap();
        let mut resolving = self.resolving.lock().unwrap();
        resolving.remove(token);
        bindings.remove(token)
    }

    pub fn prune_expired(&self) -> usize {
        let now_ms = current_time_ms();
        let mut bindings = self.bindings.lock().unwrap();
        let mut resolving = self.resolving.lock().unwrap();

        let before = bindings.len();
        bindings.retain(|token, b| {
            if b.expires_at_ms <= now_ms {
                resolving.remove(token);
                false
            } else {
                true
            }
        });
        before - bindings.len()
    }
}

pub fn resolve_approval_approvers(
    explicit: Option<&[String]>,
    allow_from: Option<&[String]>,
    extra_allow_from: Option<&[String]>,
    default_to: Option<&str>,
) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();

    // Explicit approvers take absolute precedence
    if let Some(explicit_list) = explicit {
        for item in explicit_list {
            let trimmed = item.trim();
            if !trimmed.is_empty() && seen.insert(trimmed.to_ascii_lowercase()) {
                result.push(trimmed.to_string());
            }
        }
        if !result.is_empty() {
            return result;
        }
    }

    // Fallbacks
    if let Some(list) = allow_from {
        for item in list {
            let trimmed = item.trim();
            if !trimmed.is_empty() && seen.insert(trimmed.to_ascii_lowercase()) {
                result.push(trimmed.to_string());
            }
        }
    }

    if let Some(list) = extra_allow_from {
        for item in list {
            let trimmed = item.trim();
            if !trimmed.is_empty() && seen.insert(trimmed.to_ascii_lowercase()) {
                result.push(trimmed.to_string());
            }
        }
    }

    if let Some(def) = default_to {
        let trimmed = def.trim();
        if !trimmed.is_empty() && seen.insert(trimmed.to_ascii_lowercase()) {
            result.push(trimmed.to_string());
        }
    }

    result
}

// --- Exec Approval Decision & Reaction System ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecApprovalReplyDecision {
    AllowOnce,
    AllowAlways,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalReactionDecisionBinding {
    pub decision: ExecApprovalReplyDecision,
    pub emoji: &'static str,
    pub label: &'static str,
}

pub const APPROVAL_REACTION_BINDINGS: &[ApprovalReactionDecisionBinding] = &[
    ApprovalReactionDecisionBinding {
        decision: ExecApprovalReplyDecision::AllowOnce,
        emoji: "👍",
        label: "Allow Once",
    },
    ApprovalReactionDecisionBinding {
        decision: ExecApprovalReplyDecision::AllowAlways,
        emoji: "♾️",
        label: "Allow Always",
    },
    ApprovalReactionDecisionBinding {
        decision: ExecApprovalReplyDecision::Deny,
        emoji: "👎",
        label: "Deny",
    },
];

pub fn normalize_approval_reaction_emoji(reaction_key: &str) -> String {
    let mut normalized = reaction_key
        .trim()
        .chars()
        .filter(|&c| {
            // Remove variation selectors \u{FE0E} and \u{FE0F}
            c != '\u{FE0E}' && c != '\u{FE0F}'
                // Remove Fitzpatrick skin tone modifiers \u{1F3FB}..=\u{1F3FF}
                && !(c >= '\u{1F3FB}' && c <= '\u{1F3FF}')
        })
        .collect::<String>();

    if normalized == "♾" {
        normalized = "♾️".to_string();
    }
    normalized
}

pub fn resolve_approval_reaction_decision(
    reaction_key: &str,
    allowed_decisions: &[ExecApprovalReplyDecision],
) -> Option<ExecApprovalReplyDecision> {
    let normalized = normalize_approval_reaction_emoji(reaction_key);
    if normalized.is_empty() {
        return None;
    }

    let allowed_set: HashSet<ExecApprovalReplyDecision> = allowed_decisions.iter().copied().collect();
    for binding in APPROVAL_REACTION_BINDINGS {
        if binding.emoji == normalized && allowed_set.contains(&binding.decision) {
            return Some(binding.decision);
        }
    }
    None
}

pub fn build_approval_reaction_hint(allowed_decisions: &[ExecApprovalReplyDecision]) -> Option<String> {
    let allowed_set: HashSet<ExecApprovalReplyDecision> = allowed_decisions.iter().copied().collect();
    let bindings: Vec<_> = APPROVAL_REACTION_BINDINGS
        .iter()
        .filter(|b| allowed_set.contains(&b.decision))
        .collect();

    if bindings.is_empty() {
        return None;
    }

    let lines: Vec<String> = bindings
        .iter()
        .map(|b| format!("{} {}", b.emoji, b.label))
        .collect();
    Some(format!("React with:\n\n{}", lines.join("\n")))
}

pub fn has_approval_reaction_hint_text(text: &str) -> bool {
    text.to_lowercase().contains("react with:")
}

pub fn format_approval_decision_label(decision: &str) -> &'static str {
    match decision {
        "allow-once" => "Approved (once)",
        "allow-always" => "Approved (always)",
        "deny" => "Denied",
        "expired" => "Expired",
        _ => "Resolved",
    }
}

pub const SYSTEM_AGENT_APPROVAL_EXPIRED_TEXT: &str = "⏱️ OpenClaw change expired. No change was made.";

// --- Implicit Same-Chat Authorization ---

#[derive(Debug, Clone, Default)]
pub struct ImplicitSameChatApprovalRegistry {
    authorized_sessions: Arc<Mutex<HashSet<String>>>,
}

impl ImplicitSameChatApprovalRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mark_implicit_authorization(&self, session_key: &str) {
        self.authorized_sessions.lock().unwrap().insert(session_key.to_string());
    }

    pub fn is_implicit_authorized(&self, session_key: &str) -> bool {
        self.authorized_sessions.lock().unwrap().contains(session_key)
    }

    pub fn clear(&self, session_key: &str) {
        self.authorized_sessions.lock().unwrap().remove(session_key);
    }
}

// --- Outbound SSRF Protection ---

#[derive(Debug, PartialEq, Eq)]
pub enum SsrfCheckResult {
    Allowed,
    Disallowed(String),
}

pub fn validate_outbound_url(url_str: &str) -> SsrfCheckResult {
    let parsed = match url_str.parse::<url::Url>() {
        Ok(u) => u,
        Err(e) => return SsrfCheckResult::Disallowed(format!("Invalid URL syntax: {e}")),
    };

    let scheme = parsed.scheme().to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return SsrfCheckResult::Disallowed(format!("Disallowed scheme: '{scheme}' (must be http or https)"));
    }

    let host = match parsed.host_str() {
        Some(h) => h.to_ascii_lowercase(),
        None => return SsrfCheckResult::Disallowed("Missing host component".to_string()),
    };

    // Check host string
    if host == "localhost" || host.ends_with(".localhost") || host == "local" || host.ends_with(".local") {
        return SsrfCheckResult::Disallowed(format!("SSRF protection: loopback hostname '{host}' is prohibited"));
    }

    // Check if host is an IP address
    if let Ok(ip) = host.parse::<IpAddr>() {
        match ip {
            IpAddr::V4(ipv4) => {
                let octets = ipv4.octets();
                // 127.0.0.0/8 (loopback)
                if octets[0] == 127 {
                    return SsrfCheckResult::Disallowed(format!("SSRF protection: loopback IPv4 '{ip}' is prohibited"));
                }
                // 10.0.0.0/8 (private)
                if octets[0] == 10 {
                    return SsrfCheckResult::Disallowed(format!("SSRF protection: private RFC1918 IPv4 '{ip}' is prohibited"));
                }
                // 172.16.0.0/12 (private)
                if octets[0] == 172 && (16..=31).contains(&octets[1]) {
                    return SsrfCheckResult::Disallowed(format!("SSRF protection: private RFC1918 IPv4 '{ip}' is prohibited"));
                }
                // 192.168.0.0/16 (private)
                if octets[0] == 192 && octets[1] == 168 {
                    return SsrfCheckResult::Disallowed(format!("SSRF protection: private RFC1918 IPv4 '{ip}' is prohibited"));
                }
                // 169.254.0.0/16 (link-local, cloud metadata)
                if octets[0] == 169 && octets[1] == 254 {
                    return SsrfCheckResult::Disallowed(format!("SSRF protection: link-local IPv4 '{ip}' is prohibited"));
                }
                // 0.0.0.0/8
                if octets[0] == 0 {
                    return SsrfCheckResult::Disallowed(format!("SSRF protection: unspecified IPv4 '{ip}' is prohibited"));
                }
            }
            IpAddr::V6(ipv6) => {
                if ipv6.is_loopback() {
                    return SsrfCheckResult::Disallowed(format!("SSRF protection: loopback IPv6 '{ip}' is prohibited"));
                }
                if ipv6.is_unspecified() {
                    return SsrfCheckResult::Disallowed(format!("SSRF protection: unspecified IPv6 '{ip}' is prohibited"));
                }
            }
        }
    }

    SsrfCheckResult::Allowed
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
