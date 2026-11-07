//! Telegram / TDRP Dialog State, Conversation Creation, Metadata Recovery, and Directory Listing.

use super::*;

impl ProductionSessionWorkers {
    pub fn create_human_conversation(
        &self,
        peer_human_id: &str,
        title: &str,
    ) -> Result<serde_json::Value, String> {
        self.create_human_conversation_with_recovery(peer_human_id, title, None)
    }

    pub(crate) fn create_human_conversation_with_recovery(
        &self,
        peer_human_id: &str,
        title: &str,
        recovery: Option<&HumanSyncRecoveryContext>,
    ) -> Result<serde_json::Value, String> {
        self.check_human_sync_continuation(recovery)?;
        let local_human_id = self.local_human_id()?;
        let peer_human_id = peer_human_id.trim();
        let title = title.trim();
        if peer_human_id.is_empty() {
            return Err("human conversation requires peerHumanId".into());
        }
        if local_human_id == peer_human_id {
            return Err("human conversation participants must be distinct".into());
        }
        let conversation_id = Self::human_direct_conversation_id(local_human_id, peer_human_id);
        let serial = self.human_message_lock(&conversation_id)?;
        #[cfg(test)]
        self.observe_human_creation(HumanCreationStage::BeforeLock, &conversation_id, None);
        let _exclusive = serial.lock()
            .map_err(|_| "Human conversation serial lock poisoned".to_string())?;
        #[cfg(test)]
        self.observe_human_creation(HumanCreationStage::Locked, &conversation_id, None);
        // A continuation can wait behind another creator; its old pre-lock
        // credential check cannot authorize the following filesystem writes.
        self.check_human_sync_continuation(recovery)?;

        // The canonical path is shared by every discovery of this pair. Keep
        // creation in the same mutation lane as send/sync through publication.
        let db_path = get_native_conversation_db_path(&self.agents_root, &conversation_id)
            .map_err(|error| error.to_string())?;
        let key = Self::native_conversation_owner_key(&conversation_id);
        let cached_owner = self.db_owners
            .lock()
            .map_err(|_| "conversation db owner map poisoned".to_string())?
            .get(&key)
            .cloned();
        let owner = match cached_owner {
            Some(owner) => owner,
            None => {
                let conversation_dir = db_path
                    .parent()
                    .ok_or_else(|| "native conversation database has no parent".to_string())?;
                fs::create_dir_all(conversation_dir).map_err(|error| error.to_string())?;
                Arc::new(
                    SandAgentDb::open_human_conversation_store(&db_path, self.busy_timeout_ms)
                        .map_err(|error| error.to_string())?,
                )
            }
        };
        // A schema-only store left by an interrupted/failed initialization is
        // retryable. read_metadata alone would synthesize Agent defaults here.
        if owner.read_kv("metadata").map_err(|error| error.to_string())?.is_none() {
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
            #[cfg(test)]
            self.observe_human_creation(
                HumanCreationStage::BeforeInitialize, &conversation_id, Some(&owner),
            );
            self.check_human_sync_continuation(recovery)?;
            // false can mean existing metadata or a busy write, never ownership
            // of this directory. Validate durable metadata below in either case.
            let _ = owner.initialize_human_metadata_if_empty(metadata)
                .map_err(|error| error.to_string())?;
        }
        let conversation = Self::human_creation_summary(
            &owner, &conversation_id, local_human_id, peer_human_id,
        )?;
        #[cfg(test)]
        self.observe_human_creation(
            HumanCreationStage::BeforePublish, &conversation_id, Some(&owner),
        );
        self.check_human_sync_continuation(recovery)?;
        // Read-side openers do not take the mutation lock and may have published
        // a valid owner after initialization. Retain that Arc; dropping only our
        // unpublished candidate must never close or replace the cached winner.
        let published_owner = {
            let mut owners = self.db_owners.lock()
                .map_err(|_| "conversation db owner map poisoned".to_string())?;
            Arc::clone(owners.entry(key).or_insert_with(|| Arc::clone(&owner)))
        };
        if Arc::ptr_eq(&owner, &published_owner) {
            Ok(conversation)
        } else {
            Self::human_creation_summary(
                &published_owner, &conversation_id, local_human_id, peer_human_id,
            )
        }
    }

    pub(crate) fn human_creation_summary(
        owner: &SandAgentDb,
        conversation_id: &str,
        local_human_id: &str,
        peer_human_id: &str,
    ) -> Result<serde_json::Value, String> {
        let Some(raw) = owner.read_kv("metadata").map_err(|error| error.to_string())? else {
            return Err("failed to initialize native Human conversation metadata".into());
        };
        // The shared metadata decoder reads hex byte pairs. Reject non-ASCII
        // corruption before it can slice through a UTF-8 character boundary.
        if !raw.trim().is_ascii() {
            return Err("existing Human conversation metadata encoding is invalid".into());
        }
        let metadata = owner.read_metadata().map_err(|error| error.to_string())?;
        if metadata.get("conversationKind").and_then(serde_json::Value::as_str) != Some("human") {
            return Err(format!("Conversation is not a Human conversation: {conversation_id}"));
        }
        if metadata.get("conversationId").and_then(serde_json::Value::as_str) != Some(conversation_id) {
            return Err("existing Human conversation identity does not match its canonical path".into());
        }
        let participants = metadata.get("participantIds")
            .and_then(serde_json::Value::as_array)
            .filter(|participants| participants.len() == 2 && participants.iter().all(serde_json::Value::is_string))
            .ok_or_else(|| "existing Human conversation participants are invalid".to_string())?;
        if !participants.iter().any(|id| id.as_str() == Some(local_human_id)) {
            return Err("existing Human conversation does not contain the local identity".into());
        }
        if !participants.iter().any(|id| id.as_str() == Some(peer_human_id)) {
            return Err("existing Human conversation does not contain the requested peer".into());
        }
        let title = match metadata.get("title") {
            Some(serde_json::Value::String(title)) => title.as_str(),
            None => peer_human_id,
            Some(_) => return Err("existing Human conversation title is invalid".into()),
        };
        Ok(serde_json::json!({
            "id": conversation_id,
            "kind": "human",
            "title": title,
            "participantIds": participants,
        }))
    }

    #[cfg(test)]
    pub(crate) fn observe_human_creation(
        &self,
        stage: HumanCreationStage,
        conversation_id: &str,
        owner: Option<&Arc<SandAgentDb>>,
    ) {
        if let Some(hook) = &self.human_creation_hook {
            hook(stage, conversation_id, owner);
        }
    }

    pub(crate) fn read_human_conversation_metadata(
        owner: &SandAgentDb,
        conversation_id: &str,
        require_canonical_pair: bool,
    ) -> Result<serde_json::Value, String> {
        let raw = owner.read_kv("metadata").map_err(|error| error.to_string())?
            .ok_or_else(|| "existing Human conversation metadata is missing".to_string())?;
        // Match the creation boundary before the shared hex decoder can slice
        // corrupt UTF-8. Missing identity must never become Agent defaults.
        if !raw.trim().is_ascii() {
            return Err("existing Human conversation metadata encoding is invalid".into());
        }
        let metadata = owner.read_metadata().map_err(|error| error.to_string())?;
        let kind = metadata.get("conversationKind").and_then(serde_json::Value::as_str);
        if metadata.get("conversationId").and_then(serde_json::Value::as_str) != Some(conversation_id) {
            return Err(format!("Conversation is not the requested conversation: {conversation_id}"));
        }
        if kind == Some("group") || kind == Some("channel") {
            let expected_prefix = if kind == Some("group") { "group-" } else { "channel-" };
            if !conversation_id.starts_with(expected_prefix) {
                return Err(format!("Conversation ID does not match expected prefix: {conversation_id}"));
            }
            let participants = metadata.get("participantIds").and_then(serde_json::Value::as_array)
                .ok_or_else(|| "existing conversation participants are invalid".to_string())?;
            if participants.is_empty()
                || participants.iter().any(|value| value.as_str()
                    .is_none_or(|id| id.trim().is_empty() || id.trim() != id))
            {
                return Err("existing conversation participants are invalid".into());
            }
            return Ok(metadata);
        }
        if kind != Some("human") {
            return Err(format!("Conversation is not the requested Human conversation: {conversation_id}"));
        }
        let participants = metadata.get("participantIds").and_then(serde_json::Value::as_array)
            .ok_or_else(|| "existing Human conversation participants are invalid".to_string())?;
        if participants.len() != 2
            || participants.iter().any(|value| value.as_str()
                .is_none_or(|id| id.trim().is_empty() || id.trim() != id))
            || participants[0] == participants[1]
        {
            return Err("existing Human conversation participants are invalid".into());
        }
        let first = participants[0].as_str().expect("validated Human participant");
        let second = participants[1].as_str().expect("validated Human participant");
        if require_canonical_pair && Self::human_direct_conversation_id(first, second) != conversation_id {
            return Err("existing Human conversation participants conflict with the canonical identity".into());
        }
        Ok(metadata)
    }

    pub fn open_human_conversation_db_owner(
        &self,
        conversation_id: &str,
    ) -> Result<Arc<SandAgentDb>, String> {
        self.open_human_conversation_db_owner_with_pair_policy(conversation_id, true)
    }

    // This private opener is reserved for retained forwarding-journal discovery
    // and denial settlement. It grants no ordinary transcript/resource authority:
    // authorize_forward_source rechecks the canonical pair before reading a row.
    pub(crate) fn open_human_forward_recovery_owner(
        &self,
        conversation_id: &str,
    ) -> Result<Arc<SandAgentDb>, String> {
        self.open_human_conversation_db_owner_with_pair_policy(conversation_id, false)
    }

    pub(crate) fn open_human_conversation_db_owner_with_pair_policy(
        &self,
        conversation_id: &str,
        require_canonical_pair: bool,
    ) -> Result<Arc<SandAgentDb>, String> {
        let db_path = get_native_conversation_db_path(&self.agents_root, conversation_id)
            .map_err(|error| error.to_string())?;
        let directory = db_path.parent()
            .ok_or_else(|| "native Human conversation database has no parent".to_string())?;
        let directory_type = fs::symlink_metadata(directory).map_err(|error| error.to_string())?;
        if directory_type.file_type().is_symlink() || !directory_type.is_dir() {
            return Err(format!("Human conversation parent is not a real directory: {conversation_id}"));
        }
        let file_type = fs::symlink_metadata(&db_path).map_err(|error| error.to_string())?;
        if file_type.file_type().is_symlink() || !file_type.is_file() {
            return Err(format!("Human conversation store is not a regular non-symlink file: {conversation_id}"));
        }
        let key = Self::native_conversation_owner_key(conversation_id);
        let mut owners = self
            .db_owners
            .lock()
            .map_err(|_| "conversation db owner map poisoned".to_string())?;
        if let Some(owner) = owners.get(&key).cloned() {
            // An independent SQLite writer may have changed this source after
            // the owner was cached. Cached handles cannot bypass the same
            // conversation identity check as a freshly opened source.
            drop(owners);
            Self::read_human_conversation_metadata(&owner, conversation_id, require_canonical_pair)?;
            return Ok(owner);
        }
        let owner = Arc::new(
            SandAgentDb::open_human_conversation_store(&db_path, self.busy_timeout_ms)
                .map_err(|error| error.to_string())?,
        );
        if let Err(error) = Self::read_human_conversation_metadata(&owner, conversation_id, require_canonical_pair) {
            owner.close(false);
            return Err(error);
        }
        owners.insert(key, Arc::clone(&owner));
        Ok(owner)
    }

    pub(crate) fn metadata_history_coverage_incomplete(metadata: &serde_json::Value) -> bool {
        metadata
            .get("humanHistoryCoverage")
            .and_then(|coverage| coverage.get("complete"))
            .and_then(serde_json::Value::as_bool)
            == Some(false)
    }

    pub fn human_conversation_coverage_incomplete(
        &self,
        conversation_id: &str,
    ) -> Result<bool, String> {
        let owner = self.open_human_conversation_db_owner(conversation_id)?;
        let metadata = owner.read_metadata().map_err(|error| error.to_string())?;
        if !self.metadata_has_local_human(&metadata)? {
            return Err("local Human identity is not a conversation participant".into());
        }
        Ok(Self::metadata_history_coverage_incomplete(&metadata))
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
            let kind = metadata
                .get("conversationKind")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("human");
            let mut conversation = serde_json::json!({
                "id": id,
                "kind": kind,
                "title": title,
                "participantIds": participant_ids,
                "updatedAt": updated_at,
            });
            if Self::metadata_history_coverage_incomplete(&metadata) {
                if let Some(object) = conversation.as_object_mut() {
                    object.insert("historyCoverageComplete".into(), serde_json::json!(false));
                }
            }
            conversations.push(conversation);
        }
        conversations.sort_by(|left, right| {
            left.get("id")
                .and_then(serde_json::Value::as_str)
                .cmp(&right.get("id").and_then(serde_json::Value::as_str))
        });
        Ok(conversations)
    }

    pub(crate) fn source_entry_requires_preserving_forward_info(
        source_entry: &serde_json::Value,
    ) -> bool {
        source_entry
            .get("forcedForwardInfo")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
            || source_entry
                .get("forcePreserveForwardInfo")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
    }

    pub(crate) fn human_recipient_requirements_from_entry(
        source_entry: &serde_json::Value,
    ) -> fabushi_messaging_core::RecipientSearchRequirements {
        let has_non_null = |key: &str| source_entry.get(key).is_some_and(|v| !v.is_null());
        let has_non_empty_str = |key: &str| {
            source_entry
                .get(key)
                .and_then(serde_json::Value::as_str)
                .is_some_and(|s| !s.trim().is_empty())
        };
        let has_game = has_non_null("game");
        let has_other_content = has_game
            || has_non_null("sticker")
            || has_non_null("story")
            || has_non_null("giveaway")
            || has_non_null("location")
            || has_non_null("venue")
            || has_non_null("contact")
            || has_non_null("dice");
        let has_inline_content = has_non_null("miniApp")
            || has_non_empty_str("viaBotId")
            || has_non_empty_str("inlineBotId");
        fabushi_messaging_core::RecipientSearchRequirements {
            require_media: source_entry
                .get("attachments")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|attachments| !attachments.is_empty())
                || source_entry
                    .get("requireMedia")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            require_polls: source_entry
                .get("requirePolls")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
                || has_non_null("poll"),
            require_send_other: source_entry
                .get("requireSendOther")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
                || has_other_content,
            require_inline: source_entry
                .get("requireInline")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
                || has_inline_content,
            require_game: source_entry
                .get("requireGame")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
                || has_game,
            source_protected_content: source_entry
                .get("protectedContent")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            content_kind: if source_entry
                .get("secretContent")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
            {
                fabushi_messaging_core::RecipientContentKind::Secret
            } else {
                fabushi_messaging_core::RecipientContentKind::Standard
            },
            ..fabushi_messaging_core::RecipientSearchRequirements::default()
        }
    }

    pub(crate) fn human_recipient_authorization_from_metadata(
        metadata: &serde_json::Value,
        sender_is_participant: bool,
    ) -> fabushi_messaging_core::RecipientAuthorizationInput {
        let mut input =
            fabushi_messaging_core::RecipientAuthorizationInput::direct_default(
                sender_is_participant,
            );
        if let Some(permissions_val) = metadata.get("permissions") {
            if let Ok(parsed) = serde_json::from_value::<
                fabushi_messaging_core::ConversationPermissions,
            >(permissions_val.clone())
            {
                input.can_send_messages = parsed.can_send_messages;
                input.can_send_media = parsed.can_send_media;
                input.can_send_polls = parsed.can_send_polls;
                input.can_send_other = parsed.can_send_other;
                input.can_send_inline = parsed.can_send_inline;
                input.can_send_games = parsed.can_send_games;
            } else if let Some(permissions) = permissions_val.as_object() {
                if let Some(v) = permissions
                    .get("canSendMessages")
                    .and_then(serde_json::Value::as_bool)
                {
                    input.can_send_messages = v;
                }
                if let Some(v) = permissions
                    .get("canSendMedia")
                    .and_then(serde_json::Value::as_bool)
                {
                    input.can_send_media = v;
                }
                if let Some(v) = permissions
                    .get("canSendPolls")
                    .and_then(serde_json::Value::as_bool)
                {
                    input.can_send_polls = v;
                }
                if let Some(v) = permissions
                    .get("canSendOther")
                    .and_then(serde_json::Value::as_bool)
                {
                    input.can_send_other = v;
                }
                if let Some(v) = permissions
                    .get("canSendInline")
                    .and_then(serde_json::Value::as_bool)
                {
                    input.can_send_inline = v;
                }
                if let Some(v) = permissions
                    .get("canSendGames")
                    .and_then(serde_json::Value::as_bool)
                {
                    input.can_send_games = v;
                }
            }
        }
        if let Some(restrictions_val) = metadata.get("restrictions") {
            if let Ok(parsed) = serde_json::from_value::<
                fabushi_messaging_core::MemberRestrictions,
            >(restrictions_val.clone())
            {
                input.community_restrict_messages = parsed.send_messages;
                input.community_restrict_media = parsed.send_media;
                input.community_restrict_polls = parsed.send_polls;
                input.community_restrict_other = parsed.send_other;
                input.community_restrict_inline = parsed.send_inline;
                input.community_restrict_games = parsed.send_games;
            }
            if let Some(restrictions) = restrictions_val.as_object() {
                if let Some(v) = restrictions
                    .get("sendMessages")
                    .and_then(serde_json::Value::as_bool)
                {
                    input.community_restrict_messages = v;
                }
                if let Some(v) = restrictions
                    .get("sendMedia")
                    .and_then(serde_json::Value::as_bool)
                {
                    input.community_restrict_media = v;
                }
                if let Some(v) = restrictions
                    .get("sendPolls")
                    .and_then(serde_json::Value::as_bool)
                {
                    input.community_restrict_polls = v;
                }
                if let Some(v) = restrictions
                    .get("sendOther")
                    .and_then(serde_json::Value::as_bool)
                {
                    input.community_restrict_other = v;
                }
                if let Some(v) = restrictions
                    .get("sendInline")
                    .and_then(serde_json::Value::as_bool)
                {
                    input.community_restrict_inline = v;
                }
                if let Some(v) = restrictions
                    .get("sendGames")
                    .and_then(serde_json::Value::as_bool)
                {
                    input.community_restrict_games = v;
                }
            }
        }
        if let Some(v) = metadata
            .get("communityForbidden")
            .and_then(serde_json::Value::as_bool)
        {
            input.community_forbidden = v;
        }
        if let Some(v) = metadata
            .get("channelPostingAllowed")
            .and_then(serde_json::Value::as_bool)
        {
            input.channel_posting_allowed = v;
        }
        if metadata
            .get("isBroadcastChannel")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
            || metadata
                .get("conversationSubkind")
                .and_then(serde_json::Value::as_str)
                == Some("channel")
        {
            input.conversation_kind = fabushi_messaging_core::ConversationKind::Channel;
        }
        if metadata
            .get("secret")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
            || metadata
                .get("conversationSubkind")
                .and_then(serde_json::Value::as_str)
                == Some("secret")
        {
            input.conversation_kind = fabushi_messaging_core::ConversationKind::Secret;
        }
        input
    }

}
