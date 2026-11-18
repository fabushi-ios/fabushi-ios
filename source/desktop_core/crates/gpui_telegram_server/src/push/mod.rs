pub mod dispatchers;
pub mod gateway_hub;

pub use gateway_hub::{PushDispatcher, PushGatewayHub};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PushPlatform {
    Apns,
    Webpush,
    Fcm,
    Mock,
}

impl PushPlatform {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Apns => "apns",
            Self::Webpush => "webpush",
            Self::Fcm => "fcm",
            Self::Mock => "mock",
        }
    }

    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "apns" | "apple" | "ios" | "macos" => Some(Self::Apns),
            "webpush" | "web" => Some(Self::Webpush),
            "fcm" | "android" | "firebase" => Some(Self::Fcm),
            "mock" | "test" => Some(Self::Mock),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushNotification {
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub chat_id: Option<i64>,
    #[serde(default)]
    pub message_id: Option<i64>,
    #[serde(default)]
    pub badge: Option<u32>,
    #[serde(default)]
    pub sound: Option<String>,
    #[serde(default)]
    pub custom_data: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushDeliveryReceipt {
    pub token: String,
    pub platform: PushPlatform,
    pub delivered: bool,
    pub message_id: Option<String>,
    pub delivered_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PushError {
    #[error("Invalid token: {0}")]
    InvalidToken(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("Payload too large")]
    PayloadTooLarge,

    #[error("Device token unregistered / expired: {0}")]
    Unregistered(String),

    #[error("Dispatcher not found for platform: {0:?}")]
    NoDispatcher(PushPlatform),

    #[error("Platform dispatch error: {0}")]
    PlatformError(String),
}
