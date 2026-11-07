//! Telegram / TDRP Reactions, Remote Reaction Projection, Replay, and History Sync Recovery.

use super::*;

impl ProductionSessionWorkers {
    pub(crate) fn validate_remote_human_reactions(
        reactions: &[FabushiRemoteHumanReaction],
    ) -> Result<(), String> {
        let mut emojis = BTreeSet::new();
        for reaction in reactions {
            if !matches!(
                (reaction.count, reaction.reacted_by_me),
                (0, false) | (1, false) | (1, true) | (2, true)
            ) {
                return Err("remote Human reaction summary is inconsistent with direct participants".into());
            }
            if !emojis.insert(reaction.emoji.as_str()) {
                return Err("remote Human reaction summary contains duplicate emoji".into());
            }
        }
        Ok(())
    }

    // Call only after direct-pair summary admission. Projection work depends on
    // summary items, never on an externally supplied numeric count.
    pub(crate) fn project_remote_human_reactions(
        reactions: &[FabushiRemoteHumanReaction],
        peer_human_id: &str,
    ) -> Vec<serde_json::Value> {
        let mut projected = Vec::new();
        for reaction in reactions {
            if reaction.reacted_by_me {
                projected.push(serde_json::json!({
                    "emoji": reaction.emoji,
                    "by": "me",
                }));
            }
            if reaction.count > u64::from(reaction.reacted_by_me) {
                projected.push(serde_json::json!({
                    "emoji": reaction.emoji,
                    "by": peer_human_id,
                }));
            }
        }
        projected
    }

    // Admission is pure and happens before remote resources or transcript writes.
    // Server request ids are scoped to their actual author, not to this device.

    fn store_human_coverage(owner: &SandAgentDb, after_id: u64, complete: bool) -> Result<(), String> {
        if !owner.set_metadata(
            "humanHistoryCoverage",
            serde_json::json!({"version": 1, "afterId": after_id, "complete": complete}),
        ).map_err(|error| error.to_string())? {
            return Err("Human history coverage checkpoint was not durably stored".into());
        }
        Ok(())
    }

    pub(crate) fn capture_human_sync_recovery(
        &self,
        client: &FabushiNativeMessagingClient,
    ) -> Result<HumanSyncRecoveryContext, String> {
        let local_human_id = self.local_human_id()?.to_string();
        if client.identity()?.user_id != local_human_id {
            return Err("Fabushi native messaging credential changed Human account".into());
        }
        let recovery = HumanSyncRecoveryContext {
            local_human_id,
            credential_fingerprint: client.request_credential_fingerprint()?,
            first_reaction_refusal: None,
        };
        self.check_human_sync_binding(client, &recovery)?;
        Ok(recovery)
    }

    pub(crate) fn check_human_sync_binding(
        &self,
        client: &FabushiNativeMessagingClient,
        recovery: &HumanSyncRecoveryContext,
    ) -> Result<(), String> {
        if self.local_human_id()? != recovery.local_human_id
            || client.identity()?.user_id != recovery.local_human_id
        {
            return Err("Fabushi native messaging credential changed Human account".into());
        }
        if client.request_credential_fingerprint()? != recovery.credential_fingerprint {
            return Err("Fabushi native messaging credential changed during Human request".into());
        }
        Ok(())
    }

    pub(crate) fn check_human_sync_continuation(
        &self,
        recovery: Option<&HumanSyncRecoveryContext>,
    ) -> Result<(), String> {
        let Some(recovery) = recovery.filter(|value| value.is_continuing()) else {
            return Ok(());
        };
        let client = self.shipping_native_messaging()?
            .ok_or_else(|| "Human recovery continuation requires native messaging".to_string())?;
        self.check_human_sync_binding(client, recovery)
    }

    pub(crate) fn revalidate_human_recovery_owner(
        &self,
        conversation_id: &str,
        owner: &SandAgentDb,
        original_metadata: &serde_json::Value,
    ) -> Result<(), String> {
        // A retained response cannot reopen or register a replacement after
        // Session shutdown. Reuse only the original, still registered owner.
        if owner.is_closed() {
            return Err("Human conversation owner is closed".into());
        }
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
        let current = {
            let owners = self.db_owners.lock()
                .map_err(|_| "conversation db owner map poisoned".to_string())?;
            owners.get(&key).cloned()
        }.ok_or_else(|| "Human conversation owner changed during reaction recovery".to_string())?;
        // SandAgentDb resolves relative paths against the current directory
        // when it opens the existing owner. Match that same path contract.
        let expected_owner_path = if db_path.is_absolute() {
            db_path
        } else {
            std::env::current_dir().map_err(|error| error.to_string())?.join(db_path)
        };
        if !std::ptr::eq(current.as_ref(), owner) || owner.db_path() != expected_owner_path.as_path() {
            return Err("Human conversation owner changed during reaction recovery".into());
        }
        if current.is_closed() {
            return Err("Human conversation owner is closed".into());
        }
        let metadata = Self::read_human_conversation_metadata(&current, conversation_id, true)?;
        if !self.metadata_has_local_human(&metadata)? {
            return Err("local Human identity is not a conversation participant".into());
        }
        if metadata.get("participantIds") != original_metadata.get("participantIds") {
            return Err("Human conversation participants changed during reaction recovery".into());
        }
        Ok(())
    }

    pub(crate) fn check_human_history_continuation(
        &self,
        conversation_id: &str,
        owner: &SandAgentDb,
        metadata: &serde_json::Value,
        recovery: Option<&HumanSyncRecoveryContext>,
    ) -> Result<(), String> {
        if recovery.is_some_and(HumanSyncRecoveryContext::is_continuing) {
            self.check_human_sync_continuation(recovery)?;
            self.revalidate_human_recovery_owner(conversation_id, owner, metadata)?;
        }
        Ok(())
    }

    pub fn sync_human_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<serde_json::Value>, String> {
        let client = self.shipping_native_messaging()?;
        let mut recovery = client.map(|client| self.capture_human_sync_recovery(client)).transpose()?;
        // Capture before forward recovery; its completion cannot authorize a
        // replacement credential. Forward delivery still precedes destination locks.
        self.resume_human_forward_batches()?;
        if let (Some(client), Some(recovery)) = (client, recovery.as_ref()) {
            self.check_human_sync_binding(client, recovery)?;
        }
        let entries = self.sync_human_conversation_history(conversation_id, recovery.as_mut())?;
        match recovery {
            Some(recovery) => recovery.finish(entries),
            None => Ok(entries),
        }
    }

    pub(crate) fn sync_human_conversation_history(
        &self,
        conversation_id: &str,
        mut recovery: Option<&mut HumanSyncRecoveryContext>,
    ) -> Result<Vec<serde_json::Value>, String> {
        const PAGE_LIMIT: usize = 200;
        const MAX_GAP_PAGES: usize = 32;
        const MAX_SAFE_ID: u64 = 9_007_199_254_740_991;
        self.check_human_sync_continuation(recovery.as_deref())?;
        let serial = self.human_message_lock(conversation_id)?;
        let _exclusive = serial.lock().map_err(|_| "Human conversation serial lock poisoned".to_string())?;
        self.check_human_sync_continuation(recovery.as_deref())?;
        let owner = self.open_human_conversation_db_owner(conversation_id)?;
        let metadata = owner.read_metadata().map_err(|error| error.to_string())?;
        if !self.metadata_has_local_human(&metadata)? {
            return Err("local Human identity is not a conversation participant".into());
        }
        let Some(client) = self.shipping_native_messaging()? else {
            return owner.get_human_transcript_entries().map_err(|error| error.to_string());
        };
        let local_human_id = self.local_human_id()?.to_string();
        if client.identity()?.user_id != local_human_id {
            return Err("Fabushi native messaging credential changed Human account".into());
        }
        self.check_human_history_continuation(conversation_id, &owner, &metadata, recovery.as_deref())?;
        let coverage = owner.get_metadata("humanHistoryCoverage").map_err(|error| error.to_string())?;
        let (covered, complete) = match coverage.as_ref() {
            None | Some(serde_json::Value::Null) => (0, false),
            Some(checkpoint) => {
                let after_id = checkpoint.get("afterId")
                    .and_then(serde_json::Value::as_u64)
                    .filter(|value| *value <= MAX_SAFE_ID)
                    .ok_or_else(|| "persisted Human history coverage cursor is invalid".to_string())?;
                let complete = checkpoint.get("complete")
                    .and_then(serde_json::Value::as_bool)
                    .ok_or_else(|| "persisted Human history coverage completion flag is invalid".to_string())?;
                match checkpoint.get("version") {
                    Some(version) if version.as_u64() == Some(1) => (after_id, complete),
                    None => {
                        // Older clients persisted this shape without requiring
                        // historyCursor. Preserve their rows and checkpoint until
                        // a qualified origin page is durably applied.
                        (0, false)
                    }
                    Some(_) => return Err("persisted Human history coverage version is unsupported".into()),
                }
            }
        };
        self.replay_human_reaction_intent_with_recovery(
            &owner, &metadata, client, recovery.as_deref_mut(),
        )?;
        self.check_human_history_continuation(conversation_id, &owner, &metadata, recovery.as_deref())?;
        let pending = owner.get_transcript_entries().map_err(|error| error.to_string())?
            .into_iter().filter(|entry| {
                entry.get("authorId").and_then(serde_json::Value::as_str) == Some(local_human_id.as_str())
                    && entry.get("remoteMessageId").is_none()
                    && matches!(entry.get("delivery").and_then(serde_json::Value::as_str), Some("pending" | "dispatching"))
                    && entry.get("clientNonce").and_then(serde_json::Value::as_str).is_some()
            }).collect::<Vec<_>>();
        for entry in pending {
            self.check_human_history_continuation(conversation_id, &owner, &metadata, recovery.as_deref())?;
            // Earlier parents in this ordered replay may have atomically
            // repaired this child's reply edge. Consume the current durable
            // row, not the reply ID captured in the initial pending snapshot.
            let Some(entry) = owner.get_transcript_entries().map_err(|error| error.to_string())?
                .into_iter().find(|current| {
                    current.get("id") == entry.get("id")
                        && current.get("clientNonce") == entry.get("clientNonce")
                        && current.get("authorId").and_then(serde_json::Value::as_str) == Some(local_human_id.as_str())
                        && current.get("remoteMessageId").is_none()
                        && matches!(current.get("delivery").and_then(serde_json::Value::as_str), Some("pending" | "dispatching"))
                }) else { continue; };
            // A source-owned forwarding outbox never becomes a generic send
            // because its version/context was damaged or replaced.
            if self.pending_human_forward_is_batch_owned(conversation_id, &entry)? {
                continue;
            }
            let Some(nonce) = entry.get("clientNonce").and_then(serde_json::Value::as_str) else { continue; };
            let text = entry.get("content").and_then(serde_json::Value::as_str).unwrap_or_default();
            let reply = entry.get("replyToId").and_then(serde_json::Value::as_str);
            let composed = entry.get("composedAtMs").and_then(serde_json::Value::as_f64);
            let attachments = entry.get("attachments").and_then(serde_json::Value::as_array).cloned().unwrap_or_default();
            let result = self.append_human_message_with_context(
                conversation_id, text, nonce, composed, reply, &attachments,
                entry.get("forwardContext"), Some(&_exclusive), false,
            );
            if recovery.as_deref().is_some_and(HumanSyncRecoveryContext::is_continuing) {
                // This branch was unreachable after a refused reaction before.
                // Recheck binding first, then preserve every append error unchanged.
                self.check_human_history_continuation(conversation_id, &owner, &metadata, recovery.as_deref())?;
                result?;
            }
        }
        let peer = self.peer_human_id_from_metadata(&metadata)?;
        let mut after = if complete && covered < PAGE_LIMIT as u64 { 0 } else { covered };
        let mut rejected: Option<String> = None;
        let mut reached_end = false;
        let mut pages_used = 0;
        for _ in 0..MAX_GAP_PAGES {
            self.check_human_history_continuation(conversation_id, &owner, &metadata, recovery.as_deref())?;
            pages_used += 1;
            let page = client.list_direct_messages_after_for_user(&peer, &local_human_id, after, PAGE_LIMIT)?;
            self.check_human_history_continuation(conversation_id, &owner, &metadata, recovery.as_deref())?;
            if client.identity()?.user_id != local_human_id {
                return Err("Fabushi native messaging credential changed Human account".into());
            }
            if page.len() > PAGE_LIMIT {
                return Err("Fabushi Human history page exceeded the 200-message response budget".into());
            }
            if page.is_empty() {
                if rejected.is_none() {
                    self.check_human_history_continuation(conversation_id, &owner, &metadata, recovery.as_deref())?;
                    Self::store_human_coverage(&owner, after.max(covered), true)?;
                }
                reached_end = true;
                break;
            }
            let is_last = page.len() < PAGE_LIMIT;
            let mut snapshot = owner.get_transcript_entries().map_err(|error| error.to_string())?;
            let mut last = after;
            let mut accepted = Vec::new();
            for remote in &page {
                let id = fabushi_message_identity_text(&remote.id)?.parse::<u64>()
                    .map_err(|_| "remote Human history id is not a positive integer".to_string())?;
                if id <= last || id > MAX_SAFE_ID {
                    return Err("Fabushi Human history afterId did not advance monotonically".into());
                }
                last = id;
                match self.validate_remote_human_message(remote, &local_human_id, &peer, &snapshot) {
                    Ok(()) => accepted.push(remote.clone()),
                    Err(error) => {
                        if recovery.as_deref().is_some_and(HumanSyncRecoveryContext::is_continuing) {
                            return Err(error);
                        }
                        rejected.get_or_insert(error);
                    }
                }
            }
            for remote in accepted {
                self.check_human_history_continuation(conversation_id, &owner, &metadata, recovery.as_deref())?;
                if let Err(error) = self.validate_remote_human_message(&remote, &local_human_id, &peer, &snapshot) {
                    if recovery.as_deref().is_some_and(HumanSyncRecoveryContext::is_continuing) {
                        return Err(error);
                    }
                    rejected.get_or_insert(error);
                    continue;
                }
                let settled = self.materialize_remote_human_message_with_snapshot(&owner, &remote, &snapshot)?;
                Self::update_human_replay_snapshot(&mut snapshot, settled);
                // A synchronous committed-row callback may rotate credentials.
                // Finish that existing materialization/activity unit, then stop.
                self.check_human_history_continuation(conversation_id, &owner, &metadata, recovery.as_deref())?;
            }
            if rejected.is_none() {
                self.check_human_history_continuation(conversation_id, &owner, &metadata, recovery.as_deref())?;
                Self::store_human_coverage(&owner, last, is_last)?;
            }
            after = last;
            if is_last { reached_end = true; break; }
        }
        // A 32-page window is an absolute request budget, including the last
        // short or empty page. A latest-snapshot refresh may use only spare capacity.
        if reached_end && rejected.is_none() && pages_used < MAX_GAP_PAGES && after >= PAGE_LIMIT as u64 {
            self.check_human_history_continuation(conversation_id, &owner, &metadata, recovery.as_deref())?;
            let latest = client.list_direct_messages_for_user(&peer, &local_human_id, None, PAGE_LIMIT)?;
            self.check_human_history_continuation(conversation_id, &owner, &metadata, recovery.as_deref())?;
            let mut snapshot = owner.get_transcript_entries().map_err(|error| error.to_string())?;
            let known = snapshot.iter().filter_map(|entry| entry.get("remoteMessageId")
                .and_then(serde_json::Value::as_str).map(str::to_string)).collect::<BTreeSet<_>>();
            for remote in latest {
                let remote_id = fabushi_message_identity_text(&remote.id)?;
                if !known.contains(remote_id.as_str()) { continue; }
                self.check_human_history_continuation(conversation_id, &owner, &metadata, recovery.as_deref())?;
                self.validate_remote_human_message(&remote, &local_human_id, &peer, &snapshot)?;
                let settled = self.materialize_remote_human_message_with_snapshot(&owner, &remote, &snapshot)?;
                Self::update_human_replay_snapshot(&mut snapshot, settled);
                self.check_human_history_continuation(conversation_id, &owner, &metadata, recovery.as_deref())?;
            }
        }
        self.check_human_history_continuation(conversation_id, &owner, &metadata, recovery.as_deref())?;
        if let Some(error) = rejected { return Err(error); }
        // Full 32-page windows persist a nonterminal cursor for the next sync/restart.
        owner.get_human_transcript_entries().map_err(|error| error.to_string())
    }


    fn human_reaction_remote_identity(entry: &serde_json::Value) -> Result<String, String> {
        let value = entry.get("remoteMessageId")
            .ok_or_else(|| "Human reaction target has no canonical remote message identity".to_string())?;
        let remote_id = fabushi_message_identity_text(value)
            .map_err(|_| "Human reaction target has an invalid canonical remote message identity".to_string())?;
        let canonical_id = format!("human-server-message:{remote_id}");
        if entry.get("id").and_then(serde_json::Value::as_str) != Some(canonical_id.as_str())
            || entry.get("delivery").and_then(serde_json::Value::as_str) != Some("sent")
        {
            return Err("Human reaction target is not a settled canonical message".into());
        }
        Ok(remote_id)
    }

    fn validate_human_reaction_target(
        entry: &serde_json::Value,
        local_human_id: &str,
        peer_human_id: &str,
    ) -> Result<(), String> {
        let author = entry.get("authorId").and_then(serde_json::Value::as_str);
        if entry.get("kind").and_then(serde_json::Value::as_str) != Some("message")
            || entry.get("authorKind").and_then(serde_json::Value::as_str) != Some("human")
            || !matches!(author, Some(value) if value == local_human_id || value == peer_human_id)
        {
            return Err("persisted Human reaction target is outside the selected Human conversation".into());
        }
        Ok(())
    }

    fn replay_human_reaction_intent(
        &self,
        owner: &SandAgentDb,
        metadata: &serde_json::Value,
        client: &FabushiNativeMessagingClient,
    ) -> Result<Option<serde_json::Value>, String> {
        self.replay_human_reaction_intent_with_recovery(owner, metadata, client, None)
    }

    pub(crate) fn replay_human_reaction_intent_with_recovery(
        &self,
        owner: &SandAgentDb,
        metadata: &serde_json::Value,
        client: &FabushiNativeMessagingClient,
        mut recovery: Option<&mut HumanSyncRecoveryContext>,
    ) -> Result<Option<serde_json::Value>, String> {
        let Some(intent) = owner.get_metadata("humanReactionIntent")
            .map_err(|error| error.to_string())?.filter(|value| !value.is_null()) else {
            return Ok(None);
        };
        let get = |field: &str| intent.get(field).and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("persisted Human reaction intent lacks {field}"));
        let entry_id = get("entryId")?;
        let remote_id = get("remoteMessageId")?;
        let emoji = get("emoji")?;
        let author = get("localHumanId")?;
        let active = intent.get("active").and_then(serde_json::Value::as_bool)
            .ok_or_else(|| "persisted Human reaction intent lacks desired state".to_string())?;
        if author != self.local_human_id()? || !self.metadata_has_local_human(metadata)? {
            return Err("persisted Human reaction intent belongs to a different account".into());
        }
        if emoji.is_empty() || emoji != emoji.trim() || emoji.as_bytes().len() > 32 {
            return Err("persisted Human reaction intent contains invalid emoji".into());
        }
        if remote_id.parse::<u64>().ok().filter(|id| {
            *id > 0 && *id <= 9_007_199_254_740_991 && id.to_string() == remote_id
        }).is_none() {
            return Err("persisted Human reaction intent contains invalid remote identity".into());
        }
        let peer = self.peer_human_id_from_metadata(metadata)?;
        let current = owner.get_transcript_entry_by_id(entry_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "persisted Human reaction target disappeared".to_string())?;
        Self::validate_human_reaction_target(&current, author, &peer)?;
        if Self::human_reaction_remote_identity(&current)? != remote_id {
            return Err("persisted Human reaction remote identity changed".into());
        }
        let revalidate_owner = || -> Result<(), String> {
            let conversation_id = metadata.get("conversationId")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "existing Human conversation metadata is missing its identity".to_string())?;
            self.revalidate_human_recovery_owner(conversation_id, owner, metadata)
        };
        if let Some(recovery) = recovery.as_deref() {
            self.check_human_sync_binding(client, recovery)?;
            if recovery.is_continuing() { revalidate_owner()?; }
        }
        let response = client.set_direct_message_reaction_for_user_classified(
            remote_id, emoji, active, author,
        );
        if let Some(recovery) = recovery.as_deref() {
            self.check_human_sync_binding(client, recovery)?;
            if recovery.is_continuing() { revalidate_owner()?; }
        }
        let remote = match response {
            Ok(remote) => remote,
            Err(FabushiHumanReactionError::RemoteRefusal { status, message }) => {
                let Some(recovery) = recovery.as_deref_mut() else { return Err(message); };
                if !matches!(status, 404 | 409) { return Err(message); }
                // A transport refusal is only a candidate. The request cannot
                // lend its old admission to an independently changed SQLite row.
                revalidate_owner()?;
                if owner.get_metadata("humanReactionIntent").map_err(|error| error.to_string())?.as_ref()
                    != Some(&intent)
                {
                    return Err("persisted Human reaction intent changed before recovery continuation".into());
                }
                let current_target = owner.get_transcript_entry_by_id(entry_id)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| "persisted Human reaction target disappeared".to_string())?;
                Self::validate_human_reaction_target(&current_target, author, &peer)?;
                if Self::human_reaction_remote_identity(&current_target)? != remote_id {
                    return Err("persisted Human reaction remote identity changed".into());
                }
                if current_target.get("authorId") != current.get("authorId") {
                    return Err("persisted Human reaction target author changed".into());
                }
                self.check_human_sync_binding(client, recovery)?;
                recovery.first_reaction_refusal.get_or_insert(message);
                return Ok(None);
            }
            Err(error) => return Err(error.into_message()),
        };
        Self::validate_remote_human_reactions(&remote)?;
        // HTTP success is not settlement: the service must confirm the requested
        // participant state. Keep the durable intent if a stale/misrouted reply
        // reports the opposite state so restart can retry without retoggling.
        let observed_active = remote.iter().any(|reaction| {
            reaction.emoji == emoji && reaction.reacted_by_me
        });
        if observed_active != active {
            return Err("Fabushi Human reaction response did not confirm the desired state".into());
        }
        let projected = Self::project_remote_human_reactions(&remote, &peer);
        let mut next = owner.get_transcript_entry_by_id(entry_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "Human reaction target disappeared before durable settlement".to_string())?;
        Self::validate_human_reaction_target(&next, author, &peer)?;
        if Self::human_reaction_remote_identity(&next)? != remote_id {
            return Err("persisted Human reaction remote identity changed".into());
        }
        let is_peer = next.get("authorId").and_then(serde_json::Value::as_str) != Some(author);
        let object = next.as_object_mut()
            .ok_or_else(|| "Human reaction target is not a message object".to_string())?;
        if is_peer {
            if let Some(legacy_nonce) = object.remove("clientNonce") {
                object.entry("remoteClientRequestId".to_string()).or_insert(legacy_nonce);
            }
        }
        object.insert("remoteReactions".into(), serde_json::to_value(&remote)
            .map_err(|error| format!("remote Human reactions could not be projected: {error}"))?);
        object.insert("reactions".into(), serde_json::Value::Array(projected));
        let settled = owner.update_transcript_entry(entry_id, &next)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "Human reaction target disappeared before durable settlement".to_string())?;
        if let Some(recovery) = recovery.as_deref().filter(|value| value.is_continuing()) {
            self.check_human_sync_binding(client, recovery)?;
            revalidate_owner()?;
        }
        if !owner.set_metadata("humanReactionIntent", serde_json::Value::Null)
            .map_err(|error| error.to_string())? {
            return Err("Human reaction intent could not be durably acknowledged".into());
        }
        Ok(Some(settled))
    }

    pub fn toggle_human_message_reaction(
        &self,
        conversation_id: &str,
        entry_id: &str,
        emoji: &str,
    ) -> Result<serde_json::Value, String> {
        if emoji.trim().is_empty() { return Err("reactHumanMessage requires emoji".into()); }
        if emoji != emoji.trim() {
            return Err("Fabushi Human reaction emoji must not contain surrounding whitespace".into());
        }
        if emoji.as_bytes().len() > 32 {
            return Err("Fabushi Human reaction emoji exceeds the backend limit".into());
        }
        let serial = self.human_message_lock(conversation_id)?;
        let _exclusive = serial.lock().map_err(|_| "Human conversation serial lock poisoned".to_string())?;
        let owner = self.open_human_conversation_db_owner(conversation_id)?;
        let metadata = owner.read_metadata().map_err(|error| error.to_string())?;
        if !self.metadata_has_local_human(&metadata)? {
            return Err("local Human identity is not a conversation participant".into());
        }
        let client = self.shipping_native_messaging()?
            .ok_or_else(|| "Fabushi Human reactions require the shipping messaging backend".to_string())?;
        let prior = owner.get_metadata("humanReactionIntent").map_err(|error| error.to_string())?;
        let same_intent = prior.as_ref().is_some_and(|intent| {
            intent.get("entryId").and_then(serde_json::Value::as_str) == Some(entry_id)
                && intent.get("emoji").and_then(serde_json::Value::as_str) == Some(emoji)
        });
        if let Some(replayed) = self.replay_human_reaction_intent(&owner, &metadata, client)? {
            if same_intent { return Ok(replayed); }
        }
        let current = owner.get_transcript_entry_by_id(entry_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "Human reaction target is not in this conversation".to_string())?;
        Self::validate_human_reaction_target(&current, self.local_human_id()?, &self.peer_human_id_from_metadata(&metadata)?)?;
        let remote_id = Self::human_reaction_remote_identity(&current)?;
        let active = current.get("remoteReactions")
            .and_then(serde_json::Value::as_array).is_some_and(|reactions| {
                reactions.iter().any(|reaction| {
                    reaction.get("emoji").and_then(serde_json::Value::as_str) == Some(emoji)
                        && reaction.get("reactedByMe").and_then(serde_json::Value::as_bool) == Some(true)
                })
            });
        let author = self.local_human_id()?;
        if client.identity()?.user_id != author {
            return Err("Fabushi native messaging credential changed Human account".into());
        }
        if !owner.set_metadata("humanReactionIntent", serde_json::json!({
            "entryId": entry_id, "remoteMessageId": remote_id,
            "emoji": emoji, "active": !active, "localHumanId": author,
        })).map_err(|error| error.to_string())? {
            return Err("Human reaction intent could not be durably staged".into());
        }
        self.replay_human_reaction_intent(&owner, &metadata, client)?
            .ok_or_else(|| "Human reaction intent disappeared before settlement".to_string())
    }

    pub(crate) fn resolve_human_forward_origin(
        source_entry: &serde_json::Value,
        source_metadata: Option<&serde_json::Value>,
        local_human_id: &str,
        privacy: fabushi_messaging_core::ForwardPrivacy,
    ) -> Option<serde_json::Value> {
        let source_context = source_entry.get("forwardContext");
        let source_forward_privacy =
            source_context.map(|ctx| fabushi_messaging_core::ForwardPrivacy {
                drop_sender_names: ctx
                    .pointer("/privacy/dropSenderNames")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                drop_captions: ctx
                    .pointer("/privacy/dropCaptions")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            });
        let source_origin_author_id = source_context
            .and_then(|ctx| ctx.pointer("/origin/authorId"))
            .and_then(serde_json::Value::as_str);
        let source_origin_author_name = source_context
            .and_then(|ctx| ctx.pointer("/origin/authorName"))
            .and_then(serde_json::Value::as_str);
        let source_author_id = source_entry
            .get("authorId")
            .and_then(serde_json::Value::as_str);
        let source_author_name = source_entry
            .get("authorName")
            .and_then(serde_json::Value::as_str);
        let peer_title_fallback = if source_author_id.is_some_and(|id| id.trim() != local_human_id)
        {
            source_metadata
                .and_then(|metadata| metadata.get("title"))
                .and_then(serde_json::Value::as_str)
        } else {
            None
        };
        let attribution = privacy.resolve_forward_origin_attribution(
            source_forward_privacy,
            source_origin_author_id,
            source_origin_author_name,
            source_author_id,
            source_author_name,
            peer_title_fallback,
        )?;
        let mut origin = serde_json::Map::new();
        if let Some(author_id) = attribution.author_id {
            origin.insert(
                "authorId".into(),
                serde_json::Value::String(author_id.to_string()),
            );
        }
        if let Some(author_name) = attribution.author_name {
            origin.insert(
                "authorName".into(),
                serde_json::Value::String(author_name.to_string()),
            );
        }
        (!origin.is_empty()).then_some(serde_json::Value::Object(origin))
    }

    pub(crate) fn human_entry_visible_forward_origin_author(
        entry: &serde_json::Value,
    ) -> Option<&str> {
        let context = entry.get("forwardContext")?;
        let privacy = fabushi_messaging_core::ForwardPrivacy {
            drop_sender_names: context
                .pointer("/privacy/dropSenderNames")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            drop_captions: context
                .pointer("/privacy/dropCaptions")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        };
        let origin_author_name = context
            .pointer("/origin/authorName")
            .and_then(serde_json::Value::as_str);
        fabushi_messaging_core::visible_forward_origin_author(privacy, origin_author_name)
    }

}
