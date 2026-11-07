use crate::protocol::Failure;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum CallState {
    Dialing,
    Ringing,
    Active { started_at_ms: i64 },
    Ended { duration_ms: i64, reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OneOnOneCall {
    pub call_id: String,
    pub caller_id: String,
    pub callee_id: String,
    pub state: CallState,
    pub is_video: bool,
    pub signaling_messages: Vec<String>,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroupCallParticipant {
    pub participant_id: String,
    pub joined_at_ms: i64,
    pub is_muted: bool,
    pub is_hand_raised: bool,
    pub is_presenting_screen: bool,
    pub ssrc: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroupCall {
    pub group_call_id: String,
    pub chat_id: String,
    pub title: String,
    pub is_active: bool,
    pub participants: BTreeMap<String, GroupCallParticipant>,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, Default)]
pub struct CallsGroupCallsState {
    pub direct_calls: BTreeMap<String, OneOnOneCall>,
    pub group_calls: BTreeMap<String, GroupCall>,
    next_call_id: u32,
    next_group_call_id: u32,
}

impl CallsGroupCallsState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_call(
        &mut self,
        caller_id: String,
        callee_id: String,
        is_video: bool,
        now_ms: i64,
    ) -> Result<OneOnOneCall, Failure> {
        if caller_id == callee_id {
            return Err(Failure::new("CANNOT_CALL_SELF", "cannot initiate call to self"));
        }
        self.next_call_id += 1;
        let call_id = format!("call_{}", self.next_call_id);
        let call = OneOnOneCall {
            call_id: call_id.clone(),
            caller_id,
            callee_id,
            state: CallState::Dialing,
            is_video,
            signaling_messages: Vec::new(),
            created_at_ms: now_ms,
        };
        self.direct_calls.insert(call_id, call.clone());
        Ok(call)
    }

    pub fn accept_call(
        &mut self,
        call_id: &str,
        callee_id: &str,
        now_ms: i64,
    ) -> Result<OneOnOneCall, Failure> {
        let call = self
            .direct_calls
            .get_mut(call_id)
            .ok_or_else(|| Failure::new("CALL_NOT_FOUND", format!("call {call_id} not found")))?;

        if call.callee_id != callee_id {
            return Err(Failure::new(
                "UNAUTHORIZED_ACCEPT",
                "only callee can accept the call",
            ));
        }

        match &call.state {
            CallState::Dialing | CallState::Ringing => {
                call.state = CallState::Active { started_at_ms: now_ms };
                Ok(call.clone())
            }
            _ => Err(Failure::new(
                "INVALID_CALL_STATE",
                "call is not in a ringing/dialing state",
            )),
        }
    }

    pub fn end_call(
        &mut self,
        call_id: &str,
        reason: String,
        now_ms: i64,
    ) -> Result<OneOnOneCall, Failure> {
        let call = self
            .direct_calls
            .get_mut(call_id)
            .ok_or_else(|| Failure::new("CALL_NOT_FOUND", format!("call {call_id} not found")))?;

        let duration_ms = match &call.state {
            CallState::Active { started_at_ms } => now_ms.saturating_sub(*started_at_ms),
            _ => 0,
        };
        call.state = CallState::Ended {
            duration_ms,
            reason,
        };
        Ok(call.clone())
    }

    pub fn send_signaling(&mut self, call_id: &str, payload: String) -> Result<bool, Failure> {
        let call = self
            .direct_calls
            .get_mut(call_id)
            .ok_or_else(|| Failure::new("CALL_NOT_FOUND", format!("call {call_id} not found")))?;

        if matches!(call.state, CallState::Ended { .. }) {
            return Err(Failure::new("CALL_ALREADY_ENDED", "call has already ended"));
        }
        call.signaling_messages.push(payload);
        Ok(true)
    }

    pub fn create_group_call(
        &mut self,
        chat_id: String,
        title: String,
        now_ms: i64,
    ) -> Result<GroupCall, Failure> {
        self.next_group_call_id += 1;
        let group_call_id = format!("group_call_{}", self.next_group_call_id);
        let group_call = GroupCall {
            group_call_id: group_call_id.clone(),
            chat_id,
            title,
            is_active: true,
            participants: BTreeMap::new(),
            created_at_ms: now_ms,
        };
        self.group_calls.insert(group_call_id, group_call.clone());
        Ok(group_call)
    }

    pub fn join_group_call(
        &mut self,
        group_call_id: &str,
        participant_id: String,
        ssrc: u32,
        now_ms: i64,
    ) -> Result<GroupCallParticipant, Failure> {
        let call = self.group_calls.get_mut(group_call_id).ok_or_else(|| {
            Failure::new("GROUP_CALL_NOT_FOUND", format!("group call {group_call_id} not found"))
        })?;

        if !call.is_active {
            return Err(Failure::new(
                "GROUP_CALL_INACTIVE",
                "group call is inactive",
            ));
        }

        let participant = GroupCallParticipant {
            participant_id: participant_id.clone(),
            joined_at_ms: now_ms,
            is_muted: true,
            is_hand_raised: false,
            is_presenting_screen: false,
            ssrc,
        };
        call.participants
            .insert(participant_id, participant.clone());
        Ok(participant)
    }

    pub fn leave_group_call(
        &mut self,
        group_call_id: &str,
        participant_id: &str,
    ) -> Result<bool, Failure> {
        let call = self.group_calls.get_mut(group_call_id).ok_or_else(|| {
            Failure::new("GROUP_CALL_NOT_FOUND", format!("group call {group_call_id} not found"))
        })?;

        if call.participants.remove(participant_id).is_none() {
            return Err(Failure::new(
                "PARTICIPANT_NOT_FOUND",
                format!("participant {participant_id} not in group call"),
            ));
        }
        Ok(true)
    }

    pub fn toggle_mute_group_call(
        &mut self,
        group_call_id: &str,
        participant_id: &str,
        muted: bool,
    ) -> Result<GroupCallParticipant, Failure> {
        let call = self.group_calls.get_mut(group_call_id).ok_or_else(|| {
            Failure::new("GROUP_CALL_NOT_FOUND", format!("group call {group_call_id} not found"))
        })?;

        let participant = call
            .participants
            .get_mut(participant_id)
            .ok_or_else(|| Failure::new("PARTICIPANT_NOT_FOUND", "participant not in call"))?;

        participant.is_muted = muted;
        Ok(participant.clone())
    }

    pub fn raise_hand_group_call(
        &mut self,
        group_call_id: &str,
        participant_id: &str,
        raised: bool,
    ) -> Result<GroupCallParticipant, Failure> {
        let call = self.group_calls.get_mut(group_call_id).ok_or_else(|| {
            Failure::new("GROUP_CALL_NOT_FOUND", format!("group call {group_call_id} not found"))
        })?;

        let participant = call
            .participants
            .get_mut(participant_id)
            .ok_or_else(|| Failure::new("PARTICIPANT_NOT_FOUND", "participant not in call"))?;

        participant.is_hand_raised = raised;
        Ok(participant.clone())
    }

    pub fn toggle_screen_share_group_call(
        &mut self,
        group_call_id: &str,
        participant_id: &str,
        sharing: bool,
    ) -> Result<GroupCallParticipant, Failure> {
        let call = self.group_calls.get_mut(group_call_id).ok_or_else(|| {
            Failure::new("GROUP_CALL_NOT_FOUND", format!("group call {group_call_id} not found"))
        })?;

        let participant = call
            .participants
            .get_mut(participant_id)
            .ok_or_else(|| Failure::new("PARTICIPANT_NOT_FOUND", "participant not in call"))?;

        participant.is_presenting_screen = sharing;
        Ok(participant.clone())
    }

    pub fn handle_route(&mut self, method: &str, args: &Value) -> Result<Value, Failure> {
        if let Some(op) = method.strip_prefix("calls.") {
            match op {
                "create" | "initiate" => {
                    let caller_id = args
                        .get("callerId")
                        .or_else(|| args.get("userId"))
                        .and_then(Value::as_str)
                        .unwrap_or("self_user")
                        .to_string();
                    let callee_id = args
                        .get("calleeId")
                        .or_else(|| args.get("targetId"))
                        .or_else(|| args.get("peerId"))
                        .or_else(|| args.get("chatId"))
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "calleeId required"))?
                        .to_string();
                    let is_video = args.get("isVideo").and_then(Value::as_bool).unwrap_or(false);
                    let now_ms = args
                        .get("nowMs")
                        .and_then(Value::as_i64)
                        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

                    let call = self.create_call(caller_id, callee_id, is_video, now_ms)?;
                    Ok(json!({ "call": call }))
                }
                "accept" => {
                    let call_id = args
                        .get("callId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "callId required"))?;
                    let callee_id = args
                        .get("calleeId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "calleeId required"))?;
                    let now_ms = args
                        .get("nowMs")
                        .and_then(Value::as_i64)
                        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

                    let call = self.accept_call(call_id, callee_id, now_ms)?;
                    Ok(json!({ "call": call }))
                }
                "end" | "discard" => {
                    let call_id = args
                        .get("callId")
                        .or_else(|| args.get("id"))
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "callId required"))?;
                    let reason = args
                        .get("reason")
                        .and_then(Value::as_str)
                        .unwrap_or("hangup")
                        .to_string();
                    let now_ms = args
                        .get("nowMs")
                        .and_then(Value::as_i64)
                        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

                    let call = self.end_call(call_id, reason, now_ms)?;
                    Ok(json!({ "call": call }))
                }
                "sendSignaling" => {
                    let call_id = args
                        .get("callId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "callId required"))?;
                    let payload = args
                        .get("payload")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "payload required"))?
                        .to_string();

                    let ok = self.send_signaling(call_id, payload)?;
                    Ok(json!({ "delivered": ok }))
                }
                other => Err(Failure::new(
                    "UNKNOWN_CALL_METHOD",
                    format!("unrecognized call method {other}"),
                )),
            }
        } else if let Some(op) = method.strip_prefix("groupCalls.") {
            match op {
                "create" | "initiate" => {
                    let chat_id = args
                        .get("chatId")
                        .or_else(|| args.get("id"))
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "chatId required"))?
                        .to_string();
                    let title = args
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("Live Video Chat")
                        .to_string();
                    let now_ms = args
                        .get("nowMs")
                        .and_then(Value::as_i64)
                        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

                    let call = self.create_group_call(chat_id, title, now_ms)?;
                    Ok(json!({ "groupCall": call }))
                }
                "join" => {
                    let group_call_id = args
                        .get("groupCallId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "groupCallId required"))?;
                    let participant_id = args
                        .get("participantId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "participantId required"))?
                        .to_string();
                    let ssrc = args
                        .get("ssrc")
                        .and_then(Value::as_u64)
                        .unwrap_or(1001) as u32;
                    let now_ms = args
                        .get("nowMs")
                        .and_then(Value::as_i64)
                        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

                    let p = self.join_group_call(group_call_id, participant_id, ssrc, now_ms)?;
                    Ok(json!({ "participant": p }))
                }
                "leave" => {
                    let group_call_id = args
                        .get("groupCallId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "groupCallId required"))?;
                    let participant_id = args
                        .get("participantId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "participantId required"))?;

                    let left = self.leave_group_call(group_call_id, participant_id)?;
                    Ok(json!({ "left": left }))
                }
                "toggleMute" => {
                    let group_call_id = args
                        .get("groupCallId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "groupCallId required"))?;
                    let participant_id = args
                        .get("participantId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "participantId required"))?;
                    let muted = args.get("muted").and_then(Value::as_bool).unwrap_or(true);

                    let p = self.toggle_mute_group_call(group_call_id, participant_id, muted)?;
                    Ok(json!({ "participant": p }))
                }
                "raiseHand" => {
                    let group_call_id = args
                        .get("groupCallId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "groupCallId required"))?;
                    let participant_id = args
                        .get("participantId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "participantId required"))?;
                    let raised = args.get("raised").and_then(Value::as_bool).unwrap_or(true);

                    let p = self.raise_hand_group_call(group_call_id, participant_id, raised)?;
                    Ok(json!({ "participant": p }))
                }
                "toggleScreenShare" => {
                    let group_call_id = args
                        .get("groupCallId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "groupCallId required"))?;
                    let participant_id = args
                        .get("participantId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "participantId required"))?;
                    let sharing = args.get("sharing").and_then(Value::as_bool).unwrap_or(true);

                    let p = self.toggle_screen_share_group_call(group_call_id, participant_id, sharing)?;
                    Ok(json!({ "participant": p }))
                }
                other => Err(Failure::new(
                    "UNKNOWN_GROUP_CALL_METHOD",
                    format!("unrecognized group call method {other}"),
                )),
            }
        } else {
            Err(Failure::new(
                "UNKNOWN_METHOD",
                format!("unrecognized method {method}"),
            ))
        }
    }
}
