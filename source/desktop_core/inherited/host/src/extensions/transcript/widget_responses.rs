use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};

use crate::extensions::auto_review::auto_review_service::AutoReviewService;
use crate::extensions::local_tool_permission::local_tool_permission_resolution::{
    LocalToolPermissionWidgetResponses, StaleLocalToolPermissionCardSettlement,
};
use crate::extensions::session::production::ProductionSessionWorkers;
use crate::runner::sand_auto_review::SandAutoReviewExpiryCause;

use super::automation_runtime::AutomationRuntime;
use super::production_runtime::ProductionTranscriptRuntime;
use super::roster_emit::ProductionRosterEmit;
use super::sand_automation_spend_guard::SPEND_GUARD_VALUE_PREFIX;
use super::send_message_shaping::{
    Reaction, describe_reacted_message_quote, is_user_message_entry, skippable_prompt_summary,
    toggle_reaction,
};

pub const SAND_REACTION_SELF: &str = "me";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UnansweredQuestionPrompts {
    pub skipped_question_prompts: Vec<String>,
    pub dismissed_question_prompts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReactionApplication {
    pub before: Value,
    pub is_adding: bool,
}

#[derive(Clone)]
pub struct WidgetResponses {
    workers: Arc<ProductionSessionWorkers>,
    automation_runtime: Arc<AutomationRuntime>,
    transcript_runtime: Arc<ProductionTranscriptRuntime>,
    roster: Arc<Mutex<Option<Arc<ProductionRosterEmit>>>>,
    auto_review: Arc<Mutex<Option<Arc<AutoReviewService>>>>,
    channel_config_changed: Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync + 'static>>>>,
}

impl WidgetResponses {
    pub fn new(workers: Arc<ProductionSessionWorkers>) -> Self {
        Self::with_runtime(
            Arc::clone(&workers),
            Arc::new(AutomationRuntime::new(Arc::clone(&workers))),
            Arc::new(ProductionTranscriptRuntime::new(None)),
        )
    }

    pub fn with_runtime(
        workers: Arc<ProductionSessionWorkers>,
        automation_runtime: Arc<AutomationRuntime>,
        transcript_runtime: Arc<ProductionTranscriptRuntime>,
    ) -> Self {
        Self {
            workers,
            automation_runtime,
            transcript_runtime,
            roster: Arc::new(Mutex::new(None)),
            auto_review: Arc::new(Mutex::new(None)),
            channel_config_changed: Arc::new(Mutex::new(None)),
        }
    }

    pub fn bind_auto_review(&self, auto_review: Arc<AutoReviewService>) -> Result<(), String> {
        let mut slot = self
            .auto_review
            .lock()
            .map_err(|_| "widget response auto-review mutex poisoned".to_string())?;
        if slot.is_some() {
            return Err("widget response auto-review owner already configured".into());
        }
        *slot = Some(auto_review);
        Ok(())
    }

    pub fn bind_channel_config_changed(
        &self,
        on_changed: Arc<dyn Fn() + Send + Sync + 'static>,
    ) -> Result<(), String> {
        let mut slot = self
            .channel_config_changed
            .lock()
            .map_err(|_| "widget response channel-config mutex poisoned".to_string())?;
        if slot.is_some() {
            return Err("widget response channel-config signal already configured".into());
        }
        *slot = Some(on_changed);
        Ok(())
    }

    pub fn bind_roster(&self, roster: Arc<ProductionRosterEmit>) -> Result<(), String> {
        let mut slot = self
            .roster
            .lock()
            .map_err(|_| "widget response roster mutex poisoned".to_string())?;
        if slot.is_some() {
            return Err("widget response roster already configured".into());
        }
        *slot = Some(roster);
        Ok(())
    }

    fn roster(&self) -> Option<Arc<ProductionRosterEmit>> {
        self.roster
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn active_agent_id(&self) -> Option<String> {
        self.transcript_runtime.active_agent_id(&self.workers)
    }

    fn emit_updated(&self, agent_id: &str, entry: &Value) {
        let Some(roster) = self.roster() else {
            return;
        };
        if self.active_agent_id().as_deref() == Some(agent_id) {
            roster.publish_transcript_updated(agent_id, entry);
        } else {
            let _ = roster.emit_agent_update(agent_id);
        }
    }

    fn find_entry(&self, agent_id: &str, entry_id: &str) -> Result<Option<Value>, String> {
        Ok(self
            .workers
            .read_agent_transcript_entries(agent_id)?
            .into_iter()
            .find(|entry| entry.get("id").and_then(Value::as_str) == Some(entry_id)))
    }

    fn update_entry(
        &self,
        agent_id: &str,
        entry_id: &str,
        next: Value,
    ) -> Result<Option<Value>, String> {
        let session_key = self
            .find_entry(agent_id, entry_id)?
            .and_then(|entry| {
                entry
                    .get("sessionKey")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
            });
        let updated = if let Some(session_key) = session_key {
            self.workers.update_agent_transcript_entry_if_session_admitted(
                agent_id,
                &session_key,
                entry_id,
                &next,
            )?
        } else {
            self.workers
                .update_agent_transcript_entry(agent_id, entry_id, &next)?
        };
        if let Some(updated) = updated.as_ref() {
            self.emit_updated(agent_id, updated);
        }
        Ok(updated)
    }

    pub fn collect_unanswered_question_prompts(
        &self,
        agent_id: &str,
    ) -> Result<UnansweredQuestionPrompts, String> {
        let entries = self.workers.read_agent_transcript_entries(agent_id)?;
        let mut prompts = UnansweredQuestionPrompts::default();
        for entry in entries {
            if entry.get("kind").and_then(Value::as_str) != Some("send-message")
                || entry
                    .get("respondedValue")
                    .is_some_and(|value| !value.is_null())
                || entry.get("widgetSkipped").and_then(Value::as_bool) == Some(true)
            {
                continue;
            }
            let Some(message) = entry.get("message") else {
                continue;
            };
            let Some(summary) = skippable_prompt_summary(message) else {
                continue;
            };
            if entry.get("widgetDismissed").and_then(Value::as_bool) == Some(true) {
                prompts.dismissed_question_prompts.push(summary);
            } else {
                prompts.skipped_question_prompts.push(summary);
            }
            let Some(entry_id) = entry.get("id").and_then(Value::as_str) else {
                continue;
            };
            let mut next = entry.clone();
            if let Some(object) = next.as_object_mut() {
                object.insert("widgetSkipped".into(), Value::Bool(true));
            }
            let _ = self.update_entry(agent_id, entry_id, next)?;
        }
        Ok(prompts)
    }

    pub fn respond_to_widget_with<Send>(
        &self,
        entry_id: &str,
        value: &str,
        agent_id: &str,
        now_ms: f64,
        send: Send,
    ) -> Result<bool, String>
    where
        Send: FnOnce(Value) -> Result<(), String>,
    {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Ok(false);
        }
        let Some(existing) = self.find_entry(agent_id, entry_id)? else {
            return Ok(false);
        };
        if existing.get("kind").and_then(Value::as_str) != Some("send-message")
            || existing.pointer("/message/type").and_then(Value::as_str) != Some("widget")
            || existing
                .get("respondedValue")
                .is_some_and(|value| !value.is_null())
            || existing.get("widgetDismissed").and_then(Value::as_bool) == Some(true)
        {
            return Ok(false);
        }

        if existing
            .pointer("/message/widget/dismissOnMoveOn")
            .and_then(Value::as_bool)
            == Some(true)
        {
            let transcript = self.workers.read_agent_transcript_entries(agent_id)?;
            let surfaces = relevant_widget_surfaces(&transcript, entry_id);
            if !surfaces.is_empty()
                && surfaces
                    .iter()
                    .all(|surface| has_later_user_moment(surface, entry_id))
            {
                return Ok(false);
            }
        }

        let reply_to = existing
            .get("replyTo")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let mut stamped = existing.clone();
        stamped
            .as_object_mut()
            .expect("transcript entry must be object")
            .insert("respondedValue".into(), Value::String(trimmed.to_string()));
        if self.update_entry(agent_id, entry_id, stamped)?.is_none() {
            return Ok(false);
        }

        let mut model_prompt = trimmed.to_string();
        let mut guard_applied = false;
        if trimmed.starts_with(SPEND_GUARD_VALUE_PREFIX) {
            match self
                .automation_runtime
                .handle_spend_guard_answer(agent_id, entry_id, trimmed, now_ms)?
            {
                Some(applied) => {
                    model_prompt = applied;
                    guard_applied = true;
                }
                None => {
                    self.rollback_widget_response(agent_id, entry_id)?;
                    return Ok(false);
                }
            }
        }

        let mut args = Map::new();
        args.insert("agentId".into(), Value::String(agent_id.to_string()));
        args.insert("prompt".into(), Value::String(model_prompt));
        args.insert("appendUserMessage".into(), Value::Bool(false));
        args.insert("awaitTurn".into(), Value::Bool(false));
        if let Some(reply_to) = reply_to {
            args.insert("replyToId".into(), Value::String(reply_to));
        }
        if let Err(error) = send(Value::Object(args)) {
            if guard_applied {
                return Ok(true);
            }
            self.rollback_widget_response(agent_id, entry_id)?;
            return Err(error);
        }
        Ok(true)
    }

    pub fn rollback_widget_response(
        &self,
        agent_id: &str,
        entry_id: &str,
    ) -> Result<Option<Value>, String> {
        let Some(mut existing) = self.find_entry(agent_id, entry_id)? else {
            return Ok(None);
        };
        if let Some(object) = existing.as_object_mut() {
            object.remove("respondedValue");
        }
        self.update_entry(agent_id, entry_id, existing)
    }

    pub fn dismiss_widget(&self, entry_id: &str, agent_id: &str) -> Result<bool, String> {
        let Some(mut existing) = self.find_entry(agent_id, entry_id)? else {
            return Ok(false);
        };
        if existing.get("kind").and_then(Value::as_str) != Some("send-message")
            || existing.pointer("/message/type").and_then(Value::as_str) != Some("widget")
            || existing
                .get("respondedValue")
                .is_some_and(|value| !value.is_null())
            || existing.get("widgetDismissed").and_then(Value::as_bool) == Some(true)
        {
            return Ok(false);
        }
        let object = existing
            .as_object_mut()
            .expect("transcript entry must be object");
        object.remove("widgetSkipped");
        object.insert("widgetDismissed".into(), Value::Bool(true));
        Ok(self.update_entry(agent_id, entry_id, existing)?.is_some())
    }

    pub fn settle_stale_auto_review_card(
        &self,
        agent_id: &str,
        entry_id: &str,
        request_id: &str,
    ) -> Result<bool, String> {
        let auto_review = self
            .auto_review
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(auto_review) = auto_review {
            if auto_review.expire_pending_approval(
                request_id,
                agent_id,
                SandAutoReviewExpiryCause::Other("stale_card".into()),
            )? {
                return Ok(true);
            }
        }
        let expired = self
            .workers
            .expire_pending_auto_review_approvals(agent_id, Some(request_id))?;
        let settled = self
            .find_entry(agent_id, entry_id)?
            .filter(|entry| {
                entry.get("kind").and_then(Value::as_str) == Some("send-message")
                    && entry.pointer("/message/type").and_then(Value::as_str)
                        == Some("auto-review-approval")
                    && entry
                        .pointer("/message/approval/requestId")
                        .and_then(Value::as_str)
                        == Some(request_id)
            });
        if let Some(entry) = settled {
            self.emit_updated(agent_id, &entry);
            return Ok(true);
        }
        Ok(!expired.is_empty())
    }

    pub fn expire_all_pending_auto_review_approval_cards(&self) -> Result<usize, String> {
        let mut retired = 0usize;
        let auto_review = self
            .auto_review
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(auto_review) = auto_review {
            for agent_id in auto_review.agent_ids_with_pending_approvals() {
                for request_id in auto_review.pending_approval_ids_for_agent(&agent_id) {
                    if auto_review.expire_pending_approval(
                        &request_id,
                        &agent_id,
                        SandAutoReviewExpiryCause::Other("stale_sweep".into()),
                    )? {
                        retired = retired.saturating_add(1);
                    }
                }
            }
        }
        for agent_id in self.workers.list_agent_record_ids()? {
            let expired = self
                .workers
                .expire_pending_auto_review_approvals(&agent_id, None)?;
            if !expired.is_empty() {
                retired = retired.saturating_add(expired.len());
                if let Some(roster) = self.roster() {
                    let _ = roster.emit_agent_update(&agent_id);
                }
            }
        }
        Ok(retired)
    }

    pub fn settle_stale_local_tool_permission_card(
        &self,
        agent_id: &str,
        entry_id: &str,
        request_id: &str,
    ) -> Result<StaleLocalToolPermissionCardSettlement, String> {
        let expired = self.workers.expire_pending_local_tool_permission_asks(
            agent_id,
            Some(request_id),
            None,
        )?;
        if !expired.is_empty() {
            if let Some(entry) = self.find_entry(agent_id, entry_id)? {
                self.emit_updated(agent_id, &entry);
            }
            return Ok(StaleLocalToolPermissionCardSettlement::Retired);
        }

        let settled = self
            .workers
            .read_agent_transcript_entries(agent_id)?
            .into_iter()
            .any(|entry| is_local_tool_permission_entry(&entry, entry_id, request_id));
        Ok(if settled {
            StaleLocalToolPermissionCardSettlement::Settled
        } else {
            StaleLocalToolPermissionCardSettlement::NotSettled
        })
    }

    pub fn expire_all_pending_local_tool_permission_cards(
        &self,
        if_pending_before_ms: Option<u64>,
    ) -> usize {
        let Ok(agent_ids) = self.workers.list_agent_record_ids() else {
            return 0;
        };
        let mut expired_count = 0usize;
        for agent_id in agent_ids {
            let expired = self.workers.expire_pending_local_tool_permission_asks(
                &agent_id,
                None,
                if_pending_before_ms.map(|value| value as f64),
            );
            if let Ok(expired) = expired {
                if !expired.is_empty() {
                    if let Some(roster) = self.roster() {
                        let _ = roster.emit_agent_update(&agent_id);
                    }
                }
                expired_count = expired_count.saturating_add(expired.len());
            }
        }
        expired_count
    }

    pub fn submit_secret(
        &self,
        entry_id: &str,
        value: &str,
        agent_id: &str,
    ) -> Result<Option<String>, String> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        let Some(mut entry) = self.find_entry(agent_id, entry_id)? else {
            return Ok(None);
        };
        if entry.get("kind").and_then(Value::as_str) != Some("send-message")
            || entry.pointer("/message/type").and_then(Value::as_str) != Some("secret-request")
            || entry.get("secretProvided").and_then(Value::as_bool) == Some(true)
        {
            return Ok(None);
        }
        let request = entry
            .pointer("/message/secretRequest")
            .ok_or_else(|| "secret request payload missing".to_string())?;
        let target = request
            .get("target")
            .ok_or_else(|| "secret request target missing".to_string())?;
        if target.get("kind").and_then(Value::as_str) != Some("channel-credential") {
            return Ok(None);
        }
        let platform = target
            .get("platform")
            .and_then(Value::as_str)
            .ok_or_else(|| "secret request platform missing".to_string())?;
        let field = target
            .get("field")
            .and_then(Value::as_str)
            .ok_or_else(|| "secret request field missing".to_string())?;
        if !self
            .workers
            .store_connector_credential(agent_id, platform, field, trimmed)?
        {
            return Err("could not store connector credential".into());
        }
        if let Some(on_changed) = self
            .channel_config_changed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            on_changed();
        }
        let label = request
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or("credential")
            .to_string();
        let target_kind = target
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("channel-credential")
            .to_string();
        entry
            .as_object_mut()
            .expect("transcript entry must be object")
            .insert("secretProvided".into(), Value::Bool(true));
        if self.update_entry(agent_id, entry_id, entry)?.is_none() {
            return Err("secret request response was not durably recorded".into());
        }
        Ok(Some(crate::runner::tools::sand_secret_request::build_secret_provided_ack(
            &label,
            &target_kind,
        )))
    }

    pub fn apply_reaction(
        &self,
        agent_id: &str,
        entry_id: &str,
        emoji: &str,
        by: &str,
    ) -> Result<Option<ReactionApplication>, String> {
        let Some(mut entry) = self.find_entry(agent_id, entry_id)? else {
            return Ok(None);
        };
        let before = entry.clone();
        let current = entry
            .get("reactions")
            .and_then(Value::as_array)
            .map(|reactions| {
                reactions
                    .iter()
                    .filter_map(|reaction| {
                        Some(Reaction {
                            emoji: reaction.get("emoji")?.as_str()?.to_string(),
                            by: reaction.get("by")?.as_str()?.to_string(),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let has = current
            .iter()
            .any(|reaction| reaction.emoji == emoji && reaction.by == by);
        let next = toggle_reaction(Some(&current), emoji, by);
        let object = entry
            .as_object_mut()
            .expect("transcript entry must be object");
        if let Some(next) = next {
            object.insert(
                "reactions".into(),
                Value::Array(
                    next.into_iter()
                        .map(|reaction| json!({"emoji": reaction.emoji, "by": reaction.by}))
                        .collect(),
                ),
            );
        } else {
            object.remove("reactions");
        }
        if self.update_entry(agent_id, entry_id, entry)?.is_none() {
            return Ok(None);
        }
        Ok(Some(ReactionApplication {
            before,
            is_adding: !has,
        }))
    }

    pub fn react_to_message(
        &self,
        entry_id: &str,
        emoji: &str,
        agent_id: &str,
    ) -> Result<Option<String>, String> {
        let trimmed = emoji.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        let Some(result) =
            self.apply_reaction(agent_id, entry_id, trimmed, SAND_REACTION_SELF)?
        else {
            return Ok(None);
        };
        if !result.is_adding || is_user_message_entry(&result.before) {
            return Ok(None);
        }
        let quote = describe_reacted_message_quote(&result.before);
        Ok(Some(format!(
            "[The user reacted {trimmed} to your message: \"{quote}\". You don't need to reply; act on it only if it's useful (e.g. acknowledge, adjust, or continue).]"
        )))
    }
}

fn is_agent_peer_message_entry(entry: &Value) -> bool {
    entry.get("kind").and_then(Value::as_str) == Some("message")
        && (entry.get("fromAgent").is_some() || entry.get("toAgent").is_some())
}

fn has_later_user_moment(scope: &[Value], entry_id: &str) -> bool {
    let Some(index) = scope
        .iter()
        .position(|entry| entry.get("id").and_then(Value::as_str) == Some(entry_id))
    else {
        return false;
    };
    scope.iter().skip(index + 1).any(|entry| {
        (is_user_message_entry(entry) && !is_agent_peer_message_entry(entry))
            || (entry.get("kind").and_then(Value::as_str) == Some("send-message")
                && entry.pointer("/message/type").and_then(Value::as_str) == Some("widget")
                && (entry
                    .get("respondedValue")
                    .is_some_and(|value| !value.is_null())
                    || entry.get("widgetDismissed").and_then(Value::as_bool) == Some(true)))
    })
}

fn is_branched_entry(entry: &Value) -> bool {
    matches!(
        entry.get("kind").and_then(Value::as_str),
        Some("message" | "send-message" | "user-attachment" | "notice")
    ) && entry.get("branched").and_then(Value::as_bool) == Some(true)
}

fn branch_root(
    entry: &Value,
    by_id: &HashMap<String, &Value>,
) -> Option<String> {
    let mut current = entry;
    let mut seen = HashSet::new();
    if let Some(id) = current.get("id").and_then(Value::as_str) {
        seen.insert(id.to_string());
    }
    loop {
        let parent_id = current.get("replyTo").and_then(Value::as_str)?;
        let parent = by_id.get(parent_id)?;
        if !is_branched_entry(parent) {
            return Some(parent_id.to_string());
        }
        if !seen.insert(parent_id.to_string()) {
            return None;
        }
        current = parent;
    }
}

fn main_transcript_entries(entries: &[Value]) -> Vec<Value> {
    let by_id = entries
        .iter()
        .filter_map(|entry| Some((entry.get("id")?.as_str()?.to_string(), entry)))
        .collect::<HashMap<_, _>>();
    entries
        .iter()
        .filter(|entry| !(is_branched_entry(entry) && branch_root(entry, &by_id).is_some()))
        .cloned()
        .collect()
}

fn thread_transcript_entries(entries: &[Value], root_id: &str) -> Vec<Value> {
    let by_id = entries
        .iter()
        .filter_map(|entry| Some((entry.get("id")?.as_str()?.to_string(), entry)))
        .collect::<HashMap<_, _>>();
    if !by_id.contains_key(root_id) {
        return Vec::new();
    }
    let mut thread = HashSet::from([root_id.to_string()]);
    let mut changed = true;
    while changed {
        changed = false;
        for entry in entries {
            let Some(id) = entry.get("id").and_then(Value::as_str) else {
                continue;
            };
            let Some(parent) = entry.get("replyTo").and_then(Value::as_str) else {
                continue;
            };
            if is_branched_entry(entry) && thread.contains(parent) && thread.insert(id.to_string()) {
                changed = true;
            }
        }
    }
    entries
        .iter()
        .filter(|entry| {
            entry
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| thread.contains(id))
        })
        .cloned()
        .collect()
}

fn relevant_widget_surfaces(entries: &[Value], entry_id: &str) -> Vec<Vec<Value>> {
    let mut surfaces = Vec::new();
    let main = main_transcript_entries(entries);
    if main
        .iter()
        .any(|entry| entry.get("id").and_then(Value::as_str) == Some(entry_id))
    {
        surfaces.push(main);
    }
    let thread = thread_transcript_entries(entries, entry_id);
    if thread.len() > 1 {
        surfaces.push(thread);
    }
    surfaces
}

impl LocalToolPermissionWidgetResponses for WidgetResponses {
    fn settle_stale_local_tool_permission_card(
        &self,
        agent_id: &str,
        entry_id: &str,
        request_id: &str,
    ) -> Result<StaleLocalToolPermissionCardSettlement, String> {
        WidgetResponses::settle_stale_local_tool_permission_card(
            self,
            agent_id,
            entry_id,
            request_id,
        )
    }
}

fn is_local_tool_permission_entry(
    entry: &Value,
    entry_id: &str,
    request_id: &str,
) -> bool {
    entry.get("id").and_then(Value::as_str) == Some(entry_id)
        && entry.get("kind").and_then(Value::as_str) == Some("send-message")
        && entry
            .get("message")
            .and_then(|message| message.get("type"))
            .and_then(Value::as_str)
            == Some("local-tool-permission")
        && entry
            .get("message")
            .and_then(|message| message.get("ask"))
            .and_then(|ask| ask.get("requestId"))
            .and_then(Value::as_str)
            == Some(request_id)
}
