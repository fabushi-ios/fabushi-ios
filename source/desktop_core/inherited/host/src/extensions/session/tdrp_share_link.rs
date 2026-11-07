//! Telegram / TDRP Share Comments, Deep Link Sharing, and Destination Messaging.

use super::*;

impl ProductionSessionWorkers {
    pub fn compose_share_comment(
        &self,
        link: &str,
        comment: fabushi_messaging_core::FormattedText,
    ) -> Result<fabushi_messaging_core::FormattedText, fabushi_messaging_core::TextEntityOffsetOverflow> {
        fabushi_messaging_core::compose_share_comment(link, comment)
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
        self.forward_human_message_with_comment(
            source_conversation_id,
            source_entry_id,
            destination_conversation_ids,
            client_nonce,
            drop_sender_names,
            drop_captions,
            None,
        )
    }

    pub fn forward_human_message_to_destinations(
        &self,
        source_conversation_id: &str,
        source_entry_id: &str,
        destinations: &[HumanForwardDestination],
        client_nonce: &str,
        drop_sender_names: bool,
        drop_captions: bool,
        comment: Option<fabushi_messaging_core::FormattedText>,
    ) -> Result<serde_json::Value, String> {
        let client_nonce = client_nonce.trim();
        if client_nonce.is_empty() || client_nonce.len() > 200 {
            return Err("forwardHumanMessage requires a valid clientNonce".into());
        }
        let source_entry_id = source_entry_id.trim();
        if source_entry_id.is_empty() {
            return Err("forwardHumanMessage requires a sourceEntryId".into());
        }
        if destinations.is_empty() {
            return Err("forwardHumanMessage requires at least one destination".into());
        }
        let mut seen = BTreeSet::new();
        let mut unique_destinations: Vec<HumanForwardDestination> = Vec::new();
        let mut destination_conversation_ids: Vec<String> = Vec::new();
        for dest in destinations {
            let id = dest.conversation_id.trim();
            if id.is_empty() {
                return Err("forwardHumanMessage destination id must not be empty".into());
            }
            if seen.insert(id.to_string()) {
                unique_destinations.push(HumanForwardDestination {
                    conversation_id: id.to_string(),
                    topic_id: dest.topic_id.as_ref().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
                });
                destination_conversation_ids.push(id.to_string());
            }
        }
        if destination_conversation_ids.len() > 32 {
            return Err("forwardHumanMessage supports at most 32 unique destinations".into());
        }
        let requested_privacy = fabushi_messaging_core::ForwardPrivacy { drop_sender_names, drop_captions }.normalized();
        let (account, issuer) = self.human_forward_scope()?;

        // Validate comment using pure validate_formatted_text
        let normalized_comment = if let Some(c) = comment {
            fabushi_messaging_core::validate_formatted_text(&c)
                .map_err(|err| format!("forwardHumanMessage comment validation failed: {err}"))?;
            if c.text.trim().is_empty() {
                None
            } else {
                Some(c)
            }
        } else {
            None
        };
        let comment_json = normalized_comment.as_ref().map(|c| {
            serde_json::to_value(c).expect("FormattedText serialization")
        });

        // Serialize request-ID discovery and initial source persistence by
        // account/issuer. Release this short lane before destination locks/HTTP.
        let staging = self.human_message_lock(&format!("forward-account:{account}:{issuer}"))?;
        let stage_guard = staging.lock().map_err(|_| "Human forward staging lock poisoned".to_string())?;
        let existing = self.find_forward_request(&account, &issuer, client_nonce)?;
        if let Some(ref prior) = existing {
            if prior.source_conversation_id != source_conversation_id || prior.source_entry_id != source_entry_id {
                return Err("forwardHumanMessage clientNonce already belongs to another source message".into());
            }
            let forced_preserve = prior
                .context
                .pointer("/privacy/forcedPreserveInfo")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let privacy = requested_privacy.normalized_for_source(forced_preserve);
            if prior.context.pointer("/privacy/dropSenderNames").and_then(serde_json::Value::as_bool)
                != Some(privacy.drop_sender_names)
                || prior.context.pointer("/privacy/dropCaptions").and_then(serde_json::Value::as_bool)
                    != Some(privacy.drop_captions)
            {
                return Err("forwardHumanMessage clientNonce already identifies different privacy".into());
            }
            let prior_comment = prior.context.get("comment");
            if prior_comment != comment_json.as_ref() {
                return Err("forwardHumanMessage clientNonce already identifies a different comment".into());
            }
            if !prior.contains_requested(&destination_conversation_ids) {
                return Err("forwardHumanMessage request may not expand its original destination set".into());
            }
        } else {
            let source_entry = self.authorize_forward_source(source_conversation_id, source_entry_id)
                .map_err(|error| error.to_string())?;
            let force_preserve = Self::source_entry_requires_preserving_forward_info(&source_entry);
            let privacy = requested_privacy.normalized_for_source(force_preserve);
            let source_text = source_entry.get("content").and_then(serde_json::Value::as_str).unwrap_or_default();
            let attachments = source_entry.get("attachments")
                .and_then(serde_json::Value::as_array).cloned().unwrap_or_default();
            let source_owner = self.open_human_conversation_db_owner(source_conversation_id)?;
            let attachments = snapshot_forward_attachments(
                &source_owner, &account, &issuer, client_nonce, &attachments,
            )?;
            if self.human_forward_scope()? != (account.clone(), issuer.clone()) {
                return Err("Fabushi native messaging credential changed Human account or issuer".into());
            }
            let text = if privacy.drop_captions && !attachments.is_empty() {
                String::new()
            } else {
                source_text.to_string()
            };
            let mut privacy_json = serde_json::json!({
                "dropSenderNames": privacy.drop_sender_names,
                "dropCaptions": privacy.drop_captions,
            });
            if force_preserve {
                privacy_json
                    .as_object_mut()
                    .expect("privacy object")
                    .insert("forcedPreserveInfo".into(), serde_json::Value::Bool(true));
            }
            let mut context = serde_json::json!({
                "batchVersion": 1,
                "requestId": client_nonce,
                "sourceConversationId": source_conversation_id,
                "sourceEntryId": source_entry_id,
                "privacy": privacy_json,
            });
            if let Some(ref c_val) = comment_json {
                context
                    .as_object_mut()
                    .expect("forward context object")
                    .insert("comment".into(), c_val.clone());
            }
            let mut destination_topics = serde_json::Map::new();
            for d in &unique_destinations {
                if let Some(ref tid) = d.topic_id {
                    destination_topics.insert(d.conversation_id.clone(), serde_json::Value::String(tid.clone()));
                }
            }
            if !destination_topics.is_empty() {
                context
                    .as_object_mut()
                    .expect("forward context object")
                    .insert("destinationTopics".into(), serde_json::Value::Object(destination_topics));
            }
            let source_metadata = source_owner.read_metadata().ok();
            if let Some(origin) = Self::resolve_human_forward_origin(
                &source_entry,
                source_metadata.as_ref(),
                &account,
                privacy,
            ) {
                context
                    .as_object_mut()
                    .expect("forward context object")
                    .insert("origin".into(), origin);
            }
            let mut batch = HumanForwardBatch::new(
                account, issuer, client_nonce.to_string(), source_conversation_id.to_string(),
                source_entry_id.to_string(), text, attachments, context, &destination_conversation_ids,
            );
            if comment_json.is_some() {
                for target in &mut batch.destinations {
                    target.comment_nonce = Some(format!("{}:comment", target.client_nonce));
                    target.comment_completed = false;
                }
            }
            store_forward_batch(&source_owner, &batch)?;
        }
        drop(stage_guard);
        let batch = self.deliver_forward_batch(source_conversation_id, client_nonce, Some(&destination_conversation_ids))?;
        Ok(batch.response(&destination_conversation_ids))
    }

    pub fn forward_human_message_with_comment(
        &self,
        source_conversation_id: &str,
        source_entry_id: &str,
        destination_conversation_ids: &[String],
        client_nonce: &str,
        drop_sender_names: bool,
        drop_captions: bool,
        comment: Option<fabushi_messaging_core::FormattedText>,
    ) -> Result<serde_json::Value, String> {
        let destinations: Vec<HumanForwardDestination> = destination_conversation_ids
            .iter()
            .map(|id| HumanForwardDestination {
                conversation_id: id.clone(),
                topic_id: None,
            })
            .collect();
        self.forward_human_message_to_destinations(
            source_conversation_id,
            source_entry_id,
            &destinations,
            client_nonce,
            drop_sender_names,
            drop_captions,
            comment,
        )
    }

}
