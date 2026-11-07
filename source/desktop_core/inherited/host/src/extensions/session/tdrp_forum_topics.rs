//! Telegram / TDRP Forum Topics, Groups, and Channels.

use super::*;

impl ProductionSessionWorkers {
    pub fn create_group_conversation(
        &self,
        group_id: &str,
        title: &str,
        participant_ids: &[String],
        slow_mode_seconds: Option<u32>,
    ) -> Result<serde_json::Value, String> {
        let local_human_id = self.local_human_id()?;
        let trimmed_id = group_id.trim();
        if trimmed_id.is_empty()
            || !trimmed_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err("group conversation id must be non-empty and contain only safe characters".into());
        }
        let conversation_id = format!("group-{}", trimmed_id);
        let mut unique_participants = Vec::new();
        unique_participants.push(local_human_id.to_string());
        for id in participant_ids {
            let trimmed = id.trim();
            if !trimmed.is_empty() && !unique_participants.iter().any(|p| p == trimmed) {
                unique_participants.push(trimmed.to_string());
            }
        }
        let serial = self.human_message_lock(&conversation_id)?;
        let _exclusive = serial
            .lock()
            .map_err(|_| "Human conversation serial lock poisoned".to_string())?;

        let db_path = get_native_conversation_db_path(&self.agents_root, &conversation_id)
            .map_err(|error| error.to_string())?;
        let key = Self::native_conversation_owner_key(&conversation_id);
        let cached_owner = self
            .db_owners
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

        let display_title = if title.trim().is_empty() {
            trimmed_id
        } else {
            title.trim()
        };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let created_at = now_ms as f64;
        if owner.read_kv("metadata").map_err(|error| error.to_string())?.is_none() {
            let mut meta_obj = serde_json::json!({
                "conversationKind": "group",
                "conversationId": conversation_id,
                "title": display_title,
                "localHumanId": local_human_id,
                "participantIds": unique_participants,
                "createdAt": created_at,
                "lastActivityAt": 0.0,
            });
            if let Some(slow) = slow_mode_seconds {
                meta_obj
                    .as_object_mut()
                    .unwrap()
                    .insert("slowModeSeconds".into(), serde_json::json!(slow));
            }
            let _ = owner
                .initialize_human_metadata_if_empty(meta_obj)
                .map_err(|error| error.to_string())?;
        }

        let owner_member = fabushi_messaging_core::CommunityMember {
            actor_id: fabushi_messaging_core::ActorId::new(local_human_id),
            status: fabushi_messaging_core::MemberStatus::Owner,
            admin_title: Some("Owner".into()),
            admin_rights: fabushi_messaging_core::AdminRights {
                change_info: true,
                post_messages: true,
                edit_messages: true,
                delete_messages: true,
                ban_members: true,
                invite_members: true,
                pin_messages: true,
                manage_topics: true,
                manage_calls: true,
                add_admins: true,
                remain_anonymous: false,
            },
            restrictions: fabushi_messaging_core::MemberRestrictions::default(),
            joined_at_ms: now_ms,
            invited_by: None,
        };
        if owner
            .get_conversation_member(local_human_id)
            .map_err(|e| e.to_string())?
            .is_none()
        {
            owner
                .upsert_conversation_member(&owner_member, now_ms)
                .map_err(|e| e.to_string())?;
        }
        for pid in &unique_participants {
            if pid == local_human_id {
                continue;
            }
            if owner
                .get_conversation_member(pid)
                .map_err(|e| e.to_string())?
                .is_none()
            {
                let member = fabushi_messaging_core::CommunityMember {
                    actor_id: fabushi_messaging_core::ActorId::new(pid),
                    status: fabushi_messaging_core::MemberStatus::Member,
                    admin_title: None,
                    admin_rights: fabushi_messaging_core::AdminRights::default(),
                    restrictions: fabushi_messaging_core::MemberRestrictions::default(),
                    joined_at_ms: now_ms,
                    invited_by: Some(fabushi_messaging_core::ActorId::new(local_human_id)),
                };
                owner
                    .upsert_conversation_member(&member, now_ms)
                    .map_err(|e| e.to_string())?;
            }
        }

        {
            let mut owners = self
                .db_owners
                .lock()
                .map_err(|_| "conversation db owner map poisoned".to_string())?;
            owners.entry(key).or_insert_with(|| Arc::clone(&owner));
        }

        let mut result = serde_json::json!({
            "id": conversation_id,
            "kind": "group",
            "title": display_title,
            "participantIds": unique_participants,
            "createdAt": created_at,
        });
        if let Some(slow) = slow_mode_seconds {
            result
                .as_object_mut()
                .unwrap()
                .insert("slowModeSeconds".into(), serde_json::json!(slow));
        }
        Ok(result)
    }

    pub fn create_channel_conversation(
        &self,
        channel_id: &str,
        title: &str,
        participant_ids: &[String],
    ) -> Result<serde_json::Value, String> {
        let local_human_id = self.local_human_id()?;
        let trimmed_id = channel_id.trim();
        if trimmed_id.is_empty()
            || !trimmed_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err("channel conversation id must be non-empty and contain only safe characters".into());
        }
        let conversation_id = format!("channel-{}", trimmed_id);
        let mut unique_participants = Vec::new();
        unique_participants.push(local_human_id.to_string());
        for id in participant_ids {
            let trimmed = id.trim();
            if !trimmed.is_empty() && !unique_participants.iter().any(|p| p == trimmed) {
                unique_participants.push(trimmed.to_string());
            }
        }
        let serial = self.human_message_lock(&conversation_id)?;
        let _exclusive = serial
            .lock()
            .map_err(|_| "Human conversation serial lock poisoned".to_string())?;

        let db_path = get_native_conversation_db_path(&self.agents_root, &conversation_id)
            .map_err(|error| error.to_string())?;
        let key = Self::native_conversation_owner_key(&conversation_id);
        let cached_owner = self
            .db_owners
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

        let display_title = if title.trim().is_empty() {
            trimmed_id
        } else {
            title.trim()
        };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let created_at = now_ms as f64;
        if owner.read_kv("metadata").map_err(|error| error.to_string())?.is_none() {
            let meta_obj = serde_json::json!({
                "conversationKind": "channel",
                "conversationId": conversation_id,
                "title": display_title,
                "localHumanId": local_human_id,
                "participantIds": unique_participants,
                "createdAt": created_at,
                "lastActivityAt": 0.0,
            });
            let _ = owner
                .initialize_human_metadata_if_empty(meta_obj)
                .map_err(|error| error.to_string())?;
        }

        let owner_member = fabushi_messaging_core::CommunityMember {
            actor_id: fabushi_messaging_core::ActorId::new(local_human_id),
            status: fabushi_messaging_core::MemberStatus::Owner,
            admin_title: Some("Owner".into()),
            admin_rights: fabushi_messaging_core::AdminRights {
                change_info: true,
                post_messages: true,
                edit_messages: true,
                delete_messages: true,
                ban_members: true,
                invite_members: true,
                pin_messages: true,
                manage_topics: true,
                manage_calls: true,
                add_admins: true,
                remain_anonymous: false,
            },
            restrictions: fabushi_messaging_core::MemberRestrictions::default(),
            joined_at_ms: now_ms,
            invited_by: None,
        };
        if owner
            .get_conversation_member(local_human_id)
            .map_err(|e| e.to_string())?
            .is_none()
        {
            owner
                .upsert_conversation_member(&owner_member, now_ms)
                .map_err(|e| e.to_string())?;
        }
        for pid in &unique_participants {
            if pid == local_human_id {
                continue;
            }
            if owner
                .get_conversation_member(pid)
                .map_err(|e| e.to_string())?
                .is_none()
            {
                let member = fabushi_messaging_core::CommunityMember {
                    actor_id: fabushi_messaging_core::ActorId::new(pid),
                    status: fabushi_messaging_core::MemberStatus::Member,
                    admin_title: None,
                    admin_rights: fabushi_messaging_core::AdminRights::default(),
                    restrictions: fabushi_messaging_core::MemberRestrictions::default(),
                    joined_at_ms: now_ms,
                    invited_by: Some(fabushi_messaging_core::ActorId::new(local_human_id)),
                };
                owner
                    .upsert_conversation_member(&member, now_ms)
                    .map_err(|e| e.to_string())?;
            }
        }

        {
            let mut owners = self
                .db_owners
                .lock()
                .map_err(|_| "conversation db owner map poisoned".to_string())?;
            owners.entry(key).or_insert_with(|| Arc::clone(&owner));
        }

        Ok(serde_json::json!({
            "id": conversation_id,
            "kind": "channel",
            "title": display_title,
            "participantIds": unique_participants,
            "createdAt": created_at,
        }))
    }

    pub fn upsert_conversation_topic(
        &self,
        conversation_id: &str,
        topic_id: &str,
        title: &str,
        icon: Option<&str>,
        closed: bool,
        pinned: bool,
        hidden: bool,
    ) -> Result<serde_json::Value, String> {
        let local_human_id = self.local_human_id()?;
        let conversation_id = conversation_id.trim();
        let topic_id = topic_id.trim();
        let title = title.trim();
        if conversation_id.is_empty() || topic_id.is_empty() || title.is_empty() {
            return Err("upsertConversationTopic requires conversationId, topicId, and title".into());
        }
        let owner = self.open_human_conversation_db_owner(conversation_id)?;
        let metadata = owner.read_metadata().map_err(|e| e.to_string())?;
        let kind = metadata.get("conversationKind").and_then(serde_json::Value::as_str).unwrap_or("human");
        if kind != "group" && kind != "channel" {
            return Err("upsertConversationTopic is only supported on group or channel conversations".into());
        }
        let caller_member = owner.get_conversation_member(local_human_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "caller is not a member of the conversation".to_string())?;
        let can_manage_topics = caller_member.status == fabushi_messaging_core::MemberStatus::Owner
            || (caller_member.status == fabushi_messaging_core::MemberStatus::Administrator && caller_member.admin_rights.manage_topics);
        if !can_manage_topics {
            return Err("caller lacks manage_topics administrative permission".into());
        }
        let existing_topic = owner.get_conversation_topic(topic_id).map_err(|e| e.to_string())?;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let topic_state = if let Some(existing) = existing_topic {
            fabushi_messaging_core::ForumTopicState {
                id: topic_id.to_string(),
                conversation_id: fabushi_messaging_core::ConversationId(conversation_id.to_string()),
                title: title.to_string(),
                icon: icon.map(str::to_string).or(existing.icon),
                creator_id: existing.creator_id,
                created_at_ms: existing.created_at_ms,
                pinned,
                closed,
                hidden,
                unread_count: existing.unread_count,
                last_message_id: existing.last_message_id,
            }
        } else {
            fabushi_messaging_core::ForumTopicState {
                id: topic_id.to_string(),
                conversation_id: fabushi_messaging_core::ConversationId(conversation_id.to_string()),
                title: title.to_string(),
                icon: icon.map(str::to_string),
                creator_id: fabushi_messaging_core::ActorId::new(local_human_id),
                created_at_ms: now_ms,
                pinned,
                closed,
                hidden,
                unread_count: 0,
                last_message_id: None,
            }
        };
        owner.upsert_conversation_topic(&topic_state).map_err(|e| e.to_string())?;
        serde_json::to_value(&topic_state).map_err(|e| e.to_string())
    }

    pub fn list_conversation_topics(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<serde_json::Value>, String> {
        let conversation_id = conversation_id.trim();
        if conversation_id.is_empty() {
            return Err("listConversationTopics requires conversationId".into());
        }
        let owner = self.open_human_conversation_db_owner(conversation_id)?;
        let topics = owner.list_conversation_topics().map_err(|e| e.to_string())?;
        topics.into_iter()
            .map(|t| serde_json::to_value(t).map_err(|e| e.to_string()))
            .collect()
    }


    pub fn read_human_conversation_transcript_for_topic(
        &self,
        conversation_id: &str,
        topic_id: Option<&str>,
    ) -> Result<Vec<serde_json::Value>, String> {
        let owner = self.open_human_conversation_db_owner(conversation_id)?;
        let metadata = owner.read_metadata().map_err(|error| error.to_string())?;
        if !self.metadata_has_local_human(&metadata)? {
            return Err("local Human identity is not a conversation participant".into());
        }
        owner
            .get_human_transcript_entries_for_topic(topic_id)
            .map_err(|error| error.to_string())
    }

}
