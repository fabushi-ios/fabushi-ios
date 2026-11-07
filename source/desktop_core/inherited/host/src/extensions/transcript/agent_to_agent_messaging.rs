use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{Value, json};

use crate::agents::agent_messaging::{
    AgentAddress, AgentMessageImage, build_agent_inbound_wake_prompt, clamp_agent_message,
};
use crate::extensions::session::production::ProductionSessionWorkers;
use crate::extensions::session::agent_session::{
    CommunicationEndpoint, SessionCommunicationDecision, plan_session_communication,
};
use crate::extensions::session::session_conversation_state::PendingConversationTurnClaimResult;
use crate::extensions::telemetry::agent_error_telemetry::AgentErrorReport;
use crate::extensions::telemetry::sand_error_tags::SandErrorValue;
use crate::extensions::trays::trays_service::PushErrorOptions;
use crate::ports::telemetry::SandErrorDetail;
use super::send_message_shaping::load_agent_inbound_images;
use super::run_scheduler::RunLane;
use super::transcript_entry_ids::{TranscriptEntryIdKind, next_entry_id};

pub const PRIORITY_AGENT_MESSAGE_INTERRUPT_REASON: &str = "superseded by a priority agent message";

pub fn agent_inbound_failure_report(
    agent_id: &str,
    request_id: Option<&str>,
    error_code: &str,
    detail: &str,
) -> AgentErrorReport {
    let error_code = error_code.trim();
    let error = if error_code.is_empty() {
        SandErrorValue::new("SAND-E0406")
    } else {
        SandErrorValue::new("SAND-E0406").with_string("connectCode", error_code)
    };
    AgentErrorReport {
        source: "agent".into(),
        conversation_id: agent_id.to_string(),
        request_id: request_id.map(str::to_string),
        error,
        detail: Some(SandErrorDetail {
            message: detail.to_string(),
            stack: None,
        }),
    }
}

pub fn agent_inbound_failure_tray(
    agent_id: &str,
    request_id: Option<&str>,
    error_code: &str,
    detail: &str,
) -> PushErrorOptions {
    PushErrorOptions {
        agent_id: Some(agent_id.to_string()),
        title: "Message from another agent failed".into(),
        detail: detail.to_string(),
        request_id: request_id.map(str::to_string),
        error_kind: (!error_code.trim().is_empty()).then(|| error_code.to_string()),
        raw_detail: (!detail.trim().is_empty()).then(|| detail.to_string()),
        ..PushErrorOptions::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInboundMessage {
    pub from: AgentAddressProjection,
    pub text: String,
    pub timestamp_ms: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<AgentMessageImage>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub priority: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_displayed: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_redriven: bool,
}

fn is_false(value: &bool) -> bool { !*value }

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentAddressProjection { pub id: String, pub name: String }

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentWakeRequest {
    pub agent_id: String,
    pub source_agent_id: String,
    pub prompt: String,
    pub priority: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_images: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inbound: Option<Value>,
}

pub type AgentWakeSink = Arc<dyn Fn(&AgentWakeRequest) + Send + Sync + 'static>;
pub type PriorityInterruptSink = Arc<dyn Fn(&str, &str) -> usize + Send + Sync + 'static>;
pub type AgentMessageAnalyticsSink =
    Arc<dyn Fn(&str, &str, bool, bool) + Send + Sync + 'static>;
pub type GroupPostSink =
    Arc<dyn Fn(&str, &str, &str, bool) -> Result<String, String> + Send + Sync + 'static>;

pub fn should_interrupt_priority_peer(active_lane: Option<RunLane>) -> bool {
    active_lane != Some(RunLane::User)
}

pub fn partition_agent_inbound<T: Clone>(messages: &[T], is_priority: impl Fn(&T) -> bool) -> (Vec<T>, Vec<T>) {
    let mut priority = Vec::new();
    let mut rest = Vec::new();
    for message in messages {
        if is_priority(message) { priority.push(message.clone()); } else { rest.push(message.clone()); }
    }
    (priority, rest)
}

pub fn prioritize_agent_inbound(messages: &[AgentInboundMessage]) -> Vec<AgentInboundMessage> {
    let (priority, rest) = partition_agent_inbound(messages, |m| m.priority);
    priority.into_iter().chain(rest).collect()
}

pub fn merge_agent_inbound_queue(queued: &[AgentInboundMessage], deferred: &[AgentInboundMessage]) -> Vec<AgentInboundMessage> {
    let (newer_priority, newer_rest) = partition_agent_inbound(queued, |m| m.priority);
    let (older_priority, older_rest) = partition_agent_inbound(deferred, |m| m.priority);
    newer_priority.into_iter().chain(older_priority).chain(older_rest).chain(newer_rest).collect()
}

pub struct ProductionAgentToAgentMessaging {
    sessions: Arc<ProductionSessionWorkers>,
    wake_sink: AgentWakeSink,
    priority_interrupt: Option<PriorityInterruptSink>,
    analytics: Option<AgentMessageAnalyticsSink>,
    group_post: Option<GroupPostSink>,
}

impl ProductionAgentToAgentMessaging {
    pub fn new(sessions: Arc<ProductionSessionWorkers>, wake_sink: AgentWakeSink, priority_interrupt: Option<PriorityInterruptSink>) -> Self {
        Self { sessions, wake_sink, priority_interrupt, analytics: None, group_post: None }
    }

    pub fn with_analytics(mut self, analytics: AgentMessageAnalyticsSink) -> Self {
        self.analytics = Some(analytics);
        self
    }

    pub fn with_group_post(mut self, group_post: GroupPostSink) -> Self {
        self.group_post = Some(group_post);
        self
    }

    pub fn send_to_agent(
        &self,
        from_agent_id: &str,
        to_agent_id: &str,
        text: &str,
        images: &[AgentMessageImage],
        priority: bool,
    ) -> Result<String, String> {
        self.send_to_agent_with_session(
            from_agent_id,
            None,
            to_agent_id,
            text,
            images,
            priority,
        )
    }

    pub fn send_to_agent_with_session(
        &self,
        from_agent_id: &str,
        from_session_key: Option<&str>,
        to_agent_id: &str,
        text: &str,
        images: &[AgentMessageImage],
        priority: bool,
    ) -> Result<String, String> {
        self.send_to_agent_with_session_and_reply(
            from_agent_id,
            from_session_key,
            to_agent_id,
            text,
            images,
            priority,
            None,
        )
    }

    pub fn send_to_agent_with_session_and_reply(
        &self,
        from_agent_id: &str,
        from_session_key: Option<&str>,
        to_agent_id: &str,
        text: &str,
        images: &[AgentMessageImage],
        priority: bool,
        reply_to_id: Option<&str>,
    ) -> Result<String, String> {
        let message = clamp_agent_message(text);
        if message.is_empty() { return Ok("Message was empty; nothing was sent.".into()); }
        if to_agent_id == from_agent_id { return Ok("An agent can't message itself.".into()); }
        if self.sessions.is_agent_being_deleted(to_agent_id) {
            return Ok("That agent no longer exists.".into());
        }
        let Some(target) = self.sessions.summarize_agent_by_id(to_agent_id, None)? else { return Ok(format!("No agent found with id {to_agent_id}.")); };
        if target.remote_room.is_some() { return Ok("That is a shared chat hosted by another user; agents can't message it directly.".into()); }
        let sender = self.sessions.summarize_agent_by_id(from_agent_id, None)?;
        let sender_name = sender.as_ref().map(|a| a.name.clone()).unwrap_or_else(|| "An agent".into());
        let timestamp_ms = now_ms();

        if target.is_group {
            let Some(group_post) = self.group_post.as_ref() else {
                return Ok("Messaging isn't available right now.".into());
            };
            let ack = match group_post(from_agent_id, to_agent_id, &message, priority) {
                Ok(ack) => ack,
                Err(error) => return Ok(error),
            };
            let mut notes = Vec::new();
            if !images.is_empty() {
                notes.push(format!("Note: the attached image{} {} NOT delivered — group messages are text-only for now; send images to an agent directly.",
                    if images.len()==1 {""} else {"s"}, if images.len()==1 {"was"} else {"were"}));
            }
            if priority { notes.push("Note: priority is 1:1 only — this post did not interrupt members.".into()); }
            return Ok(if notes.is_empty(){ack}else{format!("{ack} {}",notes.join(" "))});
        }

        let source_admission = self.sessions.read_session_work_admission(from_agent_id)?;
        let target_admission = self.sessions.read_session_work_admission(to_agent_id)?;
        let sender_session_key = match from_session_key
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
        {
            Some(session_key) => {
                self.sessions
                    .assert_session_work_admission(from_agent_id, Some(&session_key))?;
                self.sessions
                    .read_session_work_admission(from_agent_id)?
                    .filter(|record| record.session_key == session_key)
                    .map(|record| record.session_key)
            }
            None => source_admission
                .as_ref()
                .map(|record| record.session_key.clone())
                .filter(|value| !value.trim().is_empty()),
        };
        if let Some(session_key) = sender_session_key.as_deref() {
            self.sessions
                .assert_session_work_admission(from_agent_id, Some(session_key))?;
        }
        let target_session_key = target_admission
            .as_ref()
            .map(|record| record.session_key.clone())
            .unwrap_or_default();
        let source_endpoint = CommunicationEndpoint {
            agent_id: from_agent_id.to_string(),
            session_key: sender_session_key.clone().unwrap_or_default(),
            store_path: self.sessions.session_db_path(from_agent_id)?.to_string_lossy().into_owned(),
            session_id: source_admission.as_ref().map(|record| record.session_id.clone()),
            communication: self.sessions.read_session_communication_policy(from_agent_id)?,
        };
        let target_endpoint = CommunicationEndpoint {
            agent_id: to_agent_id.to_string(),
            session_key: target_session_key,
            store_path: self.sessions.session_db_path(to_agent_id)?.to_string_lossy().into_owned(),
            session_id: target_admission.as_ref().map(|record| record.session_id.clone()),
            communication: self.sessions.read_session_communication_policy(to_agent_id)?,
        };
        match plan_session_communication(
            None,
            &[source_endpoint],
            &[target_endpoint],
            false,
        ) {
            SessionCommunicationDecision::Denied { error } => return Err(error),
            SessionCommunicationDecision::Allowed { approvals } if !approvals.is_empty() => {
                return Err(format!(
                    "Session communication requires approval before delivery ({} approval(s)).",
                    approvals.len()
                ));
            }
            SessionCommunicationDecision::Allowed { .. } => {}
        }
        if let Some(target_admission) = target_admission.as_ref() {
            self.sessions.assert_session_work_admission(
                to_agent_id,
                Some(&target_admission.session_key),
            )?;
        }

        if let Some(analytics) = self.analytics.as_ref() {
            analytics(from_agent_id, to_agent_id, false, priority);
        }
        let source_entries=self.sessions.read_agent_transcript_entries(from_agent_id)?;
        let outbound_id=next_entry_id(&source_entries,TranscriptEntryIdKind::AssistantMessage);
        let image_json=serde_json::to_value(images).map_err(|e|format!("could not encode agent message images: {e}"))?;
        let pending_turn_id = if let (Some(session_key), Some(source_admission)) =
            (sender_session_key.as_deref(), source_admission.as_ref())
        {
            let turns = self.sessions.pending_conversation_turns();
            Some(turns.register(
                from_agent_id,
                None,
                to_agent_id,
                &source_admission.session_id,
                None,
                Duration::from_secs(15 * 60),
            )?)
        } else {
            None
        };
        let mut outbound_entry = json!({
            "kind":"message","id":outbound_id,"role":"assistant","content":message,"isStreaming":false,
            "timestampMs":timestamp_ms,"toAgent":{"id":to_agent_id,"name":target.name,"kind":"agent"},"images":image_json.clone()
        });
        if let Some(reply_to_id) = reply_to_id.map(str::trim).filter(|value| !value.is_empty()) {
            outbound_entry["replyToId"] = Value::String(reply_to_id.to_string());
        }
        let append_result = if let Some(session_key) = sender_session_key.as_deref() {
            outbound_entry["sessionKey"] = Value::String(session_key.trim().to_string());
            self.sessions.append_agent_transcript_entries_if_session_admitted(
                from_agent_id,
                session_key,
                &[outbound_entry],
            )
        } else {
            self.sessions
                .append_agent_transcript_entries(from_agent_id, &[outbound_entry])
        };
        if let Err(error) = append_result {
            if let Some(turn_id) = pending_turn_id.as_deref() {
                let _ = self.sessions.pending_conversation_turns().cancel(from_agent_id, turn_id);
            }
            return Err(error);
        }
        if let Some(turn_id) = pending_turn_id.as_deref() {
            let turns = self.sessions.pending_conversation_turns();
            if !turns.set_outbound_message_id(from_agent_id, turn_id, &outbound_id)
                || !turns.mark_ready(from_agent_id, turn_id)
            {
                turns.cancel(from_agent_id, turn_id);
                return Err("could not finalize direct-agent conversation correlation".into());
            }
        }
        let _ = self
            .sessions
            .add_agent_conversation_partner(from_agent_id, to_agent_id);
        let inbound = json!({
            "from": {"id": from_agent_id, "name": sender_name},
            "text": message,
            "timestampMs": timestamp_ms,
            "images": image_json,
            "priority": priority,
            "messageId": outbound_id.clone(),
            "replyContext": {"targetId": outbound_id.clone(), "quote": message},
        });
        let mut inbound = inbound;
        if let Some(reply_to_id) = reply_to_id.map(str::trim).filter(|value| !value.is_empty()) {
            inbound["replyToId"] = Value::String(reply_to_id.to_string());
        }
        let from=AgentAddress{
            id:from_agent_id.into(), name:sender_name,
            description:sender.as_ref().and_then(|a|{let v=a.description.trim();(!v.is_empty()).then(||v.to_string())}),
            is_group:false,
        };
        let image_envelopes = images
            .iter()
            .map(|image| json!({"url": image.url, "alt": image.alt}))
            .collect::<Vec<_>>();
        let selected_images = load_agent_inbound_images(Some(&image_envelopes))
            .into_iter()
            .map(|image| {
                let mut selected = serde_json::Map::new();
                selected.insert(
                    "data".into(),
                    Value::Array(
                        image
                            .data
                            .into_iter()
                            .map(|byte| Value::Number(u64::from(byte).into()))
                            .collect(),
                    ),
                );
                selected.insert("path".into(), Value::String(image.path));
                if let Some(mime_type) = image.mime_type {
                    selected.insert("mimeType".into(), Value::String(mime_type.to_string()));
                }
                Value::Object(selected)
            })
            .collect::<Vec<_>>();
        (self.wake_sink)(&AgentWakeRequest{
            agent_id:to_agent_id.into(),source_agent_id:from_agent_id.into(),
            prompt:build_agent_inbound_wake_prompt(&from,&message,images,priority),priority,
            selected_images,
            inbound: Some(inbound),
        });
        if priority {
            if let Some(interrupt)=self.priority_interrupt.as_ref(){
                let _=interrupt(to_agent_id,PRIORITY_AGENT_MESSAGE_INTERRUPT_REASON);
            }
        }
        Ok(if priority {
            format!("Sent to {} as a priority message — it will interrupt their current non-user work and wake them now. This is asynchronous — if they reply, it'll arrive later as a new message that wakes you; don't wait on it now.",target.name)
        } else {
            format!("Sent to {}. This is asynchronous — if they reply, it'll arrive later as a new message that wakes you; don't wait on it now.",target.name)
        })
    }
}

pub fn persist_agent_inbound_message(
    sessions: &ProductionSessionWorkers,
    agent_id: &str,
    inbound: &Value,
) -> Result<bool, String> {
    if sessions.is_agent_being_deleted(agent_id) {
        return Ok(false);
    }
    let Some(target) = sessions.summarize_agent_by_id(agent_id, None)? else {
        return Ok(false);
    };
    if target.is_group || target.remote_room.is_some() {
        return Ok(false);
    }
    let from = inbound
        .get("from")
        .and_then(Value::as_object)
        .ok_or_else(|| "agent inbound message is missing from".to_string())?;
    let from_id = from
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "agent inbound message is missing from.id".to_string())?;
    if from_id == agent_id || sessions.is_agent_being_deleted(from_id) {
        return Ok(false);
    }
    let Some(source) = sessions.summarize_agent_by_id(from_id, None)? else {
        return Ok(false);
    };
    if source.is_group || source.remote_room.is_some() {
        return Ok(false);
    }
    let source_admission = sessions.read_session_work_admission(from_id)?;
    let target_admission = sessions.read_session_work_admission(agent_id)?;
    if let Some(source_admission) = source_admission.as_ref() {
        sessions.assert_session_work_admission(
            from_id,
            Some(&source_admission.session_key),
        )?;
    }
    if source_admission.is_some() || target_admission.is_some() {
        let source_endpoint = CommunicationEndpoint {
            agent_id: from_id.to_string(),
            session_key: source_admission
                .as_ref()
                .map(|admission| admission.session_key.clone())
                .unwrap_or_default(),
            store_path: sessions.session_db_path(from_id)?.to_string_lossy().into_owned(),
            session_id: source_admission
                .as_ref()
                .map(|admission| admission.session_id.clone()),
            communication: sessions.read_session_communication_policy(from_id)?,
        };
        let target_endpoint = CommunicationEndpoint {
            agent_id: agent_id.to_string(),
            session_key: target_admission
                .as_ref()
                .map(|admission| admission.session_key.clone())
                .unwrap_or_default(),
            store_path: sessions.session_db_path(agent_id)?.to_string_lossy().into_owned(),
            session_id: target_admission
                .as_ref()
                .map(|admission| admission.session_id.clone()),
            communication: sessions.read_session_communication_policy(agent_id)?,
        };
        match plan_session_communication(
            None,
            &[source_endpoint],
            &[target_endpoint],
            false,
        ) {
            SessionCommunicationDecision::Denied { error } => return Err(error),
            SessionCommunicationDecision::Allowed { approvals } if !approvals.is_empty() => {
                return Err(format!(
                    "Session communication requires approval before delivery ({} approval(s)).",
                    approvals.len()
                ));
            }
            SessionCommunicationDecision::Allowed { .. } => {}
        }
        if let Some(target_admission) = target_admission.as_ref() {
            sessions.assert_session_work_admission(
                agent_id,
                Some(&target_admission.session_key),
            )?;
        }
    }
    let from_name = source.name.trim();
    let from_name = if from_name.is_empty() { "An agent" } else { from_name };
    let text = inbound
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| "agent inbound message is missing text".to_string())?;
    let timestamp_ms = inbound
        .get("timestampMs")
        .and_then(Value::as_u64)
        .unwrap_or_else(now_ms);
    let inbound_message_id = inbound
        .get("messageId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let reply_to_id = inbound
        .get("replyToId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let entries = sessions.read_agent_transcript_entries(agent_id)?;
    if let Some(message_id) = inbound_message_id {
        if let Some(existing) = entries.iter().find(|entry| {
            entry.get("kind").and_then(Value::as_str) == Some("message")
                && entry
                    .get("fromAgent")
                    .and_then(Value::as_object)
                    .and_then(|from| from.get("id"))
                    .and_then(Value::as_str)
                    == Some(from_id)
                && entry.get("messageId").and_then(Value::as_str) == Some(message_id)
        }) {
            let same_payload = existing.get("content").and_then(Value::as_str) == Some(text)
                && existing.get("replyToId").and_then(Value::as_str) == reply_to_id
                && existing.get("images") == inbound.get("images")
                && existing.get("replyContext") == inbound.get("replyContext");
            if !same_payload {
                return Err(format!(
                    "agent inbound message identity conflict for {from_id}:{message_id}"
                ));
            }
            return Ok(false);
        }
    }
    let entry_id = inbound_message_id
        .map(|message_id| format!("agent-inbound:{from_id}:{message_id}"))
        .unwrap_or_else(|| next_entry_id(&entries, TranscriptEntryIdKind::UserMessage));
    let mut claimed_turn = None;
    if reply_to_id.is_some() {
        let Some(target_admission) = target_admission.as_ref() else { return Ok(false); };
        let claim = sessions.pending_conversation_turns().claim_reply(
            agent_id,
            from_id,
            None,
            &target_admission.session_id,
            &entry_id,
            reply_to_id,
            None,
            text,
            timestamp_ms,
        );
        match claim {
            PendingConversationTurnClaimResult::Claimed(reply) => claimed_turn = Some(reply),
            PendingConversationTurnClaimResult::NotFound
            | PendingConversationTurnClaimResult::NotReady => return Ok(false),
        }
    }
    let mut entry = json!({
        "kind": "message",
        "id": entry_id,
        "role": "user",
        "content": text,
        "isStreaming": false,
        "timestampMs": timestamp_ms,
        "fromAgent": {"id": from_id, "name": from_name},
    });
    if let Some(message_id) = inbound_message_id {
        entry["messageId"] = Value::String(message_id.to_string());
    }
    if let Some(images) = inbound.get("images").filter(|value| {
        value.as_array().is_some_and(|images| !images.is_empty())
    }) {
        entry["images"] = images.clone();
    }
    if let Some(reply_context) = inbound.get("replyContext") {
        entry["replyContext"] = reply_context.clone();
    }
    if let Some(reply_to_id) = reply_to_id {
        entry["replyToId"] = Value::String(reply_to_id.to_string());
    }
    let append_result = if let Some(session_key) = sessions
        .read_session_work_admission(agent_id)?
        .map(|record| record.session_key)
        .filter(|value| !value.trim().is_empty())
    {
        entry["sessionKey"] = Value::String(session_key.clone());
        sessions.append_agent_transcript_entries_if_session_admitted(
            agent_id,
            &session_key,
            &[entry],
        )
    } else {
        sessions.append_agent_transcript_entries(agent_id, &[entry])
    };
    let inserted = match append_result {
        Ok(inserted) => inserted,
        Err(error) => {
            if let Some(reply) = claimed_turn.as_ref() {
                let _ = sessions.pending_conversation_turns().release(agent_id, &reply.id);
            }
            return Err(error);
        }
    };
    if inserted == 0 {
        if let Some(reply) = claimed_turn.as_ref() {
            let _ = sessions.pending_conversation_turns().release(agent_id, &reply.id);
        }
        return Ok(false);
    }
    if let Some(reply) = claimed_turn.take() {
        let _ = sessions.pending_conversation_turns().complete(agent_id, &reply.id);
    }
    let _ = sessions.add_agent_conversation_partner(agent_id, from_id)?;
    let _ = sessions.mark_agent_activity(agent_id, timestamp_ms as f64)?;
    Ok(true)
}

fn now_ms()->u64{
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis().min(u64::MAX as u128) as u64
}
