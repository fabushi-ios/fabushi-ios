//! OpenClaw Plugin SDK Access Groups & Account Resolution
//!
//! Mirrors:
//! - `src/plugin-sdk/access-groups.ts`
//! - `src/plugin-sdk/access-groups.test.ts`
//! - `src/plugin-sdk/account-core.ts`
//! - `src/plugin-sdk/account-id.ts`
//! - `src/plugin-sdk/account-helpers.ts`
//! - `src/plugin-sdk/account-resolution.ts`
//! - `src/plugin-sdk/account-resolution-runtime.ts`
//! - `src/plugin-sdk/account-configured-ids.ts`
//! - `src/plugin-sdk/AGENTS.md`

use std::collections::HashSet;
use serde::{Deserialize, Serialize};

pub const ACCESS_GROUP_ALLOW_FROM_PREFIX: &str = "accessGroup:";

pub fn parse_access_group_allow_from_entry(entry: &str) -> Option<&str> {
    let trimmed = entry.trim();
    if let Some(name) = trimmed.strip_prefix(ACCESS_GROUP_ALLOW_FROM_PREFIX) {
        let name_trimmed = name.trim();
        if !name_trimmed.is_empty() {
            return Some(name_trimmed);
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessGroup {
    pub name: String,
    pub allowed_senders: HashSet<String>,
    pub allow_all: bool,
}

impl AccessGroup {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            allowed_senders: HashSet::new(),
            allow_all: false,
        }
    }

    pub fn with_sender(mut self, sender: impl Into<String>) -> Self {
        self.allowed_senders.insert(sender.into().to_ascii_lowercase());
        self
    }

    pub fn with_allow_all(mut self, allow_all: bool) -> Self {
        self.allow_all = allow_all;
        self
    }

    pub fn is_sender_allowed(&self, sender_id: &str) -> bool {
        if self.allow_all {
            return true;
        }
        let lower = sender_id.trim().to_ascii_lowercase();
        self.allowed_senders.contains(&lower) || self.allowed_senders.contains("*")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccountIdentifier {
    pub provider: String,
    pub account_id: String,
}

impl AccountIdentifier {
    pub fn new(provider: impl Into<String>, account_id: impl Into<String>) -> Self {
        Self {
            provider: provider.into().trim().to_ascii_lowercase(),
            account_id: account_id.into().trim().to_string(),
        }
    }

    pub fn canonical_key(&self) -> String {
        format!("{}:{}", self.provider, self.account_id)
    }

    pub fn parse_canonical(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.splitn(2, ':').collect();
        if parts.len() == 2 && !parts[0].trim().is_empty() && !parts[1].trim().is_empty() {
            Some(Self::new(parts[0], parts[1]))
        } else {
            None
        }
    }
}
