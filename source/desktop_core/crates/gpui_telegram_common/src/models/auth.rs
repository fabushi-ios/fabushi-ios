//! Shared Authentication and OAuth Models for GPUI Telegram (Milestone 2)

use serde::{Deserialize, Serialize};

/// Standardized User Data Transfer Object across auth and profile responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserDto {
    pub user_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    pub first_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub photo_url: Option<String>,
}

/// Response returned by POST /api/v1/auth/alipay/start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlipayStartResponse {
    pub attempt_id: String,
    #[serde(alias = "auth_url")]
    pub login_url: String,
    pub expires_at_ms: i64,
}

/// Response returned by GET /api/v1/auth/alipay/poll/{attempt_id}.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlipayPollResponse {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<UserDto>,
}

impl AlipayPollResponse {
    pub fn is_completed(&self) -> bool {
        self.status == "complete" || self.status == "completed"
    }

    pub fn is_pending(&self) -> bool {
        self.status == "pending"
    }

    pub fn is_expired(&self) -> bool {
        self.status == "expired"
    }
}

/// Request sent to POST /api/v1/auth/alipay/simulate-scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlipaySimulateScanRequest {
    pub attempt_id: String,
}

/// Request sent to POST /api/v1/auth/email/send-code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailSendCodeRequest {
    pub email: String,
}

fn default_cooldown_seconds() -> u32 {
    60
}

fn default_expires_in_seconds() -> u32 {
    600
}

/// Response returned by POST /api/v1/auth/email/send-code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailSendCodeResponse {
    #[serde(default, alias = "ok")]
    pub sent: bool,
    #[serde(default = "default_cooldown_seconds")]
    pub cooldown_seconds: u32,
    #[serde(default = "default_expires_in_seconds")]
    pub expires_in_seconds: u32,
}

/// Request sent to POST /api/v1/auth/email/register.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailRegisterRequest {
    pub email: String,
    pub code: String,
    pub username: String,
    pub password: String,
    pub first_name: String,
}
