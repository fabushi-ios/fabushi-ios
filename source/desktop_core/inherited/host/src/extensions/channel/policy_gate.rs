//! Access control list, DM/group policy gating, and candidate normalization (OC-S3).

use serde::{Deserialize, Serialize};

/// Policy controlling direct messaging (DM) admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelDmPolicy {
    #[default]
    Pairing,
    Open,
    Allowlist,
    Disabled,
}

/// Policy controlling group chat or channel admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelGroupPolicy {
    #[default]
    Open,
    Allowlist,
    RequireMention,
    Disabled,
}

/// Match outcome from evaluating an allowlist policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AllowlistMatch {
    Exact(String),
    Wildcard,
    Denied,
    NotMatched,
}

impl AllowlistMatch {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Exact(_) | Self::Wildcard)
    }
}

/// Normalizes sender identities (usernames, IDs, phone numbers) for canonical matching.
pub fn normalize_sender_id(sender: &str) -> String {
    let trimmed = sender.trim().to_ascii_lowercase();
    // Strip leading @ or common prefixes like user: or id:
    let stripped = trimmed
        .strip_prefix('@')
        .or_else(|| trimmed.strip_prefix("user:"))
        .or_else(|| trimmed.strip_prefix("id:"))
        .unwrap_or(&trimmed);

    // If it looks like a phone number (+123456789), strip whitespace, dashes, and parentheses
    if stripped.starts_with('+') || stripped.chars().all(|c| c.is_ascii_digit() || " -().".contains(c)) {
        let digits_only: String = stripped.chars().filter(|c| c.is_ascii_digit() || *c == '+').collect();
        if !digits_only.is_empty() {
            return digits_only;
        }
    }

    stripped.to_string()
}

/// Evaluates inbound DM sender admission against the configured channel DM policy.
pub fn evaluate_dm_policy(
    policy: ChannelDmPolicy,
    sender_id: &str,
    allowlist: &[String],
) -> AllowlistMatch {
    match policy {
        ChannelDmPolicy::Disabled => AllowlistMatch::Denied,
        ChannelDmPolicy::Open => AllowlistMatch::Wildcard,
        ChannelDmPolicy::Pairing => {
            // Pairing mode: allows all candidates but indicates pairing handshake is needed if unverified
            AllowlistMatch::Wildcard
        }
        ChannelDmPolicy::Allowlist => {
            if allowlist.is_empty() {
                return AllowlistMatch::NotMatched;
            }

            let normalized = normalize_sender_id(sender_id);
            for entry in allowlist {
                let entry_trimmed = entry.trim();
                if entry_trimmed == "*" {
                    return AllowlistMatch::Wildcard;
                }
                if normalize_sender_id(entry_trimmed) == normalized {
                    return AllowlistMatch::Exact(entry_trimmed.to_string());
                }
            }

            AllowlistMatch::NotMatched
        }
    }
}

/// Evaluates inbound group message admission against configured group policy and mention context.
pub fn evaluate_group_policy(
    policy: ChannelGroupPolicy,
    sender_id: &str,
    is_bot_mentioned: bool,
    allowlist: &[String],
) -> AllowlistMatch {
    match policy {
        ChannelGroupPolicy::Disabled => AllowlistMatch::Denied,
        ChannelGroupPolicy::Open => AllowlistMatch::Wildcard,
        ChannelGroupPolicy::RequireMention => {
            if is_bot_mentioned {
                AllowlistMatch::Wildcard
            } else {
                AllowlistMatch::NotMatched
            }
        }
        ChannelGroupPolicy::Allowlist => {
            if allowlist.is_empty() {
                return AllowlistMatch::NotMatched;
            }

            let normalized = normalize_sender_id(sender_id);
            for entry in allowlist {
                let entry_trimmed = entry.trim();
                if entry_trimmed == "*" {
                    return AllowlistMatch::Wildcard;
                }
                if normalize_sender_id(entry_trimmed) == normalized {
                    return AllowlistMatch::Exact(entry_trimmed.to_string());
                }
            }

            AllowlistMatch::NotMatched
        }
    }
}

/// Composite channel access gate enforcing both DM and Group policies.
#[derive(Debug, Clone, Default)]
pub struct ChannelPolicyGate {
    pub dm_policy: ChannelDmPolicy,
    pub group_policy: ChannelGroupPolicy,
    pub dm_allowlist: Vec<String>,
    pub group_allowlist: Vec<String>,
}

impl ChannelPolicyGate {
    pub fn new(dm_policy: ChannelDmPolicy, group_policy: ChannelGroupPolicy) -> Self {
        Self {
            dm_policy,
            group_policy,
            dm_allowlist: Vec::new(),
            group_allowlist: Vec::new(),
        }
    }

    pub fn with_dm_allowlist(mut self, allowlist: Vec<String>) -> Self {
        self.dm_allowlist = allowlist;
        self
    }

    pub fn with_group_allowlist(mut self, allowlist: Vec<String>) -> Self {
        self.group_allowlist = allowlist;
        self
    }

    pub fn check_admission(
        &self,
        sender_id: &str,
        is_dm: bool,
        is_bot_mentioned: bool,
    ) -> AllowlistMatch {
        if is_dm {
            evaluate_dm_policy(self.dm_policy, sender_id, &self.dm_allowlist)
        } else {
            evaluate_group_policy(self.group_policy, sender_id, is_bot_mentioned, &self.group_allowlist)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sender_normalization() {
        assert_eq!(normalize_sender_id("@alice"), "alice");
        assert_eq!(normalize_sender_id("user:Bob"), "bob");
        assert_eq!(normalize_sender_id("+1 (555) 019-2834"), "+15550192834");
        assert_eq!(normalize_sender_id("  charlie  "), "charlie");
    }

    #[test]
    fn test_dm_policy_evaluation() {
        let allowlist = vec!["alice".to_string(), "+15551234567".to_string()];

        assert_eq!(
            evaluate_dm_policy(ChannelDmPolicy::Allowlist, "@alice", &allowlist),
            AllowlistMatch::Exact("alice".to_string())
        );

        assert_eq!(
            evaluate_dm_policy(ChannelDmPolicy::Allowlist, "+1 (555) 123-4567", &allowlist),
            AllowlistMatch::Exact("+15551234567".to_string())
        );

        assert_eq!(
            evaluate_dm_policy(ChannelDmPolicy::Allowlist, "mallory", &allowlist),
            AllowlistMatch::NotMatched
        );

        assert_eq!(
            evaluate_dm_policy(ChannelDmPolicy::Disabled, "alice", &allowlist),
            AllowlistMatch::Denied
        );

        assert_eq!(
            evaluate_dm_policy(ChannelDmPolicy::Open, "mallory", &allowlist),
            AllowlistMatch::Wildcard
        );
    }

    #[test]
    fn test_group_mention_policy() {
        assert_eq!(
            evaluate_group_policy(ChannelGroupPolicy::RequireMention, "user1", true, &[]),
            AllowlistMatch::Wildcard
        );
        assert_eq!(
            evaluate_group_policy(ChannelGroupPolicy::RequireMention, "user1", false, &[]),
            AllowlistMatch::NotMatched
        );
    }
}
