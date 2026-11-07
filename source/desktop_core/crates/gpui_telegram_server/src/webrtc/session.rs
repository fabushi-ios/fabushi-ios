use crate::webrtc::fsm::validate_transition;
use chrono::{DateTime, Utc};
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_core::types::UserId;
use gpui_telegram_webrtc::signaling::{CallSession, CallState};
use serde::{Deserialize, Serialize};

/// Extended server-side WebRTC call session tracking lifecycle timestamps and SDP negotiations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerCallSession {
    pub inner: CallSession,
    pub started_at: DateTime<Utc>,
    pub connected_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub end_reason: Option<String>,
    pub sdp_offer: Option<String>,
    pub sdp_answer: Option<String>,
}

impl ServerCallSession {
    /// Creates a new call session starting in `CallState::Dialing`.
    pub fn new(call_id: String, caller: UserId, callee: UserId, is_video: bool) -> Self {
        Self {
            inner: CallSession::new(call_id, caller, callee, is_video),
            started_at: Utc::now(),
            connected_at: None,
            ended_at: None,
            end_reason: None,
            sdp_offer: None,
            sdp_answer: None,
        }
    }

    pub fn call_id(&self) -> &str {
        &self.inner.call_id
    }

    pub fn caller(&self) -> UserId {
        self.inner.caller
    }

    pub fn callee(&self) -> UserId {
        self.inner.callee
    }

    pub fn is_video(&self) -> bool {
        self.inner.is_video
    }

    pub fn state(&self) -> CallState {
        self.inner.state
    }

    /// Advances the FSM state, validating against illegal transitions.
    pub fn transition_to(&mut self, next_state: CallState) -> Result<()> {
        let valid = validate_transition(self.inner.state, next_state)?;
        self.inner.state = valid;

        if valid == CallState::Connected && self.connected_at.is_none() {
            self.connected_at = Some(Utc::now());
        } else if (valid == CallState::Ended || valid == CallState::Failed) && self.ended_at.is_none() {
            self.ended_at = Some(Utc::now());
        }

        Ok(())
    }

    /// Checks if a user is either the caller or callee in this session.
    pub fn is_participant(&self, user_id: UserId) -> bool {
        self.inner.caller == user_id || self.inner.callee == user_id
    }

    /// Returns the counterpart peer of the given participant.
    pub fn peer_of(&self, user_id: UserId) -> Result<UserId> {
        if self.inner.caller == user_id {
            Ok(self.inner.callee)
        } else if self.inner.callee == user_id {
            Ok(self.inner.caller)
        } else {
            Err(TelegramError::PermissionDenied(format!(
                "User {} is not a participant in call {}",
                user_id, self.inner.call_id
            )))
        }
    }

    /// Duration of call in seconds if connected.
    pub fn duration_seconds(&self) -> Option<i64> {
        if let Some(conn) = self.connected_at {
            let end = self.ended_at.unwrap_or_else(Utc::now);
            Some((end - conn).num_seconds().max(0))
        } else {
            None
        }
    }
}
