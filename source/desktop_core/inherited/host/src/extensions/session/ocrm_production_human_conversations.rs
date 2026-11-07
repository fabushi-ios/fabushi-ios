use std::collections::BTreeSet;
use std::fs;
use std::sync::Arc;
use sha2::{Digest, Sha256};

use super::agent_db::SandAgentDb;
use super::native_messaging::{
    FabushiRemoteHumanAttachment, FabushiRemoteHumanMessage, fabushi_identity_text,
};
use super::production::ProductionSessionWorkers;
use super::session_paths::get_native_conversation_db_path;

impl ProductionSessionWorkers {
    fn materialize_remote_human_attachment(
        &self,
        attachment: &FabushiRemoteHumanAttachment,
    ) -> Result<serde_json::Value, String> {
        let resource_id = attachment.resource_id.trim();
        if resource_id.is_empty()
            || resource_id.len() > 128
            || !resource_id
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        {
            return Err("remote Human attachment resource id is invalid".into());
        }
        if attachment.size == 0 || attachment.size > 32 * 1024 * 1024 {
            return Err("remote Human attachment size exceeds the supported limit".into());
        }
        let mut name = attachment
            .name
            .chars()
            .map(|character| match character {
                '/' | '\\' | '\0' => '_',
                character if character.is_control() => '_',
                character => character,
            })
            .take(255)
            .collect::<String>();
        if name.trim().is_empty() {
            name = "attachment".into();
        }
        let resource_root = self
            .agents_root
            .join("human-message-resources")
            .join(resource_id);
        fs::create_dir_all(&resource_root).map_err(|error| {
            format!(
                "remote Human attachment directory could not be created: {error}"
            )
        })?;
        let path = resource_root.join(name);
        let ready = fs::metadata(&path)
            .ok()
            .is_some_and(|metadata| metadata.is_file() && metadata.len() == attachment.size);
        if !ready {
            let client = self
                .shipping_native_messaging()?
                .ok_or_else(|| "remote Human attachment requires shipping native messaging".to_string())?;
            let bytes = client.download_direct_message_resource(resource_id, attachment.size)?;
            let temporary_path = resource_root.join(".download.tmp");
            fs::write(&temporary_path, &bytes).map_err(|error| {
                format!("remote Human attachment could not be staged: {error}")
            })?;
            if let Err(error) = fs::rename(&temporary_path, &path) {
                let _ = fs::remove_file(&temporary_path);
                return Err(format!(
                    "remote Human attachment could not be committed atomically: {error}"
                ));
            }
        }
        Ok(serde_json::json!({
            "path": path.to_string_lossy(),
            "name": attachment.name,
            "size": attachment.size,
            "mimeType": attachment.content_type,
            "resourceId": attachment.resource_id,
            "remoteCreatedAt": attachment.created_at,
        }))
    }

    pub(crate) fn materialize_remote_human_message(
        &self,
        owner: &SandAgentDb,
        remote: &FabushiRemoteHumanMessage,
    ) -> Result<serde_json::Value, String> {
        let local_human_id = self.local_human_id()?;
        let remote_id = fabushi_identity_text(&remote.id)?;
        let sender_id = fabushi_identity_text(&remote.sender_user_id)?;
        let recipient_id = fabushi_identity_text(&remote.recipient_user_id)?;
        if sender_id != local_human_id && recipient_id != local_human_id {
            return Err("remote Human message does not belong to the local Fabushi account".into());
        }
        let timestamp_ms = chrono::DateTime::parse_from_rfc3339(&remote.created_at)
            .map_err(|error| format!("remote Human message timestamp is invalid: {error}"))?
            .timestamp_millis() as f64;
        let entry_id = format!("human-server-message:{remote_id}");
        let mut entry = serde_json::json!({
            "id": entry_id,
            "kind": "message",
            "role": "user",
            "authorKind": "human",
            "authorId": sender_id,
            "content": remote.text,
            "timestampMs": timestamp_ms,
            "delivery": "sent",
            "remoteMessageId": remote_id,
            "remoteCreatedAt": remote.created_at,
        });
        let object = entry
            .as_object_mut()
            .ok_or_else(|| "remote Human message projection failed".to_string())?;
        if let Some(author_name) = remote
            .sender_username
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            object.insert("authorName".into(), serde_json::json!(author_name));
        }
        if let Some(client_nonce) = remote
            .client_request_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            object.insert("clientNonce".into(), serde_json::json!(client_nonce));
        }
        if let Some(read_at) = remote
            .read_at
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            object.insert("readAt".into(), serde_json::json!(read_at));
        }
        if let Some(reply_to_message_id) = remote
            .reply_to_message_id
            .as_ref()
            .map(fabushi_identity_text)
            .transpose()?
        {
            object.insert(
                "replyToId".into(),
                serde_json::json!(format!("human-server-message:{reply_to_message_id}")),
            );
            object.insert(
                "remoteReplyToMessageId".into(),
                serde_json::json!(reply_to_message_id),
            );
        }
        if !remote.attachments.is_empty() {
            let attachments = remote
                .attachments
                .iter()
                .map(|attachment| self.materialize_remote_human_attachment(attachment))
                .collect::<Result<Vec<_>, _>>()?;
            object.insert("attachments".into(), serde_json::Value::Array(attachments));
        }
        let peer_human_id = if sender_id == local_human_id {
            recipient_id.as_str()
        } else {
            sender_id.as_str()
        };
        let mut projected_reactions = Vec::new();
        for reaction in &remote.reactions {
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
        object.insert(
            "remoteReactions".into(),
            serde_json::to_value(&remote.reactions).map_err(|error| {
                format!("remote Human reactions could not be projected: {error}")
            })?,
        );
        object.insert(
            "reactions".into(),
            serde_json::Value::Array(projected_reactions),
        );
        let admitted_session_key = owner
            .read_session_work_admission()
            .map_err(|error| error.to_string())?
            .map(|record| record.session_key)
            .filter(|value| !value.trim().is_empty());
        let existing = owner
            .get_transcript_entries()
            .map_err(|error| error.to_string())?;
        if let Some(current) = existing.iter().find(|candidate| {
            candidate.get("id").and_then(serde_json::Value::as_str)
                == entry.get("id").and_then(serde_json::Value::as_str)
        }) {
            if let Some(context) = current.get("forwardContext").cloned() {
                entry
                    .as_object_mut()
                    .ok_or_else(|| "remote Human message projection failed".to_string())?
                    .insert("forwardContext".into(), context);
            }
            if current == &entry {
                return Ok(current.clone());
            }
            let entry_id = entry
                .get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| "remote Human message projection omitted its id".to_string())?;
            let updated = if let Some(session_key) = admitted_session_key.as_deref() {
                entry["sessionKey"] = serde_json::Value::String(session_key.to_string());
                owner
                    .update_transcript_entry_if_session_admitted(
                        session_key,
                        &entry_id,
                        &entry,
                    )
                    .map_err(|error| error.to_string())?
            } else {
                owner
                    .update_transcript_entry(&entry_id, &entry)
                    .map_err(|error| error.to_string())?
            };
            return updated.ok_or_else(|| "remote Human message could not be durably refreshed".to_string());
        }
        if let Some(client_nonce) = entry.get("clientNonce").and_then(serde_json::Value::as_str) {
            if let Some(current) = existing.iter().find(|candidate| {
                candidate.get("clientNonce").and_then(serde_json::Value::as_str) == Some(client_nonce)
            }) {
                if let Some(context) = current.get("forwardContext").cloned() {
                    entry
                        .as_object_mut()
                        .ok_or_else(|| "remote Human message projection failed".to_string())?
                        .insert("forwardContext".into(), context);
                }
                let current_id = current.get("id").and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "local Human settlement row omitted its id".to_string())?;
                let current_remote = current.get("remoteMessageId").and_then(serde_json::Value::as_str);
                if let Some(current_remote) = current_remote {
                    if current_remote != remote_id {
                        return Err("local Human settlement row conflicts with a different remote identity".into());
                    }
                } else if !matches!(
                    current.get("delivery").and_then(serde_json::Value::as_str),
                    Some("pending" | "dispatching")
                ) {
                    return Err("local Human message already uses the server clientRequestId without a retryable settlement state".into());
                }
                let settled = if let Some(session_key) = admitted_session_key.as_deref() {
                    entry["sessionKey"] = serde_json::Value::String(session_key.to_string());
                    owner
                        .settle_transcript_entry_identity_if_session_admitted(
                            session_key,
                            current_id,
                            &entry,
                        )
                        .map_err(|error| error.to_string())?
                } else {
                    owner
                        .settle_transcript_entry_identity(current_id, &entry)
                        .map_err(|error| error.to_string())?
                };
                return settled.ok_or_else(|| {
                    "local Human message disappeared before remote identity settlement".to_string()
                });
            }
        }
        if let Some(session_key) = admitted_session_key {
            entry["sessionKey"] = serde_json::Value::String(session_key.clone());
            if owner
                .append_transcript_entries_if_session_admitted(&session_key, &[entry.clone()])
                .map_err(|error| error.to_string())?
                == 0
            {
                let replay = owner
                    .get_transcript_entries()
                    .map_err(|error| error.to_string())?
                    .into_iter()
                    .find(|candidate| candidate.get("id") == entry.get("id"))
                    .ok_or_else(|| "remote Human message was not durably materialized".to_string())?;
                return Ok(replay);
            }
        } else if !owner
            .append_transcript_entry(&entry)
            .map_err(|error| error.to_string())?
        {
            let replay = owner
                .get_transcript_entries()
                .map_err(|error| error.to_string())?
                .into_iter()
                .find(|candidate| candidate.get("id") == entry.get("id"))
                .ok_or_else(|| "remote Human message was not durably materialized".to_string())?;
            return Ok(replay);
        }
        if !owner
            .set_metadata("lastActivityAt", serde_json::json!(timestamp_ms))
            .map_err(|error| error.to_string())?
        {
            return Err("remote Human message activity metadata was not durably updated".into());
        }
        Ok(entry)
    }

    pub fn sync_human_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<serde_json::Value>, String> {
        const PAGE_LIMIT: usize = 200;
        const MAX_GAP_PAGES: usize = 32;

        let owner = self.open_human_conversation_db_owner(conversation_id)?;
        let metadata = owner.read_metadata().map_err(|error| error.to_string())?;
        if !self.metadata_has_local_human(&metadata)? {
            return Err("local Human identity is not a conversation participant".into());
        }
        let Some(client) = self.shipping_native_messaging()? else {
            return owner.get_transcript_entries().map_err(|error| error.to_string());
        };
        let local_human_id = self.local_human_id()?.to_string();
        let pending = owner
            .get_transcript_entries()
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|entry| {
                entry.get("authorId").and_then(serde_json::Value::as_str) == Some(local_human_id.as_str())
                    && entry.get("remoteMessageId").is_none()
                    && matches!(
                        entry.get("delivery").and_then(serde_json::Value::as_str),
                        Some("pending" | "dispatching")
                    )
                    && entry.get("clientNonce").and_then(serde_json::Value::as_str).is_some()
            })
            .collect::<Vec<_>>();
        for entry in pending {
            let Some(client_nonce) = entry.get("clientNonce").and_then(serde_json::Value::as_str) else { continue; };
            let text = entry.get("content").and_then(serde_json::Value::as_str).unwrap_or_default();
            let reply_to_id = entry.get("replyToId").and_then(serde_json::Value::as_str);
            let composed_at_ms = entry.get("composedAtMs").and_then(serde_json::Value::as_f64);
            let attachments = entry.get("attachments").and_then(serde_json::Value::as_array).cloned().unwrap_or_default();
            let _ = self.append_human_message(conversation_id, text, client_nonce, composed_at_ms, reply_to_id, &attachments);
        }
        let peer_human_id = self.peer_human_id_from_metadata(&metadata)?;
        let existing = owner
            .get_transcript_entries()
            .map_err(|error| error.to_string())?;
        let known_remote_ids = existing
            .iter()
            .filter_map(|entry| entry.get("remoteMessageId").and_then(serde_json::Value::as_str))
            .map(str::to_string)
            .collect::<BTreeSet<_>>();

        // The shipping endpoint selects newest rows but returns each page
        // oldest-first. Walk bounded older pages until one known server message
        // proves overlap, or the server reaches the end of history. This closes
        // reconnect and fresh-device gaps without introducing a second message
        // store or treating renderer state as truth.
        let mut before: Option<String> = None;
        let mut unseen_pages: Vec<Vec<FabushiRemoteHumanMessage>> = Vec::new();
        let mut pages_read = 0usize;
        loop {
            let page = client.list_direct_messages(
                &peer_human_id,
                before.as_deref(),
                PAGE_LIMIT,
            )?;
            if page.is_empty() {
                break;
            }
            pages_read += 1;

            let mut overlap_at = None;
            for (index, remote) in page.iter().enumerate().rev() {
                let remote_id = fabushi_identity_text(&remote.id)?;
                if known_remote_ids.contains(&remote_id) {
                    overlap_at = Some(index);
                    break;
                }
            }
            let unseen_start = overlap_at.map_or(0, |index| index + 1);
            if unseen_start < page.len() {
                unseen_pages.push(page[unseen_start..].to_vec());
            }

            if overlap_at.is_some() || page.len() < PAGE_LIMIT {
                break;
            }
            if pages_read >= MAX_GAP_PAGES {
                return Err(format!(
                    "Fabushi Human message reconnect gap exceeded the bounded recovery window of {} messages",
                    PAGE_LIMIT * MAX_GAP_PAGES
                ));
            }

            let next_before = page
                .first()
                .map(|message| message.created_at.trim())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    "Fabushi Human message sync page omitted its oldest timestamp".to_string()
                })?
                .to_string();
            if before.as_deref() == Some(next_before.as_str()) {
                return Err(
                    "Fabushi Human message sync cursor did not advance; refusing an unbounded retry"
                        .into(),
                );
            }
            before = Some(next_before);
        }

        // Page windows arrive newest-window first while rows inside each page are
        // oldest-first. Reverse only the page windows so canonical
        // Session/Transcript replay remains deterministic after restart.
        for page in unseen_pages.into_iter().rev() {
            for remote in page {
                self.materialize_remote_human_message(&owner, &remote)?;
            }
        }
        owner.get_transcript_entries().map_err(|error| error.to_string())
    }

    pub fn sync_human_conversations(&self) -> Result<Vec<serde_json::Value>, String> {
        let Some(client) = self.shipping_native_messaging()? else {
            return self.list_human_conversations();
        };
        let identity = client.identity()?;
        if identity.user_id != self.local_human_id()? {
            return Err(format!(
                "Fabushi native messaging credential user {} does not match active Human identity {}",
                identity.user_id,
                self.local_human_id()?
            ));
        }
        for friend in client.list_friends()? {
            let peer_human_id = fabushi_identity_text(&friend.user_id)?;
            let title = friend.display_name.trim();
            let conversation = self.create_human_conversation(
                &peer_human_id,
                if title.is_empty() { &friend.username } else { title },
            )?;
            let conversation_id = conversation
                .get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "synced Human conversation omitted its id".to_string())?;
            self.sync_human_conversation(conversation_id)?;
        }
        self.sync_human_calls()?;
        self.list_human_conversations()
    }

    pub fn create_human_conversation(
        &self,
        peer_human_id: &str,
        title: &str,
    ) -> Result<serde_json::Value, String> {
        let local_human_id = self.local_human_id()?;
        let peer_human_id = peer_human_id.trim();
        let title = title.trim();
        if peer_human_id.is_empty() {
            return Err("human conversation requires peerHumanId".into());
        }
        if local_human_id == peer_human_id {
            return Err("human conversation participants must be distinct".into());
        }
        let mut participant_ids = [local_human_id.to_string(), peer_human_id.to_string()];
        participant_ids.sort();
        let mut digest = Sha256::new();
        for participant_id in &participant_ids {
            digest.update((participant_id.len() as u64).to_be_bytes());
            digest.update(participant_id.as_bytes());
        }
        let conversation_id = format!("human-direct-{:x}", digest.finalize());
        let db_path = get_native_conversation_db_path(&self.agents_root, &conversation_id)
            .map_err(|error| error.to_string())?;
        if db_path.is_file() {
            let owner = self.open_human_conversation_db_owner(&conversation_id)?;
            let metadata = owner.read_metadata().map_err(|error| error.to_string())?;
            if !self.metadata_has_local_human(&metadata)? {
                return Err("existing Human conversation does not contain the local identity".into());
            }
            let existing_participants = metadata
                .get("participantIds")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| "existing Human conversation participants are invalid".to_string())?;
            if !existing_participants
                .iter()
                .any(|value| value.as_str() == Some(peer_human_id))
            {
                return Err("existing Human conversation does not contain the requested peer".into());
            }
            let existing_title = metadata
                .get("title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(peer_human_id);
            return Ok(serde_json::json!({
                "id": conversation_id,
                "kind": "human",
                "title": existing_title,
                "participantIds": metadata.get("participantIds").cloned().unwrap_or_else(|| serde_json::json!([])),
            }));
        }
        let conversation_dir = db_path
            .parent()
            .ok_or_else(|| "native conversation database has no parent".to_string())?;
        fs::create_dir_all(conversation_dir).map_err(|error| error.to_string())?;
        let owner = Arc::new(
            SandAgentDb::open_conversation_store(&db_path, self.busy_timeout_ms)
                .map_err(|error| error.to_string())?,
        );
        let display_title = if title.is_empty() { peer_human_id } else { title };
        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as f64;
        let metadata = serde_json::json!({
            "conversationKind": "human",
            "conversationId": conversation_id,
            "title": display_title,
            "participantIds": [local_human_id, peer_human_id],
            "createdAt": created_at,
            "lastActivityAt": 0.0,
        });
        if !owner
            .initialize_metadata_if_missing(metadata)
            .map_err(|error| error.to_string())?
        {
            owner.close(false);
            let _ = fs::remove_dir_all(conversation_dir);
            return Err("failed to initialize native Human conversation metadata".into());
        }
        self.db_owners
            .lock()
            .map_err(|_| "conversation db owner map poisoned".to_string())?
            .insert(Self::native_conversation_owner_key(&conversation_id), Arc::clone(&owner));
        Ok(serde_json::json!({
            "id": conversation_id,
            "kind": "human",
            "title": display_title,
            "participantIds": [local_human_id, peer_human_id],
        }))
    }
}
