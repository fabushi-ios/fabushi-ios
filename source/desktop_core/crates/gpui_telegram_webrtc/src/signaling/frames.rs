use gpui_telegram_core::types::UserId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignalingFrame {
    Invite {
        call_id: String,
        from_user: UserId,
        to_user: UserId,
        is_video: bool,
    },
    Ringing {
        call_id: String,
    },
    Accept {
        call_id: String,
    },
    Offer {
        call_id: String,
        sdp: String,
    },
    Answer {
        call_id: String,
        sdp: String,
    },
    IceCandidate {
        call_id: String,
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    },
    Hangup {
        call_id: String,
        reason: String,
    },
}
