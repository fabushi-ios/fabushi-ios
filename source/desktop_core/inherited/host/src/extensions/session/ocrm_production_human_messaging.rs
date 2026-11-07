use super::agent_db_transcript_pages::{TranscriptPage, TranscriptWindowQuery};
use super::native_messaging::{FabushiRemoteHumanAttachment, fabushi_identity_text};
use super::production::ProductionSessionWorkers;

impl ProductionSessionWorkers {
    pub fn append_human_message(
        &self,
        conversation_id: &str,
        text: &str,
        client_nonce: &str,
        composed_at_ms: Option<f64>,
        reply_to_id: Option<&str>,
        attachments: &[serde_json::Value],
    ) -> Result<serde_json::Value, String> {
        self.append_human_message_with_context(
            conversation_id,
            text,
            client_nonce,
            composed_at_ms,
            reply_to_id,
            attachments,
            None,
        )
    }

    pub(crate) fn append_human_message_with_context(
        &self,
        conversation_id: &str,
        text: &str,
        client_nonce: &str,
        composed_at_ms: Option<f64>,
        reply_to_id: Option<&str>,
        attachments: &[serde_json::Value],
        forward_context: Option<&serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        let sender_id = self.local_human_id()?;
        let text = text.trim();
        let client_nonce = client_nonce.trim();
        if client_nonce.is_empty() {
            return Err("sendHumanMessage requires clientNonce".into());
        }
        if text.is_empty() && attachments.is_empty() {
            return Err("sendHumanMessage requires text or attachments".into());
        }
        if client_nonce.len() > 200 {
            return Err("sendHumanMessage clientNonce is too long".into());
        }
        let owner = self.open_human_conversation_db_owner(conversation_id)?;
        let metadata = owner.read_metadata().map_err(|error| error.to_string())?;
        let is_participant = metadata
            .get("participantIds")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(sender_id)));
        if !is_participant {
            return Err("local Human identity is not a conversation participant".into());
        }
        let existing = owner
            .get_transcript_entries()
            .map_err(|error| error.to_string())?;
        let normalized_reply = reply_to_id
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if let Some(reply_id) = normalized_reply {
            if !existing.iter().any(|entry| {
                entry.get("id").and_then(serde_json::Value::as_str) == Some(reply_id)
            }) {
                return Err("sendHumanMessage reply target is not in this Human conversation".into());
            }
        }
        let normalized_attachments = attachments
            .iter()
            .map(|attachment| {
                let object = attachment
                    .as_object()
                    .ok_or_else(|| "sendHumanMessage attachment must be an object".to_string())?;
                let path = object
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| "sendHumanMessage attachment requires path".to_string())?;
                let name = object
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| "sendHumanMessage attachment requires name".to_string())?;
                Ok(serde_json::json!({ "path": path, "name": name }))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let timestamp_ms = composed_at_ms.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as f64
        });
        let mut existing_pending: Option<serde_json::Value> = None;
        if let Some(entry) = existing.iter().find(|entry| {
            entry.get("clientNonce").and_then(serde_json::Value::as_str) == Some(client_nonce)
        }) {
            let same_sender = entry.get("authorId").and_then(serde_json::Value::as_str) == Some(sender_id);
            let same_text = entry.get("content").and_then(serde_json::Value::as_str) == Some(text);
            let same_reply = entry.get("replyToId").and_then(serde_json::Value::as_str) == normalized_reply;
            let same_attachments = match entry.get("attachments").and_then(serde_json::Value::as_array) {
                Some(values) => values.len() == normalized_attachments.len()
                    && values.iter().zip(normalized_attachments.iter()).all(|(current, requested)| {
                        current.get("name") == requested.get("name")
                            && (current.get("path") == requested.get("path") || current.get("resourceId").is_some())
                    }),
                None => normalized_attachments.is_empty(),
            };
            let same_forward_context = entry.get("forwardContext") == forward_context;
            if !(same_sender && same_text && same_reply && same_attachments && same_forward_context) {
                return Err("sendHumanMessage clientNonce already identifies different content or forward context".into());
            }
            if entry.get("remoteMessageId").is_some()
                || entry.get("delivery").and_then(serde_json::Value::as_str) == Some("sent")
            {
                return Ok(entry.clone());
            }
            if !matches!(
                entry.get("delivery").and_then(serde_json::Value::as_str),
                Some("pending" | "dispatching")
            ) {
                return Err("sendHumanMessage clientNonce is not in a retryable settlement state".into());
            }
            existing_pending = Some(entry.clone());
        }

        if let Some(client) = self.shipping_native_messaging()? {
            let local_entry_id = format!("human-message:{client_nonce}");
            let mut pending_entry = existing_pending.unwrap_or_else(|| {
                let mut entry = serde_json::json!({
                    "id": local_entry_id.clone(),
                    "kind": "message",
                    "role": "user",
                    "authorKind": "human",
                    "authorId": sender_id,
                    "content": text,
                    "clientNonce": client_nonce,
                    "composedAtMs": composed_at_ms,
                    "timestampMs": timestamp_ms,
                    "delivery": "pending",
                });
                let object = entry.as_object_mut().expect("Human pending message must be an object");
                if let Some(reply_id) = normalized_reply { object.insert("replyToId".into(), serde_json::json!(reply_id)); }
                if !normalized_attachments.is_empty() { object.insert("attachments".into(), serde_json::Value::Array(normalized_attachments.clone())); }
                if let Some(context) = forward_context { object.insert("forwardContext".into(), context.clone()); }
                entry
            });
            if !existing.iter().any(|entry| entry.get("id") == pending_entry.get("id"))
                && !owner.append_transcript_entry(&pending_entry).map_err(|error| error.to_string())?
            {
                return Err("Human pending send could not be durably recorded before dispatch".into());
            }

            let dispatch_result = (|| -> Result<serde_json::Value, String> {
                let peer_human_id = self.peer_human_id_from_metadata(&metadata)?;
                let remote_reply_to_id = normalized_reply
                    .map(|reply_id| {
                        owner
                            .get_transcript_entries()
                            .map_err(|error| error.to_string())?
                            .into_iter()
                            .find(|entry| entry.get("id").and_then(serde_json::Value::as_str) == Some(reply_id))
                            .and_then(|entry| entry.get("remoteMessageId").cloned())
                            .map(|value| fabushi_identity_text(&value))
                            .transpose()?
                            .ok_or_else(|| "Fabushi Human reply target has no canonical remote message identity".to_string())
                    })
                    .transpose()?;

                let mut uploaded = pending_entry
                    .get("remoteDispatchAttachments")
                    .cloned()
                    .map(serde_json::from_value::<Vec<FabushiRemoteHumanAttachment>>)
                    .transpose()
                    .map_err(|error| format!("persisted Human dispatch attachments are invalid: {error}"))?
                    .filter(|values| {
                        values.len() == normalized_attachments.len()
                            && values.iter().zip(normalized_attachments.iter()).all(|(remote, local)| {
                                local.get("name").and_then(serde_json::Value::as_str) == Some(remote.name.as_str())
                            })
                    })
                    .unwrap_or_default();
                if uploaded.is_empty() && !normalized_attachments.is_empty() {
                    uploaded = Vec::with_capacity(normalized_attachments.len());
                    for attachment in &normalized_attachments {
                        let path = attachment.get("path").and_then(serde_json::Value::as_str)
                            .ok_or_else(|| "sendHumanMessage attachment requires path".to_string())?;
                        let name = attachment.get("name").and_then(serde_json::Value::as_str)
                            .ok_or_else(|| "sendHumanMessage attachment requires name".to_string())?;
                        uploaded.push(client.upload_direct_message_resource(std::path::Path::new(path), name)?);
                    }
                }
                if let Some(object) = pending_entry.as_object_mut() {
                    object.insert("delivery".into(), serde_json::json!("dispatching"));
                    object.remove("lastDispatchError");
                    object.remove("lastDispatchFailedAtMs");
                    if !uploaded.is_empty() {
                        object.insert("remoteDispatchAttachments".into(), serde_json::to_value(&uploaded)
                            .map_err(|error| format!("Human dispatch attachments could not be persisted: {error}"))?);
                    }
                }
                owner.update_transcript_entry(&local_entry_id, &pending_entry)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| "Human pending send disappeared before dispatch".to_string())?;

                let remote = client.send_direct_message(&peer_human_id, text, client_nonce, remote_reply_to_id.as_deref(), &uploaded)?;
                let remote_sender = fabushi_identity_text(&remote.sender_user_id)?;
                let remote_recipient = fabushi_identity_text(&remote.recipient_user_id)?;
                let remote_reply_matches = match (&remote_reply_to_id, &remote.reply_to_message_id) {
                    (None, None) => true,
                    (Some(expected), Some(actual)) => fabushi_identity_text(actual).is_ok_and(|actual| actual == *expected),
                    _ => false,
                };
                let remote_resources = remote.attachments.iter().map(|attachment| attachment.resource_id.as_str()).collect::<Vec<_>>();
                let uploaded_resources = uploaded.iter().map(|attachment| attachment.resource_id.as_str()).collect::<Vec<_>>();
                if remote_sender != sender_id
                    || remote_recipient != peer_human_id
                    || remote.text.trim() != text
                    || remote.client_request_id.as_deref() != Some(client_nonce)
                    || !remote_reply_matches
                    || remote_resources != uploaded_resources
                {
                    return Err("Fabushi Human message backend returned mismatched persisted content".into());
                }
                self.materialize_remote_human_message(&owner, &remote)
            })();

            if let Err(error) = dispatch_result.as_ref() {
                if let Ok(entries) = owner.get_transcript_entries() {
                    if let Some(mut pending) = entries.into_iter().find(|entry| entry.get("id").and_then(serde_json::Value::as_str) == Some(local_entry_id.as_str())) {
                        if let Some(object) = pending.as_object_mut() {
                            object.insert("delivery".into(), serde_json::json!("pending"));
                            object.insert("lastDispatchError".into(), serde_json::json!(error));
                            object.insert("lastDispatchFailedAtMs".into(), serde_json::json!(
                                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as f64
                            ));
                        }
                        let _ = owner.update_transcript_entry(&local_entry_id, &pending);
                    }
                }
            }
            return dispatch_result;
        }

        let mut entry = serde_json::json!({
            "id": format!("human-message:{client_nonce}"),
            "kind": "message",
            "role": "user",
            "authorKind": "human",
            "authorId": sender_id,
            "content": text,
            "clientNonce": client_nonce,
            "composedAtMs": composed_at_ms,
            "timestampMs": timestamp_ms,
            "delivery": "sent",
        });
        let object = entry
            .as_object_mut()
            .ok_or_else(|| "native Human message projection failed".to_string())?;
        if let Some(reply_id) = normalized_reply {
            object.insert("replyToId".into(), serde_json::json!(reply_id));
        }
        if !normalized_attachments.is_empty() {
            object.insert(
                "attachments".into(),
                serde_json::Value::Array(normalized_attachments.clone()),
            );
        }
        if let Some(context) = forward_context {
            object.insert("forwardContext".into(), context.clone());
        }
        if !owner
            .append_transcript_entry(&entry)
            .map_err(|error| error.to_string())?
        {
            let replay = owner
                .get_transcript_entries()
                .map_err(|error| error.to_string())?
                .into_iter()
                .find(|candidate| candidate.get("id") == entry.get("id"))
                .ok_or_else(|| "native Human message was not durably appended".to_string())?;
            if replay == entry {
                return Ok(replay);
            }
            return Err("sendHumanMessage clientNonce already identifies different content".into());
        }
        if !owner
            .set_metadata("lastActivityAt", serde_json::json!(timestamp_ms))
            .map_err(|error| error.to_string())?
        {
            return Err("native Human message activity metadata was not durably updated".into());
        }
        Ok(entry)
    }

    pub fn append_human_agent_message(
        &self,
        conversation_id: &str,
        agent_id: &str,
        stream_id: &str,
        session_key: Option<&str>,
        content: &str,
        completed_at_ms: f64,
    ) -> Result<serde_json::Value, String> {
        let local_human_id = self.local_human_id()?;
        let conversation_id = conversation_id.trim();
        let agent_id = agent_id.trim();
        let stream_id = stream_id.trim();
        let session_key = match session_key {
            Some(value) => {
                let value = value.trim();
                if value.is_empty() {
                    return Err("Human handoff Agent result session key must be non-empty".into());
                }
                Some(value)
            }
            None => None,
        };
        let content = content.trim();
        if conversation_id.is_empty()
            || agent_id.is_empty()
            || stream_id.is_empty()
            || content.is_empty()
        {
            return Err(
                "Human handoff Agent result requires conversationId, agentId, streamId, and content"
                    .into(),
            );
        }
        let owner = self.open_human_conversation_db_owner(conversation_id)?;
        let metadata = owner.read_metadata().map_err(|error| error.to_string())?;
        let is_participant = metadata
            .get("participantIds")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(local_human_id)));
        if !is_participant {
            return Err("local Human identity is not a conversation participant".into());
        }

        let entry_id = format!("human-agent-message:{stream_id}");
        let existing = owner
            .get_transcript_entries()
            .map_err(|error| error.to_string())?;
        if let Some(entry) = existing.iter().find(|entry| {
            entry.get("id").and_then(serde_json::Value::as_str) == Some(entry_id.as_str())
        }) {
            let same_agent =
                entry.get("authorId").and_then(serde_json::Value::as_str) == Some(agent_id);
            let same_content =
                entry.get("content").and_then(serde_json::Value::as_str) == Some(content);
            let same_stream =
                entry.get("sourceStreamId").and_then(serde_json::Value::as_str) == Some(stream_id);
            if same_agent && same_content && same_stream {
                if let Some(expected_session_key) = session_key {
                    if let Some(actual_session_key) = entry
                        .get("sessionKey")
                        .and_then(serde_json::Value::as_str)
                    {
                        if actual_session_key != expected_session_key {
                            return Err(
                                "Human handoff stream already identifies a different session key"
                                    .into(),
                            );
                        }
                    } else {
                        let mut enriched = entry.clone();
                        enriched["sessionKey"] = serde_json::Value::String(
                            expected_session_key.to_string(),
                        );
                        return owner
                            .update_transcript_entry_if_session_admitted(
                                expected_session_key,
                                &entry_id,
                                &enriched,
                            )
                            .map_err(|error| error.to_string())?
                            .ok_or_else(|| {
                                "Human handoff Agent result disappeared during provenance settlement"
                                    .to_string()
                            });
                    }
                }
                return Ok(entry.clone());
            }
            return Err("Human handoff stream already identifies different Agent content".into());
        }

        let mut entry = serde_json::json!({
            "id": entry_id,
            "kind": "message",
            "role": "assistant",
            "authorKind": "agent",
            "authorId": agent_id,
            "authorName": agent_id,
            "content": content,
            "sourceStreamId": stream_id,
            "timestampMs": completed_at_ms,
            "delivery": "sent",
        });
        if let Some(session_key) = session_key {
            entry["sessionKey"] = serde_json::Value::String(session_key.to_string());
        }
        let inserted = if let Some(session_key) = session_key {
            owner
                .append_transcript_entries_if_session_admitted(session_key, &[entry.clone()])
                .map_err(|error| error.to_string())?
                == 1
        } else {
            owner
                .append_transcript_entry(&entry)
                .map_err(|error| error.to_string())?
        };
        if !inserted {
            let replay = owner
                .get_transcript_entries()
                .map_err(|error| error.to_string())?
                .into_iter()
                .find(|candidate| candidate.get("id") == entry.get("id"))
                .ok_or_else(|| "Human handoff Agent result was not durably appended".to_string())?;
            let same_agent =
                replay.get("authorId").and_then(serde_json::Value::as_str) == Some(agent_id);
            let same_content =
                replay.get("content").and_then(serde_json::Value::as_str) == Some(content);
            let same_stream =
                replay.get("sourceStreamId").and_then(serde_json::Value::as_str) == Some(stream_id);
            if same_agent && same_content && same_stream {
                if let Some(expected_session_key) = session_key {
                    if let Some(actual_session_key) = replay
                        .get("sessionKey")
                        .and_then(serde_json::Value::as_str)
                    {
                        if actual_session_key != expected_session_key {
                            return Err(
                                "Human handoff stream already identifies a different session key"
                                    .into(),
                            );
                        }
                    } else {
                        let mut enriched = replay.clone();
                        enriched["sessionKey"] = serde_json::Value::String(
                            expected_session_key.to_string(),
                        );
                        return owner
                            .update_transcript_entry_if_session_admitted(
                                expected_session_key,
                                &entry_id,
                                &enriched,
                            )
                            .map_err(|error| error.to_string())?
                            .ok_or_else(|| {
                                "Human handoff Agent result disappeared during replay provenance settlement"
                                    .to_string()
                            });
                    }
                }
                return Ok(replay);
            }
            return Err("Human handoff stream already identifies different Agent content".into());
        }
        if !owner
            .set_metadata("lastActivityAt", serde_json::json!(completed_at_ms))
            .map_err(|error| error.to_string())?
        {
            return Err("Human handoff Agent activity metadata was not durably updated".into());
        }
        Ok(entry)
    }

    pub fn toggle_human_message_reaction(
        &self,
        conversation_id: &str,
        entry_id: &str,
        emoji: &str,
    ) -> Result<serde_json::Value, String> {
        let emoji = emoji.trim();
        if emoji.is_empty() {
            return Err("reactHumanMessage requires emoji".into());
        }
        let owner = self.open_human_conversation_db_owner(conversation_id)?;
        let metadata = owner.read_metadata().map_err(|error| error.to_string())?;
        if !self.metadata_has_local_human(&metadata)? {
            return Err("local Human identity is not a conversation participant".into());
        }
        let entries = owner
            .get_transcript_entries()
            .map_err(|error| error.to_string())?;
        let current = entries
            .iter()
            .find(|entry| entry.get("id").and_then(serde_json::Value::as_str) == Some(entry_id))
            .cloned()
            .ok_or_else(|| "Human reaction target is not in this conversation".to_string())?;
        let remote_message_id = current
            .get("remoteMessageId")
            .map(fabushi_identity_text)
            .transpose()?
            .ok_or_else(|| "Human reaction target has no canonical remote message identity".to_string())?;
        let currently_active = current
            .get("remoteReactions")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|reactions| {
                reactions.iter().any(|reaction| {
                    reaction.get("emoji").and_then(serde_json::Value::as_str) == Some(emoji)
                        && reaction
                            .get("reactedByMe")
                            .and_then(serde_json::Value::as_bool)
                            == Some(true)
                })
            });
        let client = self
            .shipping_native_messaging()?
            .ok_or_else(|| "Fabushi Human reactions require the shipping messaging backend".to_string())?;
        let remote_reactions =
            client.set_direct_message_reaction(&remote_message_id, emoji, !currently_active)?;
        let peer_human_id = self.peer_human_id_from_metadata(&metadata)?;
        let mut projected_reactions = Vec::new();
        for reaction in &remote_reactions {
            if reaction.reacted_by_me {
                projected_reactions.push(serde_json::json!({
                    "emoji": reaction.emoji,
                    "by": "me",
                }));
            }
            let peer_reaction_count = reaction
                .count
                .saturating_sub(if reaction.reacted_by_me { 1 } else { 0 });
            for _ in 0..peer_reaction_count {
                projected_reactions.push(serde_json::json!({
                    "emoji": reaction.emoji,
                    "by": peer_human_id,
                }));
            }
        }
        let mut next = current;
        let object = next
            .as_object_mut()
            .ok_or_else(|| "Human reaction target is not a message object".to_string())?;
        object.insert(
            "remoteReactions".into(),
            serde_json::to_value(&remote_reactions).map_err(|error| {
                format!("remote Human reactions could not be projected: {error}")
            })?,
        );
        object.insert(
            "reactions".into(),
            serde_json::Value::Array(projected_reactions),
        );
        let updated = if let Some(session_key) = next
            .get("sessionKey")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            owner
                .update_transcript_entry_if_session_admitted(session_key, entry_id, &next)
                .map_err(|error| error.to_string())?
        } else {
            owner
                .update_transcript_entry(entry_id, &next)
                .map_err(|error| error.to_string())?
        };
        updated.ok_or_else(|| "Human reaction target disappeared before durable settlement".to_string())
    }

    pub fn search_human_messages(
        &self,
        conversation_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<serde_json::Value>, String> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Err("searchHumanMessages requires a non-empty query".into());
        }
        let transcript = self.read_human_conversation_transcript(conversation_id)?;
        let limit = limit.clamp(1, 200);
        let mut matches = Vec::new();
        for entry in transcript.into_iter().rev() {
            let content_match = entry
                .get("content")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|value| value.to_lowercase().contains(&query));
            let author_match = entry
                .get("authorName")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|value| value.to_lowercase().contains(&query));
            let attachment_match = entry
                .get("attachments")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|attachments| {
                    attachments.iter().any(|attachment| {
                        attachment
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|value| value.to_lowercase().contains(&query))
                    })
                });
            if content_match || author_match || attachment_match {
                matches.push(entry);
                if matches.len() >= limit {
                    break;
                }
            }
        }
        Ok(matches)
    }

    pub fn read_human_conversation_transcript(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<serde_json::Value>, String> {
        let owner = self.open_human_conversation_db_owner(conversation_id)?;
        let metadata = owner.read_metadata().map_err(|error| error.to_string())?;
        if !self.metadata_has_local_human(&metadata)? {
            return Err("local Human identity is not a conversation participant".into());
        }
        owner
            .get_transcript_entries()
            .map_err(|error| error.to_string())
    }

    pub fn read_human_conversation_transcript_tail(
        &self,
        conversation_id: &str,
        query: TranscriptWindowQuery,
    ) -> Result<TranscriptPage, String> {
        let owner = self.open_human_conversation_db_owner(conversation_id)?;
        let metadata = owner.read_metadata().map_err(|error| error.to_string())?;
        if !self.metadata_has_local_human(&metadata)? {
            return Err("local Human identity is not a conversation participant".into());
        }
        owner
            .get_transcript_tail(query)
            .map_err(|error| error.to_string())
    }
}
