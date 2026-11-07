//! Target Symbol: DataUserNames
//! Telegram Desktop 1:1 Submodule: `DataUserNames`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

/// Record for a single username associated with a peer or collectible entity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct Username {
    pub username: String,
    pub active: bool,
    pub editable: bool,
}

impl Username {
    pub fn new(username: impl Into<String>, active: bool, editable: bool) -> Self {
        Self {
            username: username.into(),
            active,
            editable,
        }
    }
}

/// Collection of usernames with active and editable primary resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UsernamesInfo {
    usernames: Vec<String>,
    index_editable: Option<usize>,
}

impl UsernamesInfo {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_single(&mut self, username: &str) {
        if username.is_empty() {
            self.usernames.clear();
            self.index_editable = None;
        } else {
            self.usernames = vec![username.to_string()];
            self.index_editable = Some(0);
        }
    }

    pub fn set_usernames(&mut self, list: &[Username]) {
        self.usernames.clear();
        self.index_editable = None;
        for (i, entry) in list.iter().enumerate() {
            if entry.active {
                self.usernames.push(entry.username.clone());
                if entry.editable && self.index_editable.is_none() {
                    self.index_editable = Some(i);
                }
            }
        }
    }

    pub fn primary_username(&self) -> Option<&str> {
        self.usernames.first().map(|s| s.as_str())
    }

    pub fn editable_username(&self) -> Option<&str> {
        self.index_editable.and_then(|idx| self.usernames.get(idx)).map(|s| s.as_str())
    }

    pub fn all_usernames(&self) -> &[String] {
        &self.usernames
    }

    pub fn is_editable(&self, username: &str) -> bool {
        self.editable_username().map_or(false, |u| u.eq_ignore_ascii_case(username))
    }
}

/// Target Symbol for Module 03 Ledger: DataUserNames coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataUserNames {
    pub info: UsernamesInfo,
}

impl DataUserNames {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_username(username: &str) -> Self {
        let mut info = UsernamesInfo::new();
        info.set_single(username);
        Self { info }
    }

    pub fn username(&self) -> Option<&str> {
        self.info.primary_username()
    }
}
