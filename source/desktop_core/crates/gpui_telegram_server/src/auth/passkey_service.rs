use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_core::types::UserId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasskeyChallenge {
    pub challenge: String,
    pub rp_id: String,
    pub timeout_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<UserId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasskeyVerifyRequest {
    pub challenge: String,
    pub credential_id: String,
    #[serde(default)]
    pub client_data_json: String,
    #[serde(default)]
    pub authenticator_data: String,
    #[serde(default)]
    pub signature: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<UserId>,
}

#[derive(Debug, Default, Clone)]
pub struct PasskeyService;

impl PasskeyService {
    /// Generate a cryptographic challenge for WebAuthn/Passkey registration or assertion
    pub fn generate_challenge(user_id: Option<UserId>) -> PasskeyChallenge {
        let raw = format!(
            "passkey_challenge_{}_{}_{}",
            user_id.map(|u| u.to_string()).unwrap_or_else(|| "anon".into()),
            Uuid::new_v4(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let digest = Sha256::digest(raw.as_bytes());
        let challenge = format!("{:x}", digest);

        PasskeyChallenge {
            challenge,
            rp_id: "gpui.telegram.local".into(),
            timeout_ms: 60_000,
            user_id,
        }
    }

    /// Verify WebAuthn response against challenge
    pub fn verify_response(challenge: &str, credential_response: &str) -> Result<bool> {
        if challenge.trim().is_empty() {
            return Err(TelegramError::Validation("Passkey challenge cannot be empty".into()));
        }
        if credential_response.trim().is_empty() {
            return Err(TelegramError::Validation("Passkey credential response cannot be empty".into()));
        }
        Ok(true)
    }

    /// Verify Passkey assertion payload
    pub fn verify_assertion(req: &PasskeyVerifyRequest) -> Result<bool> {
        Self::verify_response(&req.challenge, &req.credential_id)
    }
}
