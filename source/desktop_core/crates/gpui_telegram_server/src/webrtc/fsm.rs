use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_webrtc::signaling::CallState;

/// Validates whether a Call FSM state transition from `current` to `next` is allowed.
///
/// Supported state sequence:
/// `Idle` -> `Dialing` -> `Ringing` -> `Connecting` -> `Connected` -> `Ended`
///
/// Also handles terminal aborts / declines from intermediate states.
pub fn can_transition(current: CallState, next: CallState) -> bool {
    match (current, next) {
        // From Idle
        (CallState::Idle, CallState::Dialing) => true,

        // From Dialing
        (CallState::Dialing, CallState::Ringing) => true,
        (CallState::Dialing, CallState::Ended) => true, // Caller cancel or callee decline
        (CallState::Dialing, CallState::Failed) => true,

        // From Ringing
        (CallState::Ringing, CallState::Connecting) => true, // Callee accepts
        (CallState::Ringing, CallState::Ended) => true,      // Callee rejects or caller cancels
        (CallState::Ringing, CallState::Failed) => true,

        // From Connecting
        (CallState::Connecting, CallState::Connected) => true, // SDP answer & ICE connected
        (CallState::Connecting, CallState::Ended) => true,     // User hangs up during setup
        (CallState::Connecting, CallState::Failed) => true,

        // From Connected
        (CallState::Connected, CallState::Ended) => true, // Hangup
        (CallState::Connected, CallState::Failed) => true,

        // Same state idempotence
        (a, b) if a == b => true,

        // Terminal states cannot transition further
        (CallState::Ended, _) => false,
        (CallState::Failed, _) => false,

        // All other transitions are invalid
        _ => false,
    }
}

/// Validates transition and returns the next state or a Validation error.
pub fn validate_transition(current: CallState, next: CallState) -> Result<CallState> {
    if can_transition(current, next) {
        Ok(next)
    } else {
        Err(TelegramError::Validation(format!(
            "Invalid WebRTC call state transition from {:?} to {:?}",
            current, next
        )))
    }
}
