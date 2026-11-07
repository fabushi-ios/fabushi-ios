//! Telegram / TDRP Message Forwarding, Batch Distribution, Share Comments, and Recipient Resolution.

use super::*;


#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HumanForwardDestination {
    pub conversation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic_id: Option<String>,
}

impl ProductionSessionWorkers {
    pub fn pending_human_forward_is_batch_owned(
        &self, conversation_id: &str, entry: &serde_json::Value,
    ) -> Result<bool, String> {
        // A damaged version must not turn source-owned intent into an ordinary
        // send. The separate marker also survives replacement of its context.
        if entry.get("forwardBatchOwned").is_some() {
            return Ok(true);
        }
        let context = entry.get("forwardContext");
        if context.is_some_and(|value| value.get("batchVersion").is_some()) {
            return Ok(true);
        }
        let nonce = entry.get("clientNonce").and_then(serde_json::Value::as_str)
            .ok_or_else(|| "persisted Human pending nonce is invalid".to_string())?;
        let author = entry.get("authorId").and_then(serde_json::Value::as_str)
            .ok_or_else(|| "persisted Human pending author is invalid".to_string())?;
        let mut legacy_source_reference = None;
        if let Some(context) = context.filter(|value|
            ["sourceConversationId", "sourceEntryId", "requestId"].iter()
                .any(|key| value.get(*key).is_some())
        ) {
            let source = context.get("sourceConversationId").and_then(serde_json::Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "persisted Human forward source is invalid".to_string())?;
            let request = context.get("requestId").and_then(serde_json::Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "persisted Human forward request is invalid".to_string())?;
            // This is journal-only ownership discovery. A rebound source must remain
            // discoverable so generic outbox replay cannot bypass its denial;
            // it grants no transcript or resource authority.
            let owner = self.open_human_forward_recovery_owner(source)?;
            if load_forward_batches(&owner, source)?.iter().any(|batch|
                (batch.request_id == request && batch.account_id == author)
                    || batch.destinations.iter().any(|target|
                        target.conversation_id == conversation_id && target.client_nonce == nonce
                    )
            ) {
                return Ok(true);
            }
            // The pre-journal shipping path used this exact nonce contract.
            // A readable but unrelated source alone cannot authorize downgrade
            // of a v1 target whose real source/journal is no longer available.
            let mut legacy_digest = Sha256::new();
            legacy_digest.update(request.as_bytes());
            legacy_digest.update([0_u8]);
            legacy_digest.update(conversation_id.as_bytes());
            if nonce == format!("human-forward-{:x}", legacy_digest.finalize()) {
                let source_entry = context.get("sourceEntryId").and_then(serde_json::Value::as_str)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| "persisted Human forward source entry is invalid".to_string())?;
                legacy_source_reference = Some((source, source_entry));
            }
        }
        // Older rows did not have the independent marker. If their entire
        // context was lost/replaced, their stable target nonce still identifies
        // the source batch. Unrelated ordinary reply metadata is not itself a
        // forwarding identity, and genuine legacy nonces remain replayable.
        if !nonce.starts_with("human-forward-") { return Ok(false); }
        let entries = match fs::read_dir(get_native_conversations_root(&self.agents_root)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err("persisted Human forward ownership cannot be resolved from unavailable source stores".into());
            }
            Err(error) => return Err(error.to_string()),
        };
        for source in entries {
            let source = source.map_err(|error| error.to_string())?;
            if !source.file_type().map_err(|error| error.to_string())?.is_dir() { continue; }
            let source_id = source.file_name().to_string_lossy().into_owned();
            let db_path = get_native_conversation_db_path(&self.agents_root, &source_id)
                .map_err(|error| error.to_string())?;
            if !db_path.is_file() { continue; }
            let owner = self.open_human_forward_recovery_owner(&source_id)?;
            if load_forward_batches(&owner, &source_id)?.iter().any(|batch|
                batch.destinations.iter().any(|target|
                    target.conversation_id == conversation_id && target.client_nonce == nonce
                )
            ) {
                return Ok(true);
            }
        }
        let Some((source, source_entry)) = legacy_source_reference else {
            return Err("persisted Human forward ownership is unresolved; retain pending intent until its source can be recovered".into());
        };
        // Legacy compatibility is not a permission bypass. Use the same
        // current source policy as new batches before ordinary outbox replay;
        // retain the original pending payload and report any source denial.
        self.authorize_forward_source(source, source_entry)
            .map_err(|error| error.to_string())?;
        Ok(false)
    }


    pub fn refresh_human_conversation_recipient_policy(
        &self,
        conversation_id: &str,
        permissions: fabushi_messaging_core::ConversationPermissions,
        restrictions: Option<fabushi_messaging_core::MemberRestrictions>,
    ) -> Result<(), String> {
        let owner = self.open_human_conversation_db_owner(conversation_id)?;
        let metadata = owner.read_metadata().map_err(|error| error.to_string())?;
        if !self.metadata_has_local_human(&metadata)? {
            return Err("local Human identity is not a conversation participant".into());
        }
        let permissions_json =
            serde_json::to_value(permissions).map_err(|error| error.to_string())?;
        owner
            .set_metadata("permissions", permissions_json)
            .map_err(|error| error.to_string())?;
        if let Some(restrictions) = restrictions {
            let restrictions_json =
                serde_json::to_value(restrictions).map_err(|error| error.to_string())?;
            owner
                .set_metadata("restrictions", restrictions_json)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
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
        let requirements = Self::human_recipient_requirements_from_entry(&source_entry);
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
                let auth_input = conversation
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|id| self.open_human_conversation_db_owner(id).ok())
                    .and_then(|owner| owner.read_metadata().ok())
                    .map(|metadata| {
                        Self::human_recipient_authorization_from_metadata(
                            &metadata,
                            sender_is_participant,
                        )
                    })
                    .unwrap_or_else(|| {
                        fabushi_messaging_core::RecipientAuthorizationInput::direct_default(
                            sender_is_participant,
                        )
                    });
                if fabushi_messaging_core::authorize_destination(
                    auth_input,
                    requirements,
                )
                .is_err()
                {
                    return false;
                }
                if normalized_query.is_empty() {
                    return true;
                }
                let mut fields = Vec::new();
                if let Some(title) = conversation
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                {
                    fields.push(title);
                }
                if let Some(ids) = conversation
                    .get("participantIds")
                    .and_then(serde_json::Value::as_array)
                {
                    for id in ids.iter().filter_map(serde_json::Value::as_str) {
                        fields.push(id);
                    }
                }
                fabushi_messaging_core::query_matches_fields(&normalized_query, &fields)
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

    /// The same private Native credential owner supplies account and issuer;
    /// a token or device-session refresh does not rewrite durable intent.
    pub(crate) fn human_forward_scope(&self) -> Result<(String, String), String> {
        let account = self.local_human_id()?.to_string();
        let issuer = if let Some(client) = self.shipping_native_messaging()? {
            let (bound_account, issuer) = client.forward_account_scope()?;
            if bound_account != account {
                return Err("Fabushi native messaging credential changed Human account".into());
            }
            issuer
        } else {
            "local-only".to_string()
        };
        Ok((account, issuer))
    }

    /// Scan even now-ineligible source records, so removing a conversation
    /// cannot free its old request ID for another source under one account.
    pub(crate) fn find_forward_request(
        &self, account: &str, issuer: &str, request_id: &str,
    ) -> Result<Option<HumanForwardBatch>, String> {
        let root = get_native_conversations_root(&self.agents_root);
        let entries = match fs::read_dir(root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        let mut matched: Option<HumanForwardBatch> = None;
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            if kind.is_symlink() {
                return Err("Human forward source directory is a symbolic link; retained identity cannot be skipped".into());
            }
            if !kind.is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            // A retained directory with unavailable storage may own this
            // request identity. Never free it for another source by skipping
            // a failed stat/open; retry only after storage recovers.
            let owner = self.open_human_forward_recovery_owner(&id)?;
            for batch in load_forward_batches(&owner, &id)? {
                if batch.account_id == account && batch.issuer == issuer && batch.request_id == request_id {
                    if matched.is_some() {
                        return Err("Human forward request identity is duplicated across source conversations".into());
                    }
                    matched = Some(batch);
                }
            }
        }
        Ok(matched)
    }

    pub(crate) fn authorize_forward_source(
        &self, source_id: &str, source_entry_id: &str,
    ) -> Result<serde_json::Value, ForwardSourceFailure> {
        let owner = self.open_human_forward_recovery_owner(source_id)
            .map_err(ForwardSourceFailure::Storage)?;
        let metadata = Self::read_human_conversation_metadata(&owner, source_id, false)
            .map_err(ForwardSourceFailure::Storage)?;
        let kind = metadata.get("conversationKind").and_then(serde_json::Value::as_str).unwrap_or("human");
        if kind == "human" {
            let participants = metadata.get("participantIds").and_then(serde_json::Value::as_array)
                .ok_or_else(|| ForwardSourceFailure::Storage("existing Human conversation participants are invalid".into()))?;
            let first = participants[0].as_str().expect("structurally validated Human participant");
            let second = participants[1].as_str().expect("structurally validated Human participant");
            if Self::human_direct_conversation_id(first, second) != source_id {
                return Err(ForwardSourceFailure::Policy(
                    "forwardHumanMessage source participant pair no longer matches its canonical identity".into()));
            }
        } else if !matches!(kind, "group" | "channel") {
            return Err(ForwardSourceFailure::Policy("invalid conversation kind".into()));
        }
        if !self.metadata_has_local_human(&metadata).map_err(ForwardSourceFailure::Storage)? {
            return Err(ForwardSourceFailure::Policy(
                "local Human identity is not a participant in the source conversation".into()));
        }
        let entry = owner.get_transcript_entries()
            .map_err(|error| ForwardSourceFailure::Storage(error.to_string()))?
            .into_iter()
            .find(|item| item.get("id").and_then(serde_json::Value::as_str) == Some(source_entry_id))
            .ok_or_else(|| ForwardSourceFailure::Policy(
                "forwardHumanMessage source message is missing".into()))?;
        if entry.get("kind").and_then(serde_json::Value::as_str) != Some("message")
            || entry.get("delivery").and_then(serde_json::Value::as_str) != Some("sent")
        {
            return Err(ForwardSourceFailure::Policy(
                "forwardHumanMessage source must be a settled canonical message".into()));
        }
        if entry.get("protectedContent").and_then(serde_json::Value::as_bool) == Some(true) {
            return Err(ForwardSourceFailure::Policy(
                "forwardHumanMessage source is protected and cannot be forwarded".into()));
        }
        if entry.get("content").and_then(serde_json::Value::as_str).unwrap_or_default().trim().is_empty()
            && !entry.get("attachments").and_then(serde_json::Value::as_array).is_some_and(|a| !a.is_empty())
        {
            return Err(ForwardSourceFailure::Policy(
                "forwardHumanMessage source has no forwardable content".into()));
        }
        Ok(entry)
    }

    pub(crate) fn deliver_forward_batch(
        &self, source_id: &str, request_id: &str, requested: Option<&[String]>,
    ) -> Result<HumanForwardBatch, String> {
        self.deliver_forward_batch_from_intent(source_id, request_id, requested, false)
    }

    pub(crate) fn deliver_forward_batch_from_intent(
        &self, source_id: &str, request_id: &str, requested: Option<&[String]>, require_existing: bool,
    ) -> Result<HumanForwardBatch, String> {
        let (account, issuer) = self.human_forward_scope()?;
        let serial = self.human_message_lock(&format!("forward-batch:{account}:{issuer}:{request_id}"))?;
        let exclusive = serial.lock().map_err(|_| "Human forward batch lock poisoned".to_string())?;
        self.deliver_forward_batch_while_locked(
            source_id, request_id, requested, require_existing, &account, &issuer, &exclusive,
        )
    }

    fn deliver_forward_batch_while_locked(
        &self, source_id: &str, request_id: &str, requested: Option<&[String]>, require_existing: bool,
        account: &str, issuer: &str, _exclusive: &std::sync::MutexGuard<'_, ()>,
    ) -> Result<HumanForwardBatch, String> {
        // This lane delivers an existing account/issuer-bound journal. Its
        // source may be revoked: load the retained intent and let per-target
        // authorization below persist its denial without granting row authority.
        // require_existing applies only to the destination outbox, so automatic
        // recovery can still dispatch a previously unsubmitted target.
        let owner = self.open_human_forward_recovery_owner(source_id)?;
        let mut batch = load_forward_batches(&owner, source_id)?.into_iter().find(|item|
            item.account_id == account && item.issuer == issuer && item.request_id == request_id
        ).ok_or_else(|| "forwardHumanMessage durable request was not found".to_string())?;
        if let Some(subset) = requested {
            if !batch.contains_requested(subset) {
                return Err("forwardHumanMessage request may not expand its original destination set".into());
            }
        }
        for index in 0..batch.destinations.len() {
            let target = &batch.destinations[index];
            if target.completed || requested.is_some_and(|subset|
                !subset.iter().any(|id| id == &target.conversation_id)
            ) {
                continue;
            }
            // Reauthorize before *each* network dispatch, not merely when the
            // original snapshot was accepted. The immutable payload never
            // changes when the source text is later edited.
            let source_entry = match self.authorize_forward_source(source_id, &batch.source_entry_id) {
                Ok(entry) => entry,
                Err(ForwardSourceFailure::Policy(denial)) => {
                    // Persist a visible denial without sending any remaining
                    // destination. Ordinary inbound history may still sync.
                    for destination in &mut batch.destinations {
                        if !destination.completed {
                            destination.error = Some(denial.clone());
                        }
                    }
                    store_forward_batch(&owner, &batch)?;
                    return Ok(batch);
                }
                Err(ForwardSourceFailure::Storage(error)) => return Err(error),
            };
            if Self::source_entry_requires_preserving_forward_info(&source_entry)
                && (batch.context.pointer("/privacy/dropSenderNames").and_then(serde_json::Value::as_bool) == Some(true)
                    || batch.context.pointer("/privacy/dropCaptions").and_then(serde_json::Value::as_bool) == Some(true))
            {
                let denial = "forwardHumanMessage source requires preserving forward attribution".to_string();
                for destination in &mut batch.destinations {
                    if !destination.completed {
                        destination.error = Some(denial.clone());
                    }
                }
                store_forward_batch(&owner, &batch)?;
                return Ok(batch);
            }
            if self.human_forward_scope()? != (account.to_string(), issuer.to_string()) {
                return Err("Fabushi native messaging credential changed Human account or issuer".into());
            }
            let destination_id = target.conversation_id.clone();
            let nonce = target.client_nonce.clone();
            let result = (|| -> Result<serde_json::Value, String> {
                let destination_owner = self.open_human_conversation_db_owner(&destination_id)?;
                let metadata = destination_owner.read_metadata().map_err(|error| error.to_string())?;
                let kind = metadata.get("conversationKind").and_then(serde_json::Value::as_str).unwrap_or("human");
                if !matches!(kind, "human" | "group" | "channel") || !self.metadata_has_local_human(&metadata)? {
                    return Err("forward destination is not an authorized conversation".into());
                }
                let participants = metadata.get("participantIds").and_then(serde_json::Value::as_array)
                    .ok_or_else(|| "forward destination participants are invalid".to_string())?;
                if kind == "human" && participants.len() != 2 {
                    return Err("forward destination is not an authorized Human conversation".into());
                } else if (kind == "group" || kind == "channel") && participants.is_empty() {
                    return Err("forward destination participants are invalid".into());
                }

                let target_topic_id = batch.context.get("destinationTopics")
                    .and_then(|m| m.get(&destination_id))
                    .and_then(serde_json::Value::as_str);

                let member_opt = if matches!(kind, "group" | "channel") || target_topic_id.is_some() {
                    destination_owner.get_conversation_member(account).map_err(|e| e.to_string())?
                } else {
                    None
                };

                let topic_opt = if let Some(tid) = target_topic_id {
                    destination_owner.get_conversation_topic(tid).map_err(|e| e.to_string())?
                } else {
                    None
                };

                let can_manage_topics = member_opt.as_ref().map(|m| {
                    m.status == fabushi_messaging_core::MemberStatus::Owner
                        || (m.status == fabushi_messaging_core::MemberStatus::Administrator && m.admin_rights.manage_topics)
                }).unwrap_or(false);

                let topic_requested = target_topic_id.is_some();
                let topic_exists = topic_opt.is_some();
                let topic_closed = topic_opt.as_ref().map(|t| t.closed).unwrap_or(false);
                let topic_hidden = topic_opt.as_ref().map(|t| t.hidden).unwrap_or(false);

                if topic_requested && topic_hidden && !can_manage_topics {
                    return Err(format!(
                        "forward destination rejected the topic destination ({:?})",
                        fabushi_messaging_core::DestinationAuthorizationDenial::TopicNotFound
                    ));
                }

                let mut requirements = Self::human_recipient_requirements_from_entry(&source_entry);
                requirements.require_media |= !batch.attachments.is_empty();
                let auth_input = Self::human_recipient_authorization_from_metadata(&metadata, true);

                let eligibility = fabushi_messaging_core::TopicDestinationEligibilityInput {
                    topic_requested,
                    topic_exists,
                    topic_closed,
                    can_manage_topics,
                };
                if let Err(denial) = fabushi_messaging_core::authorize_topic_destination(
                    auth_input,
                    requirements,
                    eligibility,
                ) {
                    return Err(format!(
                        "forward destination rejected the source message policy ({denial:?})"
                    ));
                }

                let conv_kind = match kind {
                    "channel" => fabushi_messaging_core::ConversationKind::Channel,
                    "group" => fabushi_messaging_core::ConversationKind::Group,
                    _ => fabushi_messaging_core::ConversationKind::Direct,
                };
                if matches!(conv_kind, fabushi_messaging_core::ConversationKind::Group | fabushi_messaging_core::ConversationKind::Channel) {
                    let existing_dest_entries = destination_owner.get_transcript_entries().unwrap_or_default();
                    let last_sender_message_at_ms = existing_dest_entries.iter()
                        .filter(|e| e.get("authorId").and_then(serde_json::Value::as_str) == Some(account))
                        .filter_map(|e| e.get("timestampMs").and_then(serde_json::Value::as_f64))
                        .map(|t| t as i64)
                        .max();
                    let slow_mode_seconds = metadata.get("slowModeSeconds").and_then(serde_json::Value::as_u64).map(|v| v as u32);
                    let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
                    let content_flags = fabushi_messaging_core::CommunitySendContentFlags {
                        has_media: !batch.attachments.is_empty(),
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
                        return Err(format!("forward destination rejected by community policy: {denial:?}"));
                    }
                }

                // If this batch has a comment, and it hasn't completed yet for this destination,
                // dispatch the comment as an independent text message first!
                if let Some(comment_val) = batch.context.get("comment") {
                    if !batch.destinations[index].comment_completed {
                        let comment_text = comment_val.get("text").and_then(serde_json::Value::as_str).unwrap_or_default();
                        if !comment_text.is_empty() {
                            let c_nonce = batch.destinations[index].comment_nonce.clone()
                                .unwrap_or_else(|| format!("{}:comment", nonce));
                            self.append_human_message_from_intent(
                                &destination_id, comment_text, &c_nonce, None, None,
                                &[], None, target_topic_id, None, require_existing, false,
                            )?;
                        }
                        batch.destinations[index].comment_completed = true;
                        store_forward_batch(&owner, &batch)?;
                    }
                }

                self.append_human_message_from_intent(
                    &destination_id, &batch.text, &nonce, None, None,
                    &batch.attachments, Some(&batch.context), target_topic_id, None, require_existing, false,
                )
            })();
            match result {
                Ok(_) => {
                    batch.destinations[index].completed = true;
                    batch.destinations[index].error = None;
                }
                Err(error) => {
                    batch.destinations[index].completed = false;
                    batch.destinations[index].error = Some(error);
                }
            }
            // A failed progress write must stop any later destination. A
            // successfully sent destination can replay through its canonical
            // nonce after a crash between its HTTP settlement and this write.
            store_forward_batch(&owner, &batch)?;
        }
        Ok(batch)
    }

    pub(crate) fn resume_human_forward_batches(&self) -> Result<(), String> {
        if self.shipping_native_messaging()?.is_none() { return Ok(()); }
        let (account, issuer) = self.human_forward_scope()?;
        // Ordinary roster discovery is best-effort. Durable forward recovery
        // must instead surface unreadable or malformed stores before dispatch,
        // without turning another account's valid source into local authority.
        let root = get_native_conversations_root(&self.agents_root);
        let entries = match fs::read_dir(root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.to_string()),
        };
        let mut sources = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            if kind.is_symlink() {
                return Err("Human forward source directory is a symbolic link; retained identity cannot be skipped".into());
            }
            if !kind.is_dir() { continue; }
            let id = entry.file_name().to_string_lossy().into_owned();
            let owner = self.open_human_forward_recovery_owner(&id)?;
            // An account-owned intent remains discoverable after source
            // membership is revoked. Delivery persists its policy denial;
            // other accounts' valid batches are filtered below and untouched.
            let batches = load_forward_batches(&owner, &id)?;
            sources.push((id, batches));
        }
        for (source_id, batches) in sources {
            for batch in batches {
                if batch.account_id != account || batch.issuer != issuer
                    || batch.destinations.iter().all(|target| target.completed)
                { continue; }
                // Do not erase or bypass an invalid source on recovery. An
                // unavailable source retains the original intent, and no
                // destination is sent without reauthorization.
                let serial = self.human_message_lock(&format!(
                    "forward-batch:{account}:{issuer}:{}", batch.request_id,
                ))?;
                let exclusive = match serial.try_lock() {
                    Ok(guard) => guard,
                    Err(std::sync::TryLockError::WouldBlock) => continue,
                    Err(std::sync::TryLockError::Poisoned(_)) => {
                        return Err("Human forward batch lock poisoned".to_string());
                    }
                };
                // Keep the admitted guard through delivery. Releasing it here
                // and taking the blocking wrapper would stall history again.
                self.deliver_forward_batch_while_locked(
                    &source_id, &batch.request_id, None, false, &account, &issuer, &exclusive,
                )?;
            }
        }
        Ok(())
    }

}
