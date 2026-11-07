//! Headless Main Subsystem domain controllers and multi-account runtime
//! Upstream: Telegram/SourceFiles/main/
//! Subsystem: main | Feature ID: TCG-N46, TCG-N02

use serde::{Deserialize, Serialize};

/// Maximum accounts supported per installation.
pub const CLIENT_MAX_ACCOUNTS: usize = 3;
pub const CLIENT_PREMIUM_MAX_ACCOUNTS: usize = 6;

/// Headless authenticated user session model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientSession {
    pub user_id: u64,
    pub is_premium: bool,
    pub test_mode: bool,
    pub freeze_since: Option<i64>,
    pub appeal_url: Option<String>,
    pub tmp_password: Option<(Vec<u8>, i64)>,
}

impl ClientSession {
    pub fn new(user_id: u64, test_mode: bool) -> Self {
        Self {
            user_id,
            is_premium: false,
            test_mode,
            freeze_since: None,
            appeal_url: None,
            tmp_password: None,
        }
    }

    #[inline]
    pub fn is_frozen(&self) -> bool {
        self.freeze_since.is_some()
    }
}

/// Headless Telegram account model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientAccount {
    pub index: usize,
    pub data_name: String,
    pub main_dc_id: i32,
    pub session: Option<ClientSession>,
}

impl ClientAccount {
    pub fn new(index: usize, data_name: String) -> Self {
        Self {
            index,
            data_name,
            main_dc_id: 2,
            session: None,
        }
    }

    #[inline]
    pub fn is_authenticated(&self) -> bool {
        self.session.is_some()
    }

    pub fn authenticate(&mut self, user_id: u64, test_mode: bool) {
        self.session = Some(ClientSession::new(user_id, test_mode));
    }

    pub fn logout(&mut self) {
        self.session = None;
    }
}

/// Headless multi-account domain coordinator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientDomain {
    pub accounts: Vec<ClientAccount>,
    pub active_index: Option<usize>,
    pub passcode_hash: Option<Vec<u8>>,
    pub is_locked: bool,
}

impl Default for ClientDomain {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientDomain {
    pub fn new() -> Self {
        Self {
            accounts: Vec::new(),
            active_index: None,
            passcode_hash: None,
            is_locked: false,
        }
    }

    pub fn add_account(&mut self, is_premium: bool) -> Result<usize, &'static str> {
        let max = if is_premium {
            CLIENT_PREMIUM_MAX_ACCOUNTS
        } else {
            CLIENT_MAX_ACCOUNTS
        };
        if self.accounts.len() >= max {
            return Err("Max accounts reached");
        }
        let index = self.accounts.len();
        self.accounts.push(ClientAccount::new(index, format!("account_{}", index)));
        if self.active_index.is_none() {
            self.active_index = Some(index);
        }
        Ok(index)
    }

    pub fn activate(&mut self, index: usize) -> bool {
        if index < self.accounts.len() {
            self.active_index = Some(index);
            true
        } else {
            false
        }
    }

    pub fn active_account(&self) -> Option<&ClientAccount> {
        self.active_index.and_then(|idx| self.accounts.get(idx))
    }

    pub fn active_account_mut(&mut self) -> Option<&mut ClientAccount> {
        if let Some(idx) = self.active_index {
            self.accounts.get_mut(idx)
        } else {
            None
        }
    }
}
