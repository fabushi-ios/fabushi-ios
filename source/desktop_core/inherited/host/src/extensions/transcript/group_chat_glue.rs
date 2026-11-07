use std::collections::HashSet;
use std::sync::Arc;

use serde::Serialize;
use serde_json::{Value, json};

use crate::agents::agent_profile::SandAgentProfile;
use crate::groups::group_chat::{
    assert_members_are_not_groups, is_potential_pass_prefix, is_same_member_set,
};
use crate::groups::group_store::{
    GROUP_CONFIG_VERSION, GROUP_MAX_MEMBERS, SandGroupConfig,
    read_sand_group_config, write_sand_group_config,
};
use crate::extensions::session::agent_session::SandAgentSessionStore;
use crate::extensions::session::production::ProductionSessionWorkers;
use crate::extensions::session::session_summaries::AgentSummary;

use super::production_runtime::ProductionTranscriptRuntime;
use super::send_group_fanout::{
    GroupMemberTurnExecutor, LocalGroupFanoutDisposition,
    dispatch_local_group_send_with_observer,
};
pub use super::send_group_fanout::GroupRoomEntryObserver;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GroupChatGlueError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Internal(String),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupCreateResult {
    pub agent: AgentSummary,
    pub transcript: Vec<Value>,
}

pub struct GroupChatGlue {
    session: Arc<ProductionSessionWorkers>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GroupMemberPreview {
    room_id: String,
    member_id: String,
    member_name: String,
    entry_id: String,
    is_open: bool,
}

impl GroupMemberPreview {
    pub fn new(room_id: &str, member_id: &str, member_name: &str, stream_id: &str) -> Self {
        Self {
            room_id: room_id.to_string(),
            member_id: member_id.to_string(),
            member_name: member_name.to_string(),
            entry_id: format!("group-preview:{stream_id}"),
            is_open: false,
        }
    }

    pub fn on_text_delta(&mut self, accumulated: &str) -> Option<Value> {
        if is_potential_pass_prefix(accumulated) {
            return self.finish();
        }
        let content = accumulated.trim_end();
        if content.trim().is_empty() {
            return None;
        }
        let event_type = if self.is_open {
            "updated"
        } else {
            self.is_open = true;
            "appended"
        };
        Some(json!({
            "type": event_type,
            "agentId": self.room_id,
            "entry": {
                "kind": "send-message",
                "id": self.entry_id,
                "message": {"type": "text", "content": content},
                "author": {"id": self.member_id, "name": self.member_name},
                "streaming": true,
            }
        }))
    }

    pub fn finish(&mut self) -> Option<Value> {
        if !self.is_open {
            return None;
        }
        self.is_open = false;
        Some(json!({
            "type": "removed",
            "agentId": self.room_id,
            "id": self.entry_id,
        }))
    }
}

pub fn group_member_reaction_target(group_room_id: Option<&str>, member_agent_id: &str) -> String {
    group_room_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(member_agent_id)
        .to_string()
}

pub fn should_redrive_group_member_after_preemption(
    sent_message_count: u64,
    reacted: bool,
) -> bool {
    sent_message_count == 0 && !reacted
}

impl GroupChatGlue {
    pub fn new(session: Arc<ProductionSessionWorkers>) -> Self {
        Self { session }
    }

    pub fn create_group(
        &self,
        name: &str,
        description: &str,
        member_ids: &[String],
    ) -> Result<GroupCreateResult, GroupChatGlueError> {
        let store = SandAgentSessionStore::new(Arc::clone(&self.session));
        let all_agents = store.list_agents().map_err(GroupChatGlueError::Internal)?;
        let existing = all_agents
            .iter()
            .map(|agent| agent.id.clone())
            .collect::<HashSet<_>>();
        let group_ids = all_agents
            .iter()
            .filter(|agent| agent.is_group)
            .map(|agent| agent.id.clone())
            .collect::<HashSet<_>>();
        let requested = stable_unique(member_ids);
        assert_members_are_not_groups(&requested, |id| group_ids.contains(id))
            .map_err(|error| GroupChatGlueError::BadRequest(error.to_string()))?;
        let cleaned = requested
            .into_iter()
            .filter(|id| existing.contains(id))
            .take(GROUP_MAX_MEMBERS)
            .collect::<Vec<_>>();
        if cleaned.is_empty() {
            return Err(GroupChatGlueError::BadRequest(
                "A group needs at least one existing member agent.".into(),
            ));
        }

        if let Some(duplicate) = all_agents
            .iter()
            .find(|agent| agent.is_group && is_same_member_set(&agent.member_ids, &cleaned))
        {
            store
                .write_active_agent_id(&duplicate.id)
                .map_err(|error| GroupChatGlueError::Internal(error.to_string()))?;
            let agent = store
                .summarize_agent_by_id(&duplicate.id)
                .map_err(GroupChatGlueError::Internal)?
                .unwrap_or_else(|| duplicate.clone());
            let transcript = self
                .session
                .read_agent_transcript_entries(&duplicate.id)
                .map_err(GroupChatGlueError::Internal)?;
            return Ok(GroupCreateResult { agent, transcript });
        }

        let profile = SandAgentProfile {
            name: name.to_string(),
            description: description.to_string(),
            title: String::new(),
            avatar_shape: String::new(),
            avatar_color: String::new(),
        };
        let record = store
            .create_session(Some(&profile), "user", None)
            .map_err(GroupChatGlueError::Internal)?;
        let session = Arc::clone(&self.session);
        session
            .with_agent_admitted_filesystem_mutation(&record.id, || {
                write_sand_group_config(
                    store.get_agent_dir(&record.id),
                    &SandGroupConfig {
                        version: GROUP_CONFIG_VERSION,
                        member_ids: cleaned,
                        remote_members: None,
                        shared_room_id: None,
                    },
                )
                .map_err(|error| error.to_string())
            })
            .map_err(GroupChatGlueError::Internal)?;
        store
            .write_active_agent_id(&record.id)
            .map_err(|error| GroupChatGlueError::Internal(error.to_string()))?;
        let agent = store
            .summarize_agent_by_id(&record.id)
            .map_err(GroupChatGlueError::Internal)?
            .ok_or_else(|| {
                GroupChatGlueError::Internal("failed to summarize newly created group".into())
            })?;
        let transcript = self
            .session
            .read_agent_transcript_entries(&record.id)
            .map_err(GroupChatGlueError::Internal)?;
        Ok(GroupCreateResult { agent, transcript })
    }

    pub fn set_group_members(
        &self,
        group_id: &str,
        member_ids: &[String],
    ) -> Result<Option<AgentSummary>, GroupChatGlueError> {
        let store = SandAgentSessionStore::new(Arc::clone(&self.session));
        let dir = store.get_agent_dir(group_id);
        let Some(current) = read_sand_group_config(&dir) else {
            return Ok(None);
        };
        if current.shared_room_id.is_some() {
            return store
                .summarize_agent_by_id(group_id)
                .map_err(GroupChatGlueError::Internal);
        }

        let all_agents = store.list_agents().map_err(GroupChatGlueError::Internal)?;
        let existing = all_agents
            .iter()
            .map(|agent| agent.id.clone())
            .collect::<HashSet<_>>();
        let group_ids = all_agents
            .iter()
            .filter(|agent| agent.is_group)
            .map(|agent| agent.id.clone())
            .collect::<HashSet<_>>();
        let requested = stable_unique(member_ids);
        assert_members_are_not_groups(&requested, |id| group_ids.contains(id))
            .map_err(|error| GroupChatGlueError::BadRequest(error.to_string()))?;
        let cleaned = requested
            .into_iter()
            .filter(|id| id != group_id && existing.contains(id))
            .take(GROUP_MAX_MEMBERS)
            .collect::<Vec<_>>();
        if !cleaned.is_empty() {
            let session = Arc::clone(&self.session);
            session
                .with_agent_admitted_filesystem_mutation(group_id, || {
                    write_sand_group_config(
                        &dir,
                        &SandGroupConfig {
                            version: GROUP_CONFIG_VERSION,
                            member_ids: cleaned,
                            remote_members: None,
                            shared_room_id: None,
                        },
                    )
                    .map_err(|error| error.to_string())
                })
                .map_err(GroupChatGlueError::Internal)?;
        }
        store
            .summarize_agent_by_id(group_id)
            .map_err(GroupChatGlueError::Internal)
    }

    pub fn run_group_turn(
        &self,
        runtime: Arc<ProductionTranscriptRuntime>,
        room_id: &str,
        expected_epoch: u64,
        executor: GroupMemberTurnExecutor,
        remote_executor: Option<GroupMemberTurnExecutor>,
        entry_observer: Option<GroupRoomEntryObserver>,
    ) -> Result<LocalGroupFanoutDisposition, String> {
        dispatch_local_group_send_with_observer(
            Arc::clone(&self.session),
            runtime,
            room_id,
            expected_epoch,
            executor,
            remote_executor,
            entry_observer,
        )
    }

    pub fn is_group_agent_id(&self, agent_id: &str) -> bool {
        let store = SandAgentSessionStore::new(Arc::clone(&self.session));
        read_sand_group_config(store.get_agent_dir(agent_id)).is_some()
    }
}

fn stable_unique(values: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .filter(|value| seen.insert((*value).to_string()))
        .map(ToOwned::to_owned)
        .collect()
}
