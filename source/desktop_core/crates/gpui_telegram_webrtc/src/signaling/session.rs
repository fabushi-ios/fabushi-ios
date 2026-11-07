use gpui_telegram_core::types::UserId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallState {
    Idle,
    Dialing,
    Ringing,
    Connecting,
    Connected,
    Ended,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallSession {
    pub call_id: String,
    pub caller: UserId,
    pub callee: UserId,
    pub is_video: bool,
    pub state: CallState,
}

impl CallSession {
    pub fn new(call_id: String, caller: UserId, callee: UserId, is_video: bool) -> Self {
        Self {
            call_id,
            caller,
            callee,
            is_video,
            state: CallState::Dialing,
        }
    }
}
