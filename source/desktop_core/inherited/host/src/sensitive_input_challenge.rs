use std::collections::HashMap;
use std::sync::Mutex;

use uuid::Uuid;

pub const SENSITIVE_INPUT_CHALLENGE_MAX_TTL_MS: i64 = 120_000;
pub const SENSITIVE_INPUT_MAX_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SensitiveInputBinding {
    pub account_id: String,
    pub device_id: String,
    pub client_id: String,
    pub session_id: String,
    pub operation_id: String,
    pub request_sha256: String,
    pub network_generation: i64,
    pub authorization_generation: i64,
}

impl SensitiveInputBinding {
    pub fn validate(&self) -> Result<(), SensitiveInputChallengeError> {
        if self.account_id.trim().is_empty()
            || self.device_id.trim().is_empty()
            || self.client_id.trim().is_empty()
            || self.session_id.trim().is_empty()
            || self.operation_id.trim().is_empty()
            || self.request_sha256.len() != 64
            || !self.request_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            || self.network_generation <= 0
            || self.authorization_generation <= 0
        {
            return Err(SensitiveInputChallengeError::InvalidBinding);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensitiveInputChallenge {
    pub challenge_id: String,
    pub binding: SensitiveInputBinding,
    pub expires_at_ms: i64,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SensitiveInputChallengeError {
    #[error("sensitive-input challenge binding is invalid")]
    InvalidBinding,
    #[error("sensitive-input challenge ttl is invalid")]
    InvalidTtl,
    #[error("sensitive-input challenge is unknown or already consumed")]
    UnknownChallenge,
    #[error("sensitive-input challenge binding does not match")]
    BindingMismatch,
    #[error("sensitive-input challenge expired")]
    Expired,
    #[error("sensitive-input payload is empty or too large")]
    InvalidPayload,
    #[error("sensitive-input challenge store is unavailable")]
    StoreUnavailable,
}

#[derive(Default)]
pub struct SensitiveInputChallengeOwner {
    active: Mutex<HashMap<String, SensitiveInputChallenge>>,
}

struct SensitiveInputBytes(Vec<u8>);

impl SensitiveInputBytes {
    fn new(value: Vec<u8>) -> Self {
        Self(value)
    }

}

impl Drop for SensitiveInputBytes {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

impl SensitiveInputChallengeOwner {
    pub fn issue(
        &self,
        binding: SensitiveInputBinding,
        now_ms: i64,
        ttl_ms: i64,
    ) -> Result<SensitiveInputChallenge, SensitiveInputChallengeError> {
        binding.validate()?;
        if ttl_ms <= 0 || ttl_ms > SENSITIVE_INPUT_CHALLENGE_MAX_TTL_MS {
            return Err(SensitiveInputChallengeError::InvalidTtl);
        }
        let expires_at_ms = now_ms
            .checked_add(ttl_ms)
            .ok_or(SensitiveInputChallengeError::InvalidTtl)?;
        let challenge = SensitiveInputChallenge {
            challenge_id: Uuid::new_v4().to_string(),
            binding,
            expires_at_ms,
        };
        let mut active = self
            .active
            .lock()
            .map_err(|_| SensitiveInputChallengeError::StoreUnavailable)?;
        active.retain(|_, existing| existing.expires_at_ms > now_ms);
        active.insert(challenge.challenge_id.clone(), challenge.clone());
        Ok(challenge)
    }

    pub fn consume<R>(
        &self,
        challenge_id: &str,
        binding: &SensitiveInputBinding,
        now_ms: i64,
        payload: Vec<u8>,
        deliver: impl FnOnce(&mut Vec<u8>) -> R,
    ) -> Result<R, SensitiveInputChallengeError> {
        binding.validate()?;
        if payload.is_empty() || payload.len() > SENSITIVE_INPUT_MAX_BYTES {
            return Err(SensitiveInputChallengeError::InvalidPayload);
        }

        let challenge = {
            let mut active = self
                .active
                .lock()
                .map_err(|_| SensitiveInputChallengeError::StoreUnavailable)?;
            active
                .remove(challenge_id)
                .ok_or(SensitiveInputChallengeError::UnknownChallenge)?
        };

        if challenge.binding != *binding {
            return Err(SensitiveInputChallengeError::BindingMismatch);
        }
        if challenge.expires_at_ms <= now_ms {
            return Err(SensitiveInputChallengeError::Expired);
        }

        let mut payload = SensitiveInputBytes::new(payload);
        Ok(deliver(&mut payload.0))
    }

    pub fn revoke_authorization(
        &self,
        account_id: &str,
        device_id: &str,
        client_id: &str,
        session_id: &str,
        authorization_generation: i64,
    ) -> Result<usize, SensitiveInputChallengeError> {
        let mut active = self
            .active
            .lock()
            .map_err(|_| SensitiveInputChallengeError::StoreUnavailable)?;
        let before = active.len();
        active.retain(|_, challenge| {
            let binding = &challenge.binding;
            !(binding.account_id == account_id
                && binding.device_id == device_id
                && binding.client_id == client_id
                && binding.session_id == session_id
                && binding.authorization_generation == authorization_generation)
        });
        Ok(before.saturating_sub(active.len()))
    }

    pub fn revoke_session(
        &self,
        account_id: &str,
        device_id: &str,
        client_id: &str,
        session_id: &str,
    ) -> Result<usize, SensitiveInputChallengeError> {
        let mut active = self
            .active
            .lock()
            .map_err(|_| SensitiveInputChallengeError::StoreUnavailable)?;
        let before = active.len();
        active.retain(|_, challenge| {
            let binding = &challenge.binding;
            !(binding.account_id == account_id
                && binding.device_id == device_id
                && binding.client_id == client_id
                && binding.session_id == session_id)
        });
        Ok(before.saturating_sub(active.len()))
    }

    pub fn active_count_for_tests(&self) -> Result<usize, SensitiveInputChallengeError> {
        self.active
            .lock()
            .map(|active| active.len())
            .map_err(|_| SensitiveInputChallengeError::StoreUnavailable)
    }
}
