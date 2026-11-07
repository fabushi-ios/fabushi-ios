//! Telegram / TDRP Typing Indicator, Media Preflight, and Media Cache.

use super::*;

impl ProductionSessionWorkers {
    pub fn publish_conversation_typing(
        &self,
        conversation_id: &str,
        topic_id: Option<&str>,
        action: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        let conversation_id = conversation_id.trim();
        if conversation_id.is_empty() {
            return Err("publishConversationTyping requires conversationId".into());
        }
        let local_human_id = self.local_human_id()?;
        let action_str = action.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("typing");
        let topic_id_trimmed = topic_id.map(str::trim).filter(|s| !s.is_empty());

        let owner = self.open_human_conversation_db_owner(conversation_id)?;
        let metadata = owner.read_metadata().map_err(|e| e.to_string())?;

        let kind_str = metadata.get("conversationKind").and_then(serde_json::Value::as_str).unwrap_or("human");
        let (conv_kind, member_opt, topic_opt) = if kind_str == "group" || kind_str == "channel" || topic_id_trimmed.is_some() {
            let member = owner
                .get_conversation_member(local_human_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "caller is not a conversation member".to_string())?;
            let c_kind = match kind_str {
                "channel" => fabushi_messaging_core::ConversationKind::Channel,
                _ => fabushi_messaging_core::ConversationKind::Group,
            };
            let t_opt = if let Some(tid) = topic_id_trimmed {
                let topic = owner.get_conversation_topic(tid).map_err(|e| e.to_string())?;
                if topic.is_none() {
                    return Err(format!("topic {tid} does not exist"));
                }
                topic
            } else {
                None
            };
            (c_kind, Some(member), t_opt)
        } else {
            let is_participant = metadata
                .get("participantIds")
                .and_then(serde_json::Value::as_array)
                .map(|arr| arr.iter().any(|p| p.as_str() == Some(local_human_id)))
                .unwrap_or(false);
            if !is_participant {
                return Err("caller is not a conversation participant".into());
            }
            (fabushi_messaging_core::ConversationKind::Direct, None, None)
        };

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        if let Some(ref member) = member_opt {
            let content_flags = fabushi_messaging_core::CommunitySendContentFlags {
                has_media: false,
                has_poll: false,
                has_other: false,
                has_inline: false,
                has_game: false,
            };
            let policy_input = fabushi_messaging_core::CommunitySendPolicyInput {
                conversation_kind: conv_kind,
                member: Some(member),
                slow_mode_seconds: None,
                last_sender_message_at_ms: None,
                now_ms,
                target_topic: topic_opt.as_ref(),
                topic_requested: topic_id_trimmed.is_some(),
                content: content_flags,
            };
            if let Err(denial) = fabushi_messaging_core::evaluate_community_send_policy(&policy_input) {
                return Err(format!("publishConversationTyping rejected by community send policy: {denial:?}"));
            }
        }

        let topic_closed = topic_opt.as_ref().map(|t| t.closed).unwrap_or(false);
        let typing_state = fabushi_messaging_core::evaluate_typing_publication(
            fabushi_messaging_core::ConversationId::new(conversation_id),
            conv_kind,
            fabushi_messaging_core::ActorId::new(local_human_id),
            true,
            member_opt.as_ref(),
            topic_id_trimmed.map(str::to_string),
            topic_closed,
            action_str,
            now_ms,
            Some(5_000),
        ).map_err(|e| format!("publishConversationTyping rejected by realtime policy: {e:?}"))?;

        // Throttling logic (3000ms window, 5000ms TTL)
        let mut typing_states: Vec<fabushi_messaging_core::TypingState> = metadata
            .get("typingStates")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let actor_id_str = local_human_id;
        let existing_index = typing_states.iter().position(|s| {
            s.actor_id.0 == actor_id_str && s.topic_id.as_deref() == topic_id_trimmed
        });

        const TYPING_TTL_MS: i64 = 5_000;
        const TYPING_THROTTLE_WINDOW_MS: i64 = 3_000;

        let (decision_str, expires_at_ms, next_allowed_at_ms) = match existing_index {
            Some(idx) => {
                let existing = &typing_states[idx];
                let last_published_at_ms = existing.expires_at_ms - TYPING_TTL_MS;
                if now_ms < last_published_at_ms + TYPING_THROTTLE_WINDOW_MS {
                    // Throttled!
                    let next_allowed = last_published_at_ms + TYPING_THROTTLE_WINDOW_MS;
                    ("throttled", existing.expires_at_ms, next_allowed)
                } else {
                    // Allowed to re-publish!
                    let exp = now_ms + TYPING_TTL_MS;
                    let next_allowed = now_ms + TYPING_THROTTLE_WINDOW_MS;
                    typing_states[idx] = typing_state;
                    ("published", exp, next_allowed)
                }
            }
            None => {
                let exp = now_ms + TYPING_TTL_MS;
                let next_allowed = now_ms + TYPING_THROTTLE_WINDOW_MS;
                typing_states.push(typing_state);
                ("published", exp, next_allowed)
            }
        };

        if decision_str == "published" {
            // Prune expired states
            typing_states.retain(|s| s.expires_at_ms > now_ms);
            let val = serde_json::to_value(&typing_states).map_err(|e| e.to_string())?;
            owner.set_metadata("typingStates", val).map_err(|e| e.to_string())?;
        }

        Ok(serde_json::json!({
            "decision": decision_str,
            "actorId": local_human_id,
            "conversationId": conversation_id,
            "topicId": topic_id_trimmed,
            "expiresAtMs": expires_at_ms,
            "nextAllowedAtMs": next_allowed_at_ms,
        }))
    }

    pub fn validate_media_attachment_preflight(
        &self,
        file_name: &str,
        mime_type: &str,
        size_bytes: u64,
        width: Option<u32>,
        height: Option<u32>,
    ) -> Result<serde_json::Value, String> {
        let trimmed_mime = mime_type.trim();
        if trimmed_mime.is_empty() {
            return Err("media attachment mime type cannot be empty".into());
        }
        fabushi_messaging_core::validate_desktop_media_attachment_meta(file_name, size_bytes, None)
            .map_err(|e| e.to_string())?;

        Ok(serde_json::json!({
            "valid": true,
            "fileName": file_name.trim(),
            "mimeType": trimmed_mime,
            "sizeBytes": size_bytes,
            "width": width,
            "height": height,
        }))
    }


    pub fn cache_conversation_media(
        &self,
        conversation_id: &str,
        topic_id: Option<&str>,
        key: &str,
        size_bytes: u64,
        mime_type: &str,
    ) -> Result<serde_json::Value, String> {
        let conversation_id = conversation_id.trim();
        if conversation_id.is_empty() {
            return Err("conversationId is empty".into());
        }
        let key = key.trim();
        if key.is_empty() {
            return Err("media key is empty".into());
        }
        let now_ms = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()) as i64;
        let media = fabushi_messaging_core::MediaRef {
            id: key.to_string(),
            file_name: None,
            mime_type: if mime_type.trim().is_empty() {
                None
            } else {
                Some(mime_type.to_string())
            },
            size_bytes: Some(size_bytes),
            width: None,
            height: None,
            duration_ms: None,
            thumbnail_id: None,
            local_path: Some(key.to_string()),
            remote_url: None,
            content_hash: None,
        };
        let mut cache = self
            .media_cache
            .lock()
            .map_err(|_| "media_cache lock poisoned".to_string())?;
        let evicted = cache
            .insert_scoped(
                media,
                key,
                size_bytes,
                now_ms,
                false,
                Some(conversation_id.to_string()),
                topic_id.map(|t| t.to_string()),
            )
            .map_err(|err| err.to_string())?;
        let conv_bytes = cache.total_bytes_for_conversation(conversation_id);
        let total_bytes = cache.total_bytes();
        Ok(serde_json::json!({
            "cached": true,
            "key": key,
            "sizeBytes": size_bytes,
            "conversationId": conversation_id,
            "topicId": topic_id,
            "evictedCount": evicted.len(),
            "totalBytesForConversation": conv_bytes,
            "totalBytes": total_bytes,
            "maxBytes": cache.max_bytes(),
        }))
    }

    pub fn get_conversation_media_cache_stats(
        &self,
        conversation_id: &str,
    ) -> Result<serde_json::Value, String> {
        let conversation_id = conversation_id.trim();
        if conversation_id.is_empty() {
            return Err("conversationId is empty".into());
        }
        let cache = self
            .media_cache
            .lock()
            .map_err(|_| "media_cache lock poisoned".to_string())?;
        let conv_bytes = cache.total_bytes_for_conversation(conversation_id);
        let entry_count = cache
            .entries()
            .values()
            .filter(|e| e.conversation_id.as_deref() == Some(conversation_id))
            .count();
        Ok(serde_json::json!({
            "conversationId": conversation_id,
            "totalBytes": conv_bytes,
            "totalBytesForConversation": conv_bytes,
            "entryCount": entry_count,
            "cacheTotalBytes": cache.total_bytes(),
            "cacheMaxBytes": cache.max_bytes(),
        }))
    }

    pub fn clear_conversation_media_cache(
        &self,
        conversation_id: &str,
        topic_id: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        let conversation_id = conversation_id.trim();
        if conversation_id.is_empty() {
            return Err("conversationId is empty".into());
        }
        let mut cache = self
            .media_cache
            .lock()
            .map_err(|_| "media_cache lock poisoned".to_string())?;
        let cleared_entries = if let Some(topic) = topic_id {
            cache.clear_topic(conversation_id, topic, true)
        } else {
            cache.clear_conversation(conversation_id, true)
        };
        let cleared_bytes = cleared_entries
            .iter()
            .fold(0u64, |acc, e| acc.saturating_add(e.size_bytes));
        let remaining_bytes = cache.total_bytes_for_conversation(conversation_id);
        Ok(serde_json::json!({
            "cleared": true,
            "clearedCount": cleared_entries.len(),
            "clearedBytes": cleared_bytes,
            "conversationId": conversation_id,
            "topicId": topic_id,
            "remainingBytesForConversation": remaining_bytes,
            "cacheTotalBytes": cache.total_bytes(),
        }))
    }

    pub fn media_cache(&self) -> Arc<Mutex<fabushi_messaging_core::MediaCache>> {
        Arc::clone(&self.media_cache)
    }

}
