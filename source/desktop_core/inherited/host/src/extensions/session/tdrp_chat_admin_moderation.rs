//! Telegram / TDRP Chat Admin, Member Moderation, Pins, Mute, and Notifications.

use super::*;

impl ProductionSessionWorkers {
    pub fn upsert_conversation_member(
        &self,
        conversation_id: &str,
        member: fabushi_messaging_core::CommunityMember,
    ) -> Result<serde_json::Value, String> {
        let local_human_id = self.local_human_id()?;
        let conversation_id = conversation_id.trim();
        if conversation_id.is_empty() {
            return Err("upsertConversationMember requires conversationId".into());
        }
        let owner = self.open_human_conversation_db_owner(conversation_id)?;
        let mut metadata = owner.read_metadata().map_err(|e| e.to_string())?;
        let kind = metadata.get("conversationKind").and_then(serde_json::Value::as_str).unwrap_or("human");
        if kind != "group" && kind != "channel" {
            return Err("upsertConversationMember is only supported on group or channel conversations".into());
        }
        let caller_member = owner.get_conversation_member(local_human_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "caller is not a member of the conversation".to_string())?;
        let is_owner = caller_member.status == fabushi_messaging_core::MemberStatus::Owner;
        let is_admin = caller_member.status == fabushi_messaging_core::MemberStatus::Administrator;

        let target_status = member.status;
        let authorized = if is_owner {
            true
        } else if is_admin {
            match target_status {
                fabushi_messaging_core::MemberStatus::Administrator => caller_member.admin_rights.add_admins,
                fabushi_messaging_core::MemberStatus::Banned | fabushi_messaging_core::MemberStatus::Restricted => {
                    caller_member.admin_rights.ban_members
                }
                fabushi_messaging_core::MemberStatus::Member => {
                    caller_member.admin_rights.invite_members || caller_member.admin_rights.ban_members
                }
                _ => false,
            }
        } else {
            false
        };
        if !authorized {
            return Err("caller lacks permission to update conversation member".into());
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        owner.upsert_conversation_member(&member, now_ms).map_err(|e| e.to_string())?;

        if let Some(participant_ids) = metadata.get_mut("participantIds").and_then(serde_json::Value::as_array_mut) {
            let actor_str = member.actor_id.0.as_str();
            let exists = participant_ids.iter().any(|id| id.as_str() == Some(actor_str));
            match member.status {
                fabushi_messaging_core::MemberStatus::Banned | fabushi_messaging_core::MemberStatus::Left => {
                    participant_ids.retain(|id| id.as_str() != Some(actor_str));
                }
                _ => {
                    if !exists {
                        participant_ids.push(serde_json::json!(actor_str));
                    }
                }
            }
            let _ = owner.set_metadata("participantIds", serde_json::Value::Array(participant_ids.clone()));
        }

        serde_json::to_value(&member).map_err(|e| e.to_string())
    }

    pub fn get_conversation_member(
        &self,
        conversation_id: &str,
        actor_id: &str,
    ) -> Result<Option<fabushi_messaging_core::CommunityMember>, String> {
        let owner = self.open_human_conversation_db_owner(conversation_id.trim())?;
        owner.get_conversation_member(actor_id.trim()).map_err(|e| e.to_string())
    }

    pub fn list_conversation_members(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<fabushi_messaging_core::CommunityMember>, String> {
        let owner = self.open_human_conversation_db_owner(conversation_id.trim())?;
        owner.list_conversation_members().map_err(|e| e.to_string())
    }

    pub fn pin_conversation_message(
        &self,
        conversation_id: &str,
        message_id: &str,
        topic_id: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        let conversation_id = conversation_id.trim();
        if conversation_id.is_empty() {
            return Err("pinConversationMessage requires conversationId".into());
        }
        let message_id = message_id.trim();
        if message_id.is_empty() {
            return Err("pinConversationMessage requires messageId".into());
        }
        let local_human_id = self.local_human_id()?;
        let child_scope = match topic_id.map(str::trim).filter(|s| !s.is_empty()) {
            Some(tid) => format!("topic:{tid}"),
            None => "main".to_string(),
        };
        let owner = self.open_human_conversation_db_owner(conversation_id)?;
        let metadata = owner.read_metadata().map_err(|e| e.to_string())?;

        let kind = metadata.get("conversationKind").and_then(serde_json::Value::as_str).unwrap_or("human");
        if kind == "group" || kind == "channel" {
            let caller_member = owner
                .get_conversation_member(local_human_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "caller is not a conversation member".to_string())?;
            let can_pin = caller_member.status == fabushi_messaging_core::MemberStatus::Owner
                || (caller_member.status == fabushi_messaging_core::MemberStatus::Administrator
                    && caller_member.admin_rights.pin_messages);
            if !can_pin {
                return Err("caller lacks pin_messages permission".into());
            }
        } else {
            let is_participant = metadata
                .get("participantIds")
                .and_then(serde_json::Value::as_array)
                .map(|arr| arr.iter().any(|p| p.as_str() == Some(local_human_id)))
                .unwrap_or(false);
            if !is_participant {
                return Err("caller is not a conversation participant".into());
            }
        }

        let entries = owner.get_transcript_entries().map_err(|e| e.to_string())?;
        let target_topic = topic_id.map(str::trim).filter(|s| !s.is_empty());
        let exists = entries.iter().any(|entry| {
            let entry_id = entry.get("id").and_then(serde_json::Value::as_str).unwrap_or_default();
            if entry_id != message_id {
                return false;
            }
            if let Some(target_tid) = target_topic {
                entry.get("topicId").and_then(serde_json::Value::as_str) == Some(target_tid)
            } else {
                entry.get("topicId").and_then(serde_json::Value::as_str).is_none()
            }
        });
        if !exists {
            return Err(format!("message {message_id} not found in conversation transcript"));
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        owner
            .pin_conversation_message(&child_scope, message_id, now_ms, local_human_id)
            .map_err(|e| e.to_string())?;
        let pinned_message_ids = owner
            .list_conversation_pins(&child_scope)
            .map_err(|e| e.to_string())?;

        Ok(serde_json::json!({
            "conversationId": conversation_id,
            "childScope": child_scope,
            "messageId": message_id,
            "pinnedBy": local_human_id,
            "pinnedAtMs": now_ms,
            "pinnedMessageIds": pinned_message_ids,
        }))
    }

    pub fn unpin_conversation_message(
        &self,
        conversation_id: &str,
        message_id: &str,
        topic_id: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        let conversation_id = conversation_id.trim();
        if conversation_id.is_empty() {
            return Err("unpinConversationMessage requires conversationId".into());
        }
        let message_id = message_id.trim();
        if message_id.is_empty() {
            return Err("unpinConversationMessage requires messageId".into());
        }
        let local_human_id = self.local_human_id()?;
        let child_scope = match topic_id.map(str::trim).filter(|s| !s.is_empty()) {
            Some(tid) => format!("topic:{tid}"),
            None => "main".to_string(),
        };
        let owner = self.open_human_conversation_db_owner(conversation_id)?;
        let metadata = owner.read_metadata().map_err(|e| e.to_string())?;

        let kind = metadata.get("conversationKind").and_then(serde_json::Value::as_str).unwrap_or("human");
        if kind == "group" || kind == "channel" {
            let caller_member = owner
                .get_conversation_member(local_human_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "caller is not a conversation member".to_string())?;
            let can_pin = caller_member.status == fabushi_messaging_core::MemberStatus::Owner
                || (caller_member.status == fabushi_messaging_core::MemberStatus::Administrator
                    && caller_member.admin_rights.pin_messages);
            if !can_pin {
                return Err("caller lacks pin_messages permission".into());
            }
        } else {
            let is_participant = metadata
                .get("participantIds")
                .and_then(serde_json::Value::as_array)
                .map(|arr| arr.iter().any(|p| p.as_str() == Some(local_human_id)))
                .unwrap_or(false);
            if !is_participant {
                return Err("caller is not a conversation participant".into());
            }
        }

        owner
            .unpin_conversation_message(&child_scope, message_id)
            .map_err(|e| e.to_string())?;
        let pinned_message_ids = owner
            .list_conversation_pins(&child_scope)
            .map_err(|e| e.to_string())?;

        Ok(serde_json::json!({
            "conversationId": conversation_id,
            "childScope": child_scope,
            "messageId": message_id,
            "pinnedMessageIds": pinned_message_ids,
        }))
    }

    pub fn list_conversation_pins(
        &self,
        conversation_id: &str,
        topic_id: Option<&str>,
    ) -> Result<Vec<String>, String> {
        let conversation_id = conversation_id.trim();
        if conversation_id.is_empty() {
            return Err("listConversationPins requires conversationId".into());
        }
        let child_scope = match topic_id.map(str::trim).filter(|s| !s.is_empty()) {
            Some(tid) => format!("topic:{tid}"),
            None => "main".to_string(),
        };
        let owner = self.open_human_conversation_db_owner(conversation_id)?;
        owner.list_conversation_pins(&child_scope).map_err(|e| e.to_string())
    }

    pub fn set_conversation_topic_mute(
        &self,
        conversation_id: &str,
        topic_id: Option<&str>,
        muted_until_ms: Option<i64>,
    ) -> Result<serde_json::Value, String> {
        let conversation_id = conversation_id.trim();
        if conversation_id.is_empty() {
            return Err("setConversationTopicMute requires conversationId".into());
        }
        let owner = self.open_human_conversation_db_owner(conversation_id)?;
        let mut metadata = owner.read_metadata().map_err(|e| e.to_string())?;

        let mut rule: fabushi_messaging_core::ConversationNotificationRule = match metadata.get("notificationRule") {
            Some(val) if !val.is_null() => serde_json::from_value(val.clone()).map_err(|e| e.to_string())?,
            _ => fabushi_messaging_core::ConversationNotificationRule {
                conversation_id: fabushi_messaging_core::ConversationId::new(conversation_id),
                muted_until_ms: None,
                sound: None,
                show_preview: true,
                notify_mentions: true,
                importance: fabushi_messaging_core::NotificationImportance::Normal,
                topic_muted_until_ms: std::collections::BTreeMap::new(),
            },
        };

        if let Some(tid) = topic_id.map(str::trim).filter(|s| !s.is_empty()) {
            let topic = owner.get_conversation_topic(tid).map_err(|e| e.to_string())?;
            if topic.is_none() {
                return Err(format!("topic {tid} does not exist"));
            }
            match muted_until_ms {
                Some(until) => {
                    rule.topic_muted_until_ms.insert(tid.to_string(), until);
                }
                None => {
                    rule.topic_muted_until_ms.remove(tid);
                }
            }
        } else {
            rule.muted_until_ms = muted_until_ms;
        }

        let val = serde_json::to_value(&rule).map_err(|e| e.to_string())?;
        owner.set_metadata("notificationRule", val.clone()).map_err(|e| e.to_string())?;
        Ok(val)
    }

    pub fn evaluate_message_notification(
        &self,
        conversation_id: &str,
        message_id: &str,
    ) -> Result<serde_json::Value, String> {
        let conversation_id = conversation_id.trim();
        if conversation_id.is_empty() {
            return Err("evaluateMessageNotification requires conversationId".into());
        }
        let message_id = message_id.trim();
        if message_id.is_empty() {
            return Err("evaluateMessageNotification requires messageId".into());
        }
        let local_human_id = self.local_human_id()?;
        let owner = self.open_human_conversation_db_owner(conversation_id)?;
        let metadata = owner.read_metadata().map_err(|e| e.to_string())?;

        let rule: fabushi_messaging_core::ConversationNotificationRule = match metadata.get("notificationRule") {
            Some(val) if !val.is_null() => serde_json::from_value(val.clone()).map_err(|e| e.to_string())?,
            _ => fabushi_messaging_core::ConversationNotificationRule {
                conversation_id: fabushi_messaging_core::ConversationId::new(conversation_id),
                muted_until_ms: None,
                sound: None,
                show_preview: true,
                notify_mentions: true,
                importance: fabushi_messaging_core::NotificationImportance::Normal,
                topic_muted_until_ms: std::collections::BTreeMap::new(),
            },
        };

        let entries = owner.get_transcript_entries().map_err(|e| e.to_string())?;
        let msg = entries
            .iter()
            .find(|entry| entry.get("id").and_then(serde_json::Value::as_str) == Some(message_id))
            .ok_or_else(|| format!("message {message_id} not found in conversation transcript"))?;

        let sender_id = msg
            .get("authorId")
            .or_else(|| msg.get("senderId"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        let content = msg.get("content").and_then(serde_json::Value::as_str).unwrap_or_default();
        let topic_id = msg.get("topicId").and_then(serde_json::Value::as_str).map(|s| s.to_string());
        let silent = msg.get("silent").and_then(serde_json::Value::as_bool).unwrap_or(false);

        let mention_tag = format!("@{local_human_id}");
        let has_text_mention = content.contains(&mention_tag);
        let has_array_mention = msg
            .get("mentions")
            .and_then(serde_json::Value::as_array)
            .map(|arr| arr.iter().any(|v| v.as_str() == Some(local_human_id)))
            .unwrap_or(false);
        let is_mentioned = has_text_mention || has_array_mention;

        let mut mentioned_actor_ids = std::collections::BTreeSet::new();
        if is_mentioned {
            mentioned_actor_ids.insert(fabushi_messaging_core::ActorId::new(local_human_id));
        }

        let candidate = fabushi_messaging_core::NotificationCandidate {
            id: format!("notif-{message_id}"),
            conversation_id: fabushi_messaging_core::ConversationId::new(conversation_id),
            sender_id: fabushi_messaging_core::ActorId::new(sender_id),
            title: metadata
                .get("title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(conversation_id)
                .to_string(),
            body: content.to_string(),
            created_at_ms: msg.get("timestamp").and_then(serde_json::Value::as_i64).unwrap_or(0),
            mentioned_actor_ids,
            is_call: false,
            silent_message: silent,
            topic_id,
        };

        let mut conversation_rules = std::collections::BTreeMap::new();
        conversation_rules.insert(rule.conversation_id.clone(), rule);
        let policy = fabushi_messaging_core::NotificationPolicy {
            quiet_hours: None,
            conversation_rules,
        };

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let decision = policy.decide(
            &candidate,
            &fabushi_messaging_core::ActorId::new(local_human_id),
            now_ms,
            12 * 60,
        );

        if decision.deliver {
            Ok(serde_json::json!({
                "decision": "Notify",
                "deliver": true,
                "reason": serde_json::Value::Null,
                "showPreview": decision.show_preview,
                "sound": decision.sound,
                "importance": decision.importance,
                "Notify": {}
            }))
        } else {
            let reason_str = match decision.reason.as_str() {
                "topic-muted" => "TopicMuted",
                "conversation-muted" => "ConversationMuted",
                "quiet-hours" => "QuietHours",
                other => other,
            };
            Ok(serde_json::json!({
                "decision": "Suppressed",
                "deliver": false,
                "reason": reason_str,
                "showPreview": false,
                "sound": serde_json::Value::Null,
                "importance": decision.importance,
                "Suppressed": {
                    "reason": reason_str
                }
            }))
        }
    }

}
