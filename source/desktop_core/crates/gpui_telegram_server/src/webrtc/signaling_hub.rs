use crate::webrtc::session::ServerCallSession;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_core::types::{ChatId, UserId};
use gpui_telegram_webrtc::ice::TurnToken;
use gpui_telegram_webrtc::sfu::SfuBridge;
use gpui_telegram_webrtc::signaling::{CallState, SignalingFrame};
use parking_lot::RwLock;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

/// WebRTC Call Signaling Hub managing 1-on-1 calls, SFU conference rooms,
/// peer routing, and ephemeral STUN/TURN credential generation.
#[derive(Debug, Default, Clone)]
pub struct SignalingHub {
    sessions: Arc<RwLock<HashMap<String, ServerCallSession>>>,
    user_channels: Arc<RwLock<HashMap<UserId, mpsc::Sender<SignalingFrame>>>>,
    sfu_bridge: Arc<RwLock<SfuBridge>>,
    active_calls: Arc<RwLock<HashMap<String, Vec<SignalingFrame>>>>,
}

impl SignalingHub {
    /// Creates a new SignalingHub instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers an active client channel (e.g. from an active WebSocket).
    pub fn register_client(&self, user_id: UserId, tx: mpsc::Sender<SignalingFrame>) {
        self.user_channels.write().insert(user_id, tx);
    }

    /// Unregisters an active client channel upon disconnect.
    pub fn unregister_client(&self, user_id: UserId) {
        self.user_channels.write().remove(&user_id);
    }

    /// Returns whether a user is currently connected to the signaling gateway.
    pub fn is_client_online(&self, user_id: UserId) -> bool {
        self.user_channels.read().contains_key(&user_id)
    }

    /// Dispatches an incoming signaling frame, advances Call FSM, and relays to the counterpart peer.
    pub async fn dispatch_frame(&self, sender_id: UserId, frame: SignalingFrame) -> Result<()> {
        match frame {
            SignalingFrame::Invite {
                ref call_id,
                from_user,
                to_user,
                is_video,
            } => {
                if from_user != sender_id {
                    return Err(TelegramError::PermissionDenied("Sender ID mismatch on invite".into()));
                }
                if from_user == to_user {
                    return Err(TelegramError::Validation("Cannot call self".into()));
                }

                let session = ServerCallSession::new(call_id.clone(), from_user, to_user, is_video);
                self.sessions.write().insert(call_id.clone(), session);
                self.record_signal(call_id, frame.clone());

                self.relay_to_user(to_user, frame).await?;
            }
            SignalingFrame::Ringing { ref call_id } => {
                let caller = {
                    let mut sessions = self.sessions.write();
                    let session = sessions
                        .get_mut(call_id)
                        .ok_or_else(|| TelegramError::NotFound(format!("Call session {call_id} not found")))?;
                    if session.callee() != sender_id {
                        return Err(TelegramError::PermissionDenied("Only callee can acknowledge ringing".into()));
                    }
                    session.transition_to(CallState::Ringing)?;
                    session.caller()
                };
                self.record_signal(call_id, frame.clone());
                self.relay_to_user(caller, frame).await?;
            }
            SignalingFrame::Accept { ref call_id } => {
                let caller = {
                    let mut sessions = self.sessions.write();
                    let session = sessions
                        .get_mut(call_id)
                        .ok_or_else(|| TelegramError::NotFound(format!("Call session {call_id} not found")))?;
                    if session.callee() != sender_id {
                        return Err(TelegramError::PermissionDenied("Only callee can accept call".into()));
                    }
                    session.transition_to(CallState::Connecting)?;
                    session.caller()
                };
                self.record_signal(call_id, frame.clone());
                self.relay_to_user(caller, frame).await?;
            }
            SignalingFrame::Offer {
                ref call_id,
                ref sdp,
            } => {
                let peer = {
                    let mut sessions = self.sessions.write();
                    let session = sessions
                        .get_mut(call_id)
                        .ok_or_else(|| TelegramError::NotFound(format!("Call session {call_id} not found")))?;
                    session.sdp_offer = Some(sdp.clone());
                    session.peer_of(sender_id)?
                };
                self.record_signal(call_id, frame.clone());
                self.relay_to_user(peer, frame).await?;
            }
            SignalingFrame::Answer {
                ref call_id,
                ref sdp,
            } => {
                let peer = {
                    let mut sessions = self.sessions.write();
                    let session = sessions
                        .get_mut(call_id)
                        .ok_or_else(|| TelegramError::NotFound(format!("Call session {call_id} not found")))?;
                    session.sdp_answer = Some(sdp.clone());
                    session.transition_to(CallState::Connected)?;
                    session.peer_of(sender_id)?
                };
                self.record_signal(call_id, frame.clone());
                self.relay_to_user(peer, frame).await?;
            }
            SignalingFrame::IceCandidate { ref call_id, .. } => {
                let peer = {
                    let sessions = self.sessions.read();
                    let session = sessions
                        .get(call_id)
                        .ok_or_else(|| TelegramError::NotFound(format!("Call session {call_id} not found")))?;
                    session.peer_of(sender_id)?
                };
                self.record_signal(call_id, frame.clone());
                self.relay_to_user(peer, frame).await?;
            }
            SignalingFrame::Hangup {
                ref call_id,
                ref reason,
            } => {
                let peer = {
                    let mut sessions = self.sessions.write();
                    if let Some(session) = sessions.get_mut(call_id) {
                        let _ = session.transition_to(CallState::Ended);
                        session.end_reason = Some(reason.clone());
                        session.peer_of(sender_id).ok()
                    } else {
                        None
                    }
                };
                self.record_signal(call_id, frame.clone());
                if let Some(peer_id) = peer {
                    let _ = self.relay_to_user(peer_id, frame).await;
                }
            }
        }
        Ok(())
    }

    /// Records signaling history for a call ID (retains backward compatibility).
    pub fn record_signal(&self, call_id: &str, frame: SignalingFrame) {
        self.active_calls
            .write()
            .entry(call_id.to_string())
            .or_default()
            .push(frame);
    }

    /// Retrieves session snapshot if existing.
    pub fn get_session(&self, call_id: &str) -> Option<ServerCallSession> {
        self.sessions.read().get(call_id).cloned()
    }

    /// Retrieves current call FSM state.
    pub fn get_call_state(&self, call_id: &str) -> Option<CallState> {
        self.sessions.read().get(call_id).map(|s| s.state())
    }

    /// Adds a user to an SFU conference room.
    pub fn join_sfu_room(&self, chat_id: ChatId, user_id: UserId) {
        self.sfu_bridge.write().join_room(chat_id, user_id);
    }

    /// Removes a user from an SFU conference room.
    pub fn leave_sfu_room(&self, chat_id: ChatId, user_id: UserId) {
        self.sfu_bridge.write().leave_room(chat_id, user_id);
    }

    /// Returns the list of subscribers currently joined to an SFU conference room.
    pub fn get_sfu_subscribers(&self, chat_id: ChatId) -> Vec<UserId> {
        self.sfu_bridge.read().get_subscribers(chat_id).to_vec()
    }

    /// Multicasts a signaling frame to all SFU room members except the sender.
    /// Returns the number of successfully delivered frames.
    pub async fn broadcast_to_sfu_room(
        &self,
        chat_id: ChatId,
        sender_id: UserId,
        frame: SignalingFrame,
    ) -> usize {
        let subs = self.get_sfu_subscribers(chat_id);
        let mut delivered = 0;
        for user_id in subs {
            if user_id != sender_id {
                if self.relay_to_user(user_id, frame.clone()).await.is_ok() {
                    delivered += 1;
                }
            }
        }
        delivered
    }

    /// Relays a frame directly to a connected client channel.
    async fn relay_to_user(&self, target_user: UserId, frame: SignalingFrame) -> Result<()> {
        let tx = { self.user_channels.read().get(&target_user).cloned() };
        if let Some(sender) = tx {
            sender
                .send(frame)
                .await
                .map_err(|_| TelegramError::Internal("Client signaling channel disconnected".into()))?;
            Ok(())
        } else {
            Err(TelegramError::NotFound(format!("User {target_user} is offline")))
        }
    }
}

/// Computes HMAC-SHA256 according to RFC 2104.
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK_SIZE: usize = 64;
    let mut k = [0u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        let mut h = Sha256::new();
        h.update(key);
        let digest = h.finalize();
        k[..32].copy_from_slice(&digest);
    } else {
        k[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; BLOCK_SIZE];
    let mut opad = [0x5cu8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }

    let mut inner_hasher = Sha256::new();
    inner_hasher.update(&ipad);
    inner_hasher.update(message);
    let inner_digest = inner_hasher.finalize();

    let mut outer_hasher = Sha256::new();
    outer_hasher.update(&opad);
    outer_hasher.update(&inner_digest);
    outer_hasher.finalize().into()
}

/// Generates RFC 5766 REST API ephemeral STUN/TURN credentials for NAT traversal.
pub fn generate_turn_token(
    user_id: UserId,
    turn_secret: &str,
    turn_uris: Vec<String>,
    ttl_hours: i64,
) -> TurnToken {
    let expires_at: DateTime<Utc> = Utc::now() + Duration::hours(ttl_hours);
    let username = format!("{}:{}", expires_at.timestamp(), user_id.raw());
    let mac = hmac_sha256(turn_secret.as_bytes(), username.as_bytes());
    let credential = BASE64.encode(mac);

    TurnToken {
        username,
        credential,
        uris: turn_uris,
        expires_at,
    }
}
