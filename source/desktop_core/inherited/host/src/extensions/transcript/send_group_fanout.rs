use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use futures::FutureExt;
use serde_json::{Value, json};

use crate::extensions::session::production::ProductionSessionWorkers;
use crate::groups::group_chat::{
    GroupDescription, GroupMember, GroupMessage, GroupSpeaker, build_group_redrive_note,
};
use crate::groups::group_store::{SandGroupConfig, read_sand_group_config};

use super::group_chat_orchestrator::{
    GroupChatOrchestrator, GroupMemberTurnRequest, GroupOrchestratorDeps,
};
use super::production_runtime::ProductionTranscriptRuntime;
use super::transcript_entry_ids::{TranscriptEntryIdKind, next_entry_id};

pub type GroupMemberTurnExecutor =
    Arc<dyn Fn(GroupMemberTurnRequest) -> Result<Vec<String>, String> + Send + Sync + 'static>;
pub type GroupRoomEntryObserver =
    Arc<dyn Fn(&str, &Value) + Send + Sync + 'static>;

pub const GROUP_MEMBER_DM_PREEMPTED_ERROR: &str =
    "__sand_group_member_dm_preempted__";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalGroupFanoutDisposition {
    NotGroup,
    DeferredRemote {
        shared_room_id: Option<String>,
        remote_member_count: usize,
    },
    Completed {
        posted_messages: usize,
        member_failures: Vec<(String, String)>,
    },
}

struct LocalGroupFanoutDeps {
    sessions: Arc<ProductionSessionWorkers>,
    runtime: Arc<ProductionTranscriptRuntime>,
    room_id: String,
    expected_epoch: u64,
    config: SandGroupConfig,
    executor: GroupMemberTurnExecutor,
    remote_executor: Option<GroupMemberTurnExecutor>,
    posted_messages: Arc<Mutex<usize>>,
    member_failures: Arc<Mutex<Vec<(String, String)>>>,
    post_error: Arc<Mutex<Option<String>>>,
    entry_observer: Option<GroupRoomEntryObserver>,
}

impl LocalGroupFanoutDeps {
    fn read_history_from_room(&self) -> Vec<GroupMessage> {
        self.sessions
            .read_agent_transcript_entries(&self.room_id)
            .unwrap_or_default()
            .into_iter()
            .filter_map(transcript_entry_to_group_message)
            .collect()
    }

    fn append_member_message(&self, member: &GroupMember, content: &str) -> Result<(), String> {
        let entries = self.sessions.read_agent_transcript_entries(&self.room_id)?;
        let id = next_entry_id(&entries, TranscriptEntryIdKind::SendMessage);
        let mut entry = json!({
            "kind": "send-message",
            "id": id,
            "message": {
                "type": "text",
                "content": content,
            },
            "timestampMs": now_ms(),
            "author": {
                "id": member.id,
                "name": member.name,
            },
        });
        if let Some(session_key) = self
            .sessions
            .read_session_work_admission(&self.room_id)?
            .map(|record| record.session_key)
            .filter(|value| !value.trim().is_empty())
        {
            entry["sessionKey"] = Value::String(session_key.clone());
            self.sessions.append_agent_transcript_entries_if_session_admitted(
                &self.room_id,
                &session_key,
                &[entry.clone()],
            )?;
        } else {
            self.sessions
                .append_agent_transcript_entries(&self.room_id, &[entry.clone()])?;
        }
        let _ = self.sessions.mark_agent_activity(&self.room_id, now_ms());
        if let Some(shared_room_id) = self.config.shared_room_id.as_deref() {
            self.runtime
                .publish_shared_group_room_entry(shared_room_id, &entry)?;
        }
        if let Some(observer) = self.entry_observer.as_ref() {
            observer(&self.room_id, &entry);
        }
        if let Ok(mut posted) = self.posted_messages.lock() {
            *posted = posted.saturating_add(1);
        }
        Ok(())
    }
}

impl GroupOrchestratorDeps for LocalGroupFanoutDeps {
    fn resolve_members<'a>(
        &'a self,
        ids: &'a [String],
    ) -> futures::future::BoxFuture<'a, Vec<GroupMember>> {
        let mut members = Vec::new();
        for id in ids {
            if id.starts_with("sand-remote:") {
                if self.remote_executor.is_some() {
                    if let Some(remote) = self
                        .config
                        .remote_members
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .find(|remote| remote_member_id(remote) == *id)
                    {
                        members.push(GroupMember {
                            id: id.clone(),
                            name: remote.name.clone(),
                            description: String::new(),
                        });
                    }
                }
                continue;
            }
            let Ok(Some(summary)) = self.sessions.summarize_agent_by_id(id, None) else {
                continue;
            };
            if summary.is_group {
                continue;
            }
            members.push(GroupMember {
                id: summary.id,
                name: if summary.name.trim().is_empty() {
                    "Grok".to_string()
                } else {
                    summary.name
                },
                description: summary.description,
            });
        }
        futures::future::ready(members).boxed()
    }

    fn read_history(&self) -> Vec<GroupMessage> {
        self.read_history_from_room()
    }

    fn is_current(&self) -> bool {
        self.runtime.current_turn_epoch(&self.room_id) == self.expected_epoch
    }

    fn run_member_turn<'a>(
        &'a self,
        request: GroupMemberTurnRequest,
    ) -> futures::future::BoxFuture<'a, Vec<String>> {
        let member_id = request.member.id.clone();
        let is_remote = member_id.starts_with("sand-remote:");
        let executor = if is_remote {
            self.remote_executor.as_ref().unwrap_or(&self.executor)
        } else {
            &self.executor
        };
        if is_remote {
            self.runtime
                .set_active_remote_member(&self.room_id, Some(&member_id));
        }
        let mut attempt = 1usize;
        let output = loop {
            let mut attempt_request = request.clone();
            if attempt > 1 {
                attempt_request.prompt.push_str(build_group_redrive_note());
            }
            match executor(attempt_request) {
                Ok(messages) => break messages,
                Err(error) if error == GROUP_MEMBER_DM_PREEMPTED_ERROR => {
                    if attempt < 3 && self.is_current() {
                        attempt += 1;
                        continue;
                    }
                    break Vec::new();
                }
                Err(error) => {
                    if let Ok(mut failures) = self.member_failures.lock() {
                        failures.push((member_id.clone(), error));
                    }
                    break Vec::new();
                }
            }
        };
        if is_remote {
            self.runtime.set_active_remote_member(&self.room_id, None);
        }
        futures::future::ready(output).boxed()
    }

    fn post_member_message(&self, member: &GroupMember, content: &str) {
        if let Err(error) = self.append_member_message(member, content) {
            if let Ok(mut slot) = self.post_error.lock() {
                slot.get_or_insert(error);
            }
        }
    }

    fn finalize_member_turn(&self, _member: &GroupMember) {}

    fn is_shared_room(&self) -> bool {
        self.config.shared_room_id.is_some()
            || self
                .config
                .remote_members
                .as_ref()
                .is_some_and(|members| !members.is_empty())
    }

    fn shared_room_id(&self) -> Option<String> {
        self.config.shared_room_id.clone()
    }
}

pub fn dispatch_local_group_send(
    sessions: Arc<ProductionSessionWorkers>,
    runtime: Arc<ProductionTranscriptRuntime>,
    room_id: &str,
    expected_epoch: u64,
    executor: GroupMemberTurnExecutor,
    remote_executor: Option<GroupMemberTurnExecutor>,
) -> Result<LocalGroupFanoutDisposition, String> {
    dispatch_local_group_send_with_observer(
        sessions,
        runtime,
        room_id,
        expected_epoch,
        executor,
        remote_executor,
        None,
    )
}

pub fn dispatch_local_group_send_with_observer(
    sessions: Arc<ProductionSessionWorkers>,
    runtime: Arc<ProductionTranscriptRuntime>,
    room_id: &str,
    expected_epoch: u64,
    executor: GroupMemberTurnExecutor,
    remote_executor: Option<GroupMemberTurnExecutor>,
    entry_observer: Option<GroupRoomEntryObserver>,
) -> Result<LocalGroupFanoutDisposition, String> {
    let db_path = sessions.session_db_path(room_id)?;
    let agent_dir = db_path
        .parent()
        .ok_or_else(|| "group session database has no agent directory".to_string())?;
    let Some(config) = read_sand_group_config(agent_dir) else {
        return Ok(LocalGroupFanoutDisposition::NotGroup);
    };
    let remote_member_count = config
        .remote_members
        .as_ref()
        .map(Vec::len)
        .unwrap_or_default();
    if (config.shared_room_id.is_some() || remote_member_count > 0) && remote_executor.is_none() {
        return Ok(LocalGroupFanoutDisposition::DeferredRemote {
            shared_room_id: config.shared_room_id,
            remote_member_count,
        });
    }

    let profile = sessions
        .get_agent_profile_text(room_id)?
        .unwrap_or_else(|| crate::agents::agent_profile::SandAgentProfile {
            name: "Grok".into(),
            description: String::new(),
            title: String::new(),
            avatar_shape: String::new(),
            avatar_color: String::new(),
        });
    let group = GroupDescription {
        name: if profile.name.trim().is_empty() {
            "Grok".to_string()
        } else {
            profile.name
        },
        description: profile.description,
    };
    let posted_messages = Arc::new(Mutex::new(0usize));
    let member_failures = Arc::new(Mutex::new(Vec::new()));
    let post_error = Arc::new(Mutex::new(None));
    let deps = LocalGroupFanoutDeps {
        sessions,
        runtime,
        room_id: room_id.to_string(),
        expected_epoch,
        config: config.clone(),
        executor,
        remote_executor,
        posted_messages: Arc::clone(&posted_messages),
        member_failures: Arc::clone(&member_failures),
        post_error: Arc::clone(&post_error),
        entry_observer,
    };
    let mut member_ids = deps.config.member_ids.clone();
    if deps.remote_executor.is_some() {
        member_ids.extend(
            deps.config
                .remote_members
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(remote_member_id),
        );
    }
    futures::executor::block_on(
        GroupChatOrchestrator::new(deps).run(&group, &member_ids),
    );
    if let Some(error) = post_error.lock().ok().and_then(|mut slot| slot.take()) {
        return Err(error);
    }
    Ok(LocalGroupFanoutDisposition::Completed {
        posted_messages: posted_messages.lock().map(|value| *value).unwrap_or_default(),
        member_failures: member_failures.lock().map(|value| value.clone()).unwrap_or_default(),
    })
}

fn remote_member_id(member: &crate::groups::group_store::RemoteGroupMember) -> String {
    let owner =
        url::form_urlencoded::byte_serialize(member.owner_auth_id.as_bytes()).collect::<String>();
    let agent = url::form_urlencoded::byte_serialize(member.agent_id.as_bytes()).collect::<String>();
    format!("sand-remote:{owner}/{agent}")
}

fn transcript_entry_to_group_message(entry: Value) -> Option<GroupMessage> {
    match entry.get("kind").and_then(Value::as_str) {
        Some("message")
            if entry.get("role").and_then(Value::as_str) == Some("user")
                && entry
                    .get("content")
                    .and_then(Value::as_str)
                    .is_some_and(|content| !content.trim().is_empty()) =>
        {
            Some(GroupMessage {
                speaker: GroupSpeaker::User {
                    name: entry
                        .get("fromUser")
                        .and_then(Value::as_object)
                        .and_then(|from_user| from_user.get("name"))
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                },
                content: entry.get("content")?.as_str()?.to_string(),
            })
        }
        Some("send-message")
            if entry
                .get("message")
                .and_then(Value::as_object)
                .and_then(|message| message.get("type"))
                .and_then(Value::as_str)
                == Some("text")
                && entry.get("streaming").and_then(Value::as_bool) != Some(true) =>
        {
            let author = entry.get("author")?.as_object()?;
            let id = author.get("id")?.as_str()?.to_string();
            let name = author.get("name")?.as_str()?.to_string();
            let content = entry
                .get("message")?
                .get("content")?
                .as_str()?
                .to_string();
            Some(GroupMessage {
                speaker: GroupSpeaker::Member { id, name },
                content,
            })
        }
        _ => None,
    }
}

pub fn collect_new_member_send_messages(
    before: &[Value],
    after: &[Value],
) -> Vec<String> {
    let before_ids = before
        .iter()
        .filter_map(|entry| entry.get("id").and_then(Value::as_str))
        .collect::<std::collections::HashSet<_>>();
    after
        .iter()
        .filter(|entry| {
            entry
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| !before_ids.contains(id))
        })
        .filter(|entry| entry.get("kind").and_then(Value::as_str) == Some("send-message"))
        .filter_map(|entry| entry.get("message"))
        .filter(|message| message.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|message| message.get("content").and_then(Value::as_str))
        .map(str::trim)
        .filter(|content| !content.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1000.0
}
