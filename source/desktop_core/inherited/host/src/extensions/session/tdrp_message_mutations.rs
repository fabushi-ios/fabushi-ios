//! Telegram / TDRP Message Mutations, Topic Dispatches, and Intent Processing.

use super::*;

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
        self.append_human_message_with_queue_origin(
            conversation_id, text, client_nonce, composed_at_ms, reply_to_id, attachments, false,
        )
    }

    pub fn append_human_message_with_topic(
        &self,
        conversation_id: &str,
        text: &str,
        client_nonce: &str,
        composed_at_ms: Option<f64>,
        reply_to_id: Option<&str>,
        attachments: &[serde_json::Value],
        topic_id: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        self.append_human_message_with_topic_and_queue_origin(
            conversation_id,
            text,
            client_nonce,
            composed_at_ms,
            reply_to_id,
            attachments,
            topic_id,
            false,
        )
    }

    pub fn append_human_message_with_topic_and_queue_origin(
        &self,
        conversation_id: &str,
        text: &str,
        client_nonce: &str,
        composed_at_ms: Option<f64>,
        reply_to_id: Option<&str>,
        attachments: &[serde_json::Value],
        topic_id: Option<&str>,
        queued_offline: bool,
    ) -> Result<serde_json::Value, String> {
        self.append_human_message_with_topic_and_context(
            conversation_id,
            text,
            client_nonce,
            composed_at_ms,
            reply_to_id,
            attachments,
            None,
            topic_id,
            None,
            queued_offline,
        )
    }

    pub fn append_human_message_with_queue_origin(
        &self,
        conversation_id: &str,
        text: &str,
        client_nonce: &str,
        composed_at_ms: Option<f64>,
        reply_to_id: Option<&str>,
        attachments: &[serde_json::Value],
        queued_offline: bool,
    ) -> Result<serde_json::Value, String> {
        self.append_human_message_with_context(
            conversation_id,
            text,
            client_nonce,
            composed_at_ms,
            reply_to_id,
            attachments,
            None,
            None,
            queued_offline,
        )
    }

    pub fn append_human_message_with_context(
        &self,
        conversation_id: &str,
        text: &str,
        client_nonce: &str,
        composed_at_ms: Option<f64>,
        reply_to_id: Option<&str>,
        attachments: &[serde_json::Value],
        forward_context: Option<&serde_json::Value>,
        held_conversation_guard: Option<&std::sync::MutexGuard<'_, ()>>,
        queued_offline: bool,
    ) -> Result<serde_json::Value, String> {
        self.append_human_message_with_topic_and_context(
            conversation_id,
            text,
            client_nonce,
            composed_at_ms,
            reply_to_id,
            attachments,
            forward_context,
            None,
            held_conversation_guard,
            queued_offline,
        )
    }

    pub fn append_human_message_with_topic_and_context(
        &self,
        conversation_id: &str,
        text: &str,
        client_nonce: &str,
        composed_at_ms: Option<f64>,
        reply_to_id: Option<&str>,
        attachments: &[serde_json::Value],
        forward_context: Option<&serde_json::Value>,
        topic_id: Option<&str>,
        held_conversation_guard: Option<&std::sync::MutexGuard<'_, ()>>,
        queued_offline: bool,
    ) -> Result<serde_json::Value, String> {
        self.append_human_message_from_intent(
            conversation_id,
            text,
            client_nonce,
            composed_at_ms,
            reply_to_id,
            attachments,
            forward_context,
            topic_id,
            held_conversation_guard,
            false,
            queued_offline,
        )
    }

    pub(crate) fn append_human_message_from_intent(
        &self,
        conversation_id: &str,
        text: &str,
        client_nonce: &str,
        composed_at_ms: Option<f64>,
        reply_to_id: Option<&str>,
        attachments: &[serde_json::Value],
        forward_context: Option<&serde_json::Value>,
        topic_id: Option<&str>,
        held_conversation_guard: Option<&std::sync::MutexGuard<'_, ()>>,
        require_existing: bool,
        queued_offline: bool,
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
        if queued_offline && !composed_at_ms.is_some_and(f64::is_finite) {
            return Err("Human offline queue origin requires a finite composedAtMs".into());
        }
        let serial = self.human_message_lock(conversation_id)?;
        let mut staging_guard = if held_conversation_guard.is_none() {
            Some(serial.lock().map_err(|_| "Human conversation serial lock poisoned".to_string())?)
        } else {
            None
        };
        let owner = self.open_human_conversation_db_owner(conversation_id)?;
        let metadata = owner.read_metadata().map_err(|error| error.to_string())?;
        let is_participant = metadata
            .get("participantIds")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(sender_id)));
        if !is_participant {
            return Err("local Human identity is not a conversation participant".into());
        }
        if require_existing {
            Self::validate_human_retry_conversation(&metadata, conversation_id, sender_id)?;
        }
        let existing = owner
            .get_transcript_entries()
            .map_err(|error| error.to_string())?;
        if require_existing {
            let current = Self::human_retry_entry(&existing, sender_id, client_nonce)?;
            if current.get("delivery").and_then(serde_json::Value::as_str) == Some("sent") {
                return Ok(current.clone());
            }
        }
        let normalized_reply_id = Self::normalize_human_reply_identity(&existing, reply_to_id, sender_id)?;
        let normalized_reply = normalized_reply_id.as_deref();
        if !attachments.is_empty() {
            fabushi_messaging_core::validate_desktop_media_batch_count(attachments.len(), None)
                .map_err(|e| format!("sendHumanMessage attachment preflight failed: {e}"))?;
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
                let mut normalized = serde_json::json!({ "path": path, "name": name });
                if let Some(mime_type) = object
                    .get("mimeType")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    normalized
                        .as_object_mut()
                        .expect("attachment object")
                        .insert("mimeType".into(), serde_json::Value::String(mime_type.into()));
                }
                for dim in ["width", "height"] {
                    if let Some(value) = object
                        .get(dim)
                        .and_then(|value| {
                            value.as_i64().or_else(|| {
                                value
                                    .as_f64()
                                    .filter(|number| number.is_finite())
                                    .map(|number| number.round() as i64)
                            })
                        })
                        .filter(|value| *value > 0)
                    {
                        normalized
                            .as_object_mut()
                            .expect("attachment object")
                            .insert(dim.into(), serde_json::json!(value));
                    }
                }
                if forward_context.is_some() {
                    if let Some(snapshot) = object.get("forwardSnapshot") {
                        normalized.as_object_mut().expect("attachment object")
                            .insert("forwardSnapshot".into(), snapshot.clone());
                    }
                }
                Ok(normalized)
            })
            .collect::<Result<Vec<_>, String>>()?;
        let timestamp_ms = composed_at_ms.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as f64
        });
        if !timestamp_ms.is_finite() {
            return Err("Human message activity timestamp must be finite".into());
        }
        let mut existing_pending: Option<serde_json::Value> = None;
        if let Some(entry) = existing.iter().find(|entry| {
            entry.get("authorId").and_then(serde_json::Value::as_str) == Some(sender_id)
                && entry.get("clientNonce").and_then(serde_json::Value::as_str) == Some(client_nonce)
        }) {
            Self::validate_human_queue_origin(entry)?;
            if forward_context.is_none()
                && entry.get("remoteMessageId").is_none()
                && self.pending_human_forward_is_batch_owned(conversation_id, entry)?
            {
                return Err("source-owned Human pending intent requires its original forward batch".into());
            }
            let same_sender = entry.get("authorId").and_then(serde_json::Value::as_str) == Some(sender_id);
            let same_text = entry.get("content").and_then(serde_json::Value::as_str) == Some(text);
            let persisted_reply = Self::normalize_human_reply_identity(
                &existing, entry.get("replyToId").and_then(serde_json::Value::as_str), sender_id,
            )?;
            let same_reply = persisted_reply.as_deref() == normalized_reply;
            if let Some(dispatched_reply) = entry.get("remoteReplyToMessageId") {
                let expected = normalized_reply.and_then(|reply_id| existing.iter().find(|row| {
                    row.get("id").and_then(serde_json::Value::as_str) == Some(reply_id)
                })).and_then(|row| row.get("remoteMessageId"))
                    .map(fabushi_identity_text).transpose()?;
                let dispatched_reply = fabushi_identity_text(dispatched_reply)?;
                if expected.as_deref() != Some(dispatched_reply.as_str()) {
                    return Err("sendHumanMessage reply conflicts with its dispatched remote identity".into());
                }
            }
            let same_attachments = match entry.get("attachments").and_then(serde_json::Value::as_array) {
                Some(values) => values.len() == normalized_attachments.len()
                    && values.iter().zip(normalized_attachments.iter()).all(|(current, requested)| {
                        current.get("name") == requested.get("name")
                            && (current.get("path") == requested.get("path") || current.get("resourceId").is_some())
                    }),
                None => normalized_attachments.is_empty(),
            };
            let same_forward_context = entry.get("forwardContext") == forward_context;
            let same_topic = entry.get("topicId").and_then(serde_json::Value::as_str) == topic_id;
            if !(same_sender && same_text && same_reply && same_attachments && same_forward_context && same_topic) {
                return Err("sendHumanMessage clientNonce already identifies different content, topic, or forward context".into());
            }
            if entry.get("remoteMessageId").is_some()
                || entry.get("delivery").and_then(serde_json::Value::as_str) == Some("sent")
            {
                let timestamp = entry.get("timestampMs").and_then(serde_json::Value::as_f64)
                    .unwrap_or(timestamp_ms);
                Self::update_human_activity(&owner, timestamp)?;
                return Ok(entry.clone());
            }
            if !matches!(
                entry.get("delivery").and_then(serde_json::Value::as_str),
                Some("pending" | "dispatching")
            ) {
                return Err("sendHumanMessage clientNonce is not in a retryable settlement state".into());
            }
            if entry.get("kind").and_then(serde_json::Value::as_str) != Some("message")
                || entry.get("authorKind").and_then(serde_json::Value::as_str) != Some("human")
                || entry.get("id").and_then(serde_json::Value::as_str) != Some(format!("human-message:{client_nonce}").as_str())
            {
                return Err("sendHumanMessage pending identity is not a local Human outbox row".into());
            }
            existing_pending = Some(entry.clone());
        }

        let kind_str = metadata
            .get("conversationKind")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("human");

        if matches!(kind_str, "group" | "channel") || topic_id.is_some() {
            let member_opt = owner.get_conversation_member(sender_id).map_err(|e| e.to_string())?;
            let topic_requested = topic_id.is_some();
            let topic_opt = if let Some(tid) = topic_id {
                owner.get_conversation_topic(tid).map_err(|e| e.to_string())?
            } else {
                None
            };

            let can_manage_topics = member_opt.as_ref().map(|m| {
                m.status == fabushi_messaging_core::MemberStatus::Owner
                    || (m.status == fabushi_messaging_core::MemberStatus::Administrator && m.admin_rights.manage_topics)
            }).unwrap_or(false);

            if topic_requested {
                if let Some(ref t) = topic_opt {
                    if t.hidden && !can_manage_topics {
                        return Err(format!(
                            "sendHumanMessage rejected the topic destination ({:?})",
                            fabushi_messaging_core::CommunitySendDenial::TopicNotFound
                        ));
                    }
                }
            }

            let last_sender_message_at_ms = existing
                .iter()
                .filter(|e| {
                    e.get("authorId").and_then(serde_json::Value::as_str) == Some(sender_id)
                        && e.get("clientNonce").and_then(serde_json::Value::as_str) != Some(client_nonce)
                        && e.get("delivery").and_then(serde_json::Value::as_str) != Some("failed")
                })
                .filter_map(|e| e.get("timestampMs").and_then(serde_json::Value::as_f64))
                .map(|t| t as i64)
                .max();

            let slow_mode_seconds = metadata
                .get("slowModeSeconds")
                .and_then(serde_json::Value::as_u64)
                .map(|v| v as u32);

            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;

            let conv_kind = match kind_str {
                "channel" => fabushi_messaging_core::ConversationKind::Channel,
                "group" => fabushi_messaging_core::ConversationKind::Group,
                _ => fabushi_messaging_core::ConversationKind::Direct,
            };

            let content_flags = fabushi_messaging_core::CommunitySendContentFlags {
                has_media: !normalized_attachments.is_empty(),
                has_poll: false,
                has_other: false,
                has_inline: false,
                has_game: false,
            };

            let policy_input = fabushi_messaging_core::CommunitySendPolicyInput {
                conversation_kind: conv_kind,
                member: member_opt.as_ref(),
                slow_mode_seconds,
                last_sender_message_at_ms,
                now_ms,
                target_topic: topic_opt.as_ref(),
                topic_requested,
                content: content_flags,
            };

            if let Err(denial) = fabushi_messaging_core::evaluate_community_send_policy(&policy_input) {
                return Err(format!("sendHumanMessage rejected by community send policy: {denial:?}"));
            }
        }

        if kind_str == "human" {
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
                    if queued_offline { object.insert("queuedOffline".into(), serde_json::json!(true)); }
                    if let Some(reply_id) = normalized_reply { object.insert("replyToId".into(), serde_json::json!(reply_id)); }
                    if !normalized_attachments.is_empty() { object.insert("attachments".into(), serde_json::Value::Array(normalized_attachments.clone())); }
                    if let Some(context) = forward_context { object.insert("forwardContext".into(), context.clone()); }
                    if let Some(tid) = topic_id { object.insert("topicId".into(), serde_json::json!(tid)); }
                    entry
                });
            if let Some(reply_id) = normalized_reply {
                pending_entry["replyToId"] = serde_json::json!(reply_id);
            }
            if forward_context.is_some_and(|context| context.get("batchVersion").is_some()) {
                pending_entry.as_object_mut().expect("Human pending message must be an object")
                    .insert("forwardBatchOwned".into(), serde_json::json!(true));
            }
            if !existing.iter().any(|entry| entry.get("id") == pending_entry.get("id"))
                && !owner.append_transcript_entry(&pending_entry).map_err(|error| error.to_string())?
            {
                return Err("Human pending send could not be durably recorded before dispatch".into());
            }
            if existing.iter().any(|entry| entry.get("id") == pending_entry.get("id")
                && entry.get("forwardBatchOwned") != pending_entry.get("forwardBatchOwned"))
            {
                owner.update_transcript_entry(&local_entry_id, &pending_entry)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| "Human forward ownership could not be durably recorded before dispatch".to_string())?;
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
                    let mut forward_bytes = if let Some(context) = forward_context.filter(|context|
                        context.get("batchVersion").and_then(serde_json::Value::as_u64) == Some(1)
                    ) {
                        let source_id = context.get("sourceConversationId")
                            .and_then(serde_json::Value::as_str)
                            .ok_or_else(|| "Human forward source identity is missing".to_string())?;
                        let request_id = context.get("requestId")
                            .and_then(serde_json::Value::as_str)
                            .ok_or_else(|| "Human forward request identity is missing".to_string())?;
                        let source_owner = self.open_human_conversation_db_owner(source_id)?;
                        let (account, issuer) = self.human_forward_scope()?;
                        if normalized_attachments.len() > 6 {
                            return Err("Human forward exceeds the six-attachment backend limit".into());
                        }
                        // Validate the complete target before any resource
                        // POST, then consume those exact buffers. This is
                        // bounded by six files of at most 32 MiB each and
                        // cannot reopen a changed path after preflight.
                        let mut verified = Vec::with_capacity(normalized_attachments.len());
                        for (index, attachment) in normalized_attachments.iter().enumerate() {
                            verified.push(read_forward_attachment_bytes(
                                &source_owner, &account, &issuer, request_id, index, attachment,
                            )?);
                        }
                        Some(verified.into_iter())
                    } else {
                        None
                    };
                    for attachment in &normalized_attachments {
                        let path = attachment.get("path").and_then(serde_json::Value::as_str)
                            .ok_or_else(|| "sendHumanMessage attachment requires path".to_string())?;
                        let name = attachment.get("name").and_then(serde_json::Value::as_str)
                            .ok_or_else(|| "sendHumanMessage attachment requires name".to_string())?;
                        let resource = if let Some(buffers) = forward_bytes.as_mut() {
                            let bytes = buffers.next().ok_or_else(||
                                "Human forward attachment preflight omitted a buffer".to_string())?;
                            // Use the verified buffer itself. Reopening its
                            // path in the uploader would reintroduce the race.
                            client.upload_direct_message_resource_bytes_for_user(bytes, name, sender_id)?
                        } else {
                            client.upload_direct_message_resource_for_user(std::path::Path::new(path), name, sender_id)?
                        };
                        uploaded.push(resource);
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

                // Initial staging is visible atomically to a history snapshot.
                // Release before the message POST so sync can discover an
                // accepted send while this request is still in flight.
                drop(staging_guard.take());
                let request_credential_fingerprint = client.request_credential_fingerprint()?;
                let response = client.send_direct_message_for_settlement(&peer_human_id, text, client_nonce, remote_reply_to_id.as_deref(), &uploaded, sender_id);
                // The POST may outlive a sync/reaction that already settled the
                // same nonce. Rejoin the existing conversation mutation lane
                // before inspecting or applying its eventual response. Sync
                // supplies its guard when replaying a persisted send, avoiding
                // recursive acquisition of the non-reentrant mutex.
                let serial = self.human_message_lock(conversation_id)?;
                // The transport checked credential stability at send/body read.
                // Recheck after regaining the Session lock: credentials may
                // change while an old response waits behind sync/reaction.
                let _settlement_guard = Self::acquire_human_send_settlement_guard(
                    client, serial.as_ref(), held_conversation_guard,
                    &request_credential_fingerprint, &response,
                )?;
                let durable = owner.get_transcript_entries().map_err(|error| error.to_string())?;
                let settled = durable.iter().find(|entry| {
                    let remote_id = entry.get("remoteMessageId").and_then(serde_json::Value::as_str);
                    let resources = entry.get("attachments").and_then(serde_json::Value::as_array)
                        .map(|items| items.iter().map(|item| item.get("resourceId").and_then(serde_json::Value::as_str))
                            .collect::<Vec<_>>()).unwrap_or_default();
                    remote_id.is_some_and(|id| entry.get("id").and_then(serde_json::Value::as_str)
                        == Some(format!("human-server-message:{id}").as_str()))
                        && entry.get("delivery").and_then(serde_json::Value::as_str) == Some("sent")
                        && entry.get("authorId").and_then(serde_json::Value::as_str) == Some(sender_id)
                        && entry.get("clientNonce").and_then(serde_json::Value::as_str) == Some(client_nonce)
                        && entry.get("content").and_then(serde_json::Value::as_str) == Some(text)
                        && entry.get("remoteReplyToMessageId").and_then(serde_json::Value::as_str)
                            == remote_reply_to_id.as_deref()
                        && resources == uploaded.iter().map(|item| Some(item.resource_id.as_str())).collect::<Vec<_>>()
                        && entry.get("forwardContext") == forward_context
                });
                let remote = match response {
                    Ok(remote) => remote,
                    Err(FabushiHumanSendError::Transport(error)) => {
                        let settled = settled.ok_or(error)?;
                        let timestamp = settled.get("timestampMs").and_then(serde_json::Value::as_f64)
                            .unwrap_or(timestamp_ms);
                        Self::update_human_activity(&owner, timestamp)?;
                        return Ok(settled.clone());
                    }
                    Err(FabushiHumanSendError::Rejected(error)) => return Err(error),
                };
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
                self.validate_remote_human_message(&remote, sender_id, &peer_human_id, &durable)?;
                chrono::DateTime::parse_from_rfc3339(&remote.created_at)
                    .map_err(|error| format!("remote Human message timestamp is invalid: {error}"))?;
                if let Some(settled) = settled {
                    // Admission still rejects a changed canonical identity,
                    // nonce, resources, reply or malformed reaction summary.
                    // A valid late acknowledgement has no freshness authority
                    // over reactions/receipts already persisted by sync or POST.
                    let timestamp = settled.get("timestampMs").and_then(serde_json::Value::as_f64)
                        .unwrap_or(timestamp_ms);
                    Self::update_human_activity(&owner, timestamp)?;
                    return Ok(settled.clone());
                }
                self.materialize_remote_human_message(&owner, &remote)
            })();
            // A resource/preflight failure may return before the POST release.
            drop(staging_guard.take());

            if let Err(error) = dispatch_result.as_ref() {
                // Settlement may have advanced while a transport/preflight
                // error was returning. Serialize this lookup and pending-only
                // update with sync so it cannot restore an obsolete row.
                let serial = self.human_message_lock(conversation_id)?;
                let _failure_guard = if held_conversation_guard.is_none() {
                    Some(serial.lock().map_err(|_| "Human conversation serial lock poisoned".to_string())?)
                } else {
                    None
                };
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
        if queued_offline { object.insert("queuedOffline".into(), serde_json::json!(true)); }
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
        if let Some(tid) = topic_id {
            object.insert("topicId".into(), serde_json::json!(tid));
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
                Self::update_human_activity(&owner, timestamp_ms)?;
                return Ok(replay);
            }
            return Err("sendHumanMessage clientNonce already identifies different content".into());
        }
        Self::update_human_activity(&owner, timestamp_ms)?;
        Ok(entry)
    }

}
