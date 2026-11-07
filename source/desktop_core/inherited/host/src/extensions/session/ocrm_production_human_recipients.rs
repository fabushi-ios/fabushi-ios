use std::collections::BTreeSet;
use std::fs;
use std::sync::Arc;
use sha2::{Digest, Sha256};

use super::agent_db::SandAgentDb;
use super::native_messaging::fabushi_identity_text;
use super::production::ProductionSessionWorkers;
use super::session_paths::{get_native_conversation_db_path, get_native_conversations_root};

impl ProductionSessionWorkers {
    pub fn open_human_conversation_db_owner(
        &self,
        conversation_id: &str,
    ) -> Result<Arc<SandAgentDb>, String> {
        let db_path = get_native_conversation_db_path(&self.agents_root, conversation_id)
            .map_err(|error| error.to_string())?;
        if !db_path.is_file() {
            return Err(format!("Human conversation missing: {conversation_id}"));
        }
        let key = Self::native_conversation_owner_key(conversation_id);
        let mut owners = self
            .db_owners
            .lock()
            .map_err(|_| "conversation db owner map poisoned".to_string())?;
        if let Some(owner) = owners.get(&key) {
            return Ok(Arc::clone(owner));
        }
        let owner = Arc::new(
            SandAgentDb::open_conversation_store(&db_path, self.busy_timeout_ms)
                .map_err(|error| error.to_string())?,
        );
        if owner
            .get_metadata("conversationKind")
            .map_err(|error| error.to_string())?
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .as_deref()
            != Some("human")
        {
            owner.close(false);
            return Err(format!("Conversation is not a Human conversation: {conversation_id}"));
        }
        owners.insert(key, Arc::clone(&owner));
        Ok(owner)
    }

    pub fn list_human_conversations(&self) -> Result<Vec<serde_json::Value>, String> {
        let root = get_native_conversations_root(&self.agents_root);
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries.filter_map(Result::ok).collect::<Vec<_>>(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.to_string()),
        };
        let mut conversations = Vec::new();
        for entry in entries {
            if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                continue;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            let Ok(owner) = self.open_human_conversation_db_owner(&id) else {
                continue;
            };
            let metadata = owner.read_metadata().map_err(|error| error.to_string())?;
            if !self.metadata_has_local_human(&metadata)? {
                continue;
            }
            let title = metadata
                .get("title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Human conversation");
            let participant_ids = metadata
                .get("participantIds")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([]));
            let updated_at = metadata
                .get("lastActivityAt")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or_default();
            conversations.push(serde_json::json!({
                "id": id,
                "kind": "human",
                "title": title,
                "participantIds": participant_ids,
                "updatedAt": updated_at,
            }));
        }
        conversations.sort_by(|left, right| {
            left.get("id")
                .and_then(serde_json::Value::as_str)
                .cmp(&right.get("id").and_then(serde_json::Value::as_str))
        });
        Ok(conversations)
    }

    pub fn search_human_recipients(
        &self,
        source_conversation_id: &str,
        source_entry_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<serde_json::Value>, String> {
        let source_owner = self.open_human_conversation_db_owner(source_conversation_id)?;
        let source_metadata = source_owner.read_metadata().map_err(|error| error.to_string())?;
        if !self.metadata_has_local_human(&source_metadata)? {
            return Err("local Human identity is not a participant in the source conversation".into());
        }
        let source_entry_id = source_entry_id.trim();
        if source_entry_id.is_empty() {
            return Err("recipient search requires sourceEntryId".into());
        }
        let source_entry = source_owner
            .get_transcript_entries()
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|entry| entry.get("id").and_then(serde_json::Value::as_str) == Some(source_entry_id))
            .ok_or_else(|| "recipient search source message is missing".to_string())?;
        if source_entry.get("kind").and_then(serde_json::Value::as_str) != Some("message")
            || source_entry.get("delivery").and_then(serde_json::Value::as_str) != Some("sent")
        {
            return Err("recipient search source must be a settled canonical message".into());
        }
        let requirements = fabushi_messaging_core::RecipientSearchRequirements {
            require_media: source_entry
                .get("attachments")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|attachments| !attachments.is_empty()),
            source_protected_content: source_entry
                .get("protectedContent")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            ..fabushi_messaging_core::RecipientSearchRequirements::default()
        };
        let local_human_id = self.local_human_id()?;
        let normalized_query = query.trim().to_lowercase();
        let mut recipients = self
            .list_human_conversations()?
            .into_iter()
            .filter(|conversation| {
                let sender_is_participant = conversation
                    .get("participantIds")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|ids| {
                        ids.iter().any(|id| id.as_str() == Some(local_human_id))
                    });
                if !fabushi_messaging_core::recipient_search_authorized(
                    fabushi_messaging_core::RecipientAuthorizationInput::direct_default(
                        sender_is_participant,
                    ),
                    requirements,
                ) {
                    return false;
                }
                if normalized_query.is_empty() {
                    return true;
                }
                let title_matches = conversation
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|title| title.to_lowercase().contains(&normalized_query));
                let participant_matches = conversation
                    .get("participantIds")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|ids| {
                        ids.iter().any(|id| {
                            id.as_str()
                                .is_some_and(|id| id.to_lowercase().contains(&normalized_query))
                        })
                    });
                title_matches || participant_matches
            })
            .collect::<Vec<_>>();

        recipients.sort_by(|left, right| {
            let left_updated = left
                .get("updatedAt")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or_default();
            let right_updated = right
                .get("updatedAt")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or_default();
            right_updated
                .partial_cmp(&left_updated)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    left.get("title")
                        .and_then(serde_json::Value::as_str)
                        .cmp(&right.get("title").and_then(serde_json::Value::as_str))
                })
                .then_with(|| {
                    left.get("id")
                        .and_then(serde_json::Value::as_str)
                        .cmp(&right.get("id").and_then(serde_json::Value::as_str))
                })
        });
        recipients.truncate(limit.clamp(1, 100));
        Ok(recipients)
    }

    pub fn forward_human_message(
        &self,
        source_conversation_id: &str,
        source_entry_id: &str,
        destination_conversation_ids: &[String],
        client_nonce: &str,
        drop_sender_names: bool,
        drop_captions: bool,
    ) -> Result<serde_json::Value, String> {
        let client_nonce = client_nonce.trim();
        if client_nonce.is_empty() || client_nonce.len() > 200 {
            return Err("forwardHumanMessage requires a valid clientNonce".into());
        }
        if destination_conversation_ids.is_empty() {
            return Err("forwardHumanMessage requires at least one destination".into());
        }

        let mut seen_destinations = BTreeSet::new();
        let mut destinations = Vec::new();
        for destination in destination_conversation_ids {
            let destination = destination.trim();
            if destination.is_empty() {
                return Err("forwardHumanMessage destination id must not be empty".into());
            }
            if seen_destinations.insert(destination.to_string()) {
                destinations.push(destination.to_string());
            }
        }
        if destinations.len() > 32 {
            return Err("forwardHumanMessage supports at most 32 unique destinations".into());
        }

        let source_owner = self.open_human_conversation_db_owner(source_conversation_id)?;
        let source_metadata = source_owner.read_metadata().map_err(|error| error.to_string())?;
        if !self.metadata_has_local_human(&source_metadata)? {
            return Err("local Human identity is not a participant in the source conversation".into());
        }
        let source_entry_id = source_entry_id.trim();
        let source_entry = source_owner
            .get_transcript_entries()
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|entry| entry.get("id").and_then(serde_json::Value::as_str) == Some(source_entry_id))
            .ok_or_else(|| "forwardHumanMessage source message is missing".to_string())?;
        if source_entry.get("kind").and_then(serde_json::Value::as_str) != Some("message")
            || source_entry.get("delivery").and_then(serde_json::Value::as_str) != Some("sent")
        {
            return Err("forwardHumanMessage source must be a settled canonical message".into());
        }
        if source_entry
            .get("protectedContent")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            return Err("forwardHumanMessage source is protected and cannot be forwarded".into());
        }

        let source_text = source_entry
            .get("content")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let attachments = source_entry
            .get("attachments")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        if source_text.trim().is_empty() && attachments.is_empty() {
            return Err("forwardHumanMessage source has no forwardable content".into());
        }

        let privacy = fabushi_messaging_core::ForwardPrivacy {
            drop_sender_names,
            drop_captions,
        }
        .normalized();
        let drop_sender_names = privacy.drop_sender_names;
        let drop_captions = privacy.drop_captions;
        let forwarded_text = if drop_captions && !attachments.is_empty() {
            ""
        } else {
            source_text
        };
        let mut forward_context = serde_json::json!({
            "requestId": client_nonce,
            "sourceConversationId": source_conversation_id,
            "sourceEntryId": source_entry_id,
            "privacy": {
                "dropSenderNames": drop_sender_names,
                "dropCaptions": drop_captions,
            },
        });
        if !drop_sender_names {
            let mut origin = serde_json::Map::new();
            if let Some(author_id) = source_entry.get("authorId").cloned() {
                origin.insert("authorId".into(), author_id);
            }
            if let Some(author_name) = source_entry.get("authorName").cloned() {
                origin.insert("authorName".into(), author_name);
            }
            if !origin.is_empty() {
                forward_context
                    .as_object_mut()
                    .expect("forward context is an object")
                    .insert("origin".into(), serde_json::Value::Object(origin));
            }
        }

        let mut settlement = Vec::with_capacity(destinations.len());
        for destination_conversation_id in destinations {
            let mut digest = Sha256::new();
            digest.update(client_nonce.as_bytes());
            digest.update([0]);
            digest.update(destination_conversation_id.as_bytes());
            let destination_nonce = format!("human-forward-{:x}", digest.finalize());

            match self.append_human_message_with_context(
                &destination_conversation_id,
                forwarded_text,
                &destination_nonce,
                None,
                None,
                &attachments,
                Some(&forward_context),
            ) {
                Ok(entry) => settlement.push(serde_json::json!({
                    "conversationId": destination_conversation_id,
                    "clientNonce": destination_nonce,
                    "status": "sent",
                    "entry": entry,
                })),
                Err(error) => settlement.push(serde_json::json!({
                    "conversationId": destination_conversation_id,
                    "clientNonce": destination_nonce,
                    "status": "failed",
                    "error": error,
                })),
            }
        }

        Ok(serde_json::json!({
            "requestId": client_nonce,
            "privacy": {
                "dropSenderNames": drop_sender_names,
                "dropCaptions": drop_captions,
            },
            "destinations": settlement,
        }))
    }
}
