use std::collections::HashSet;

use serde_json::Value;
use uuid::Uuid;

use super::conversation_outline::{
    OutlineToolCall, get_outline_tool_call_name, get_outline_tool_call_status,
    get_outline_tool_call_summary, get_tool_call_activity_args,
};
use super::subagent_runtime::{
    SubagentLineage, compute_subagent_request_id, is_computer_use_subagent_type,
};
use super::{TurnEndedUsage, TurnUsage, turn_usage_from_turn_ended};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("{0}")]
pub struct SandSubagentDispatchError(pub String);

pub fn derive_sand_subagent_request_lineage(
    parent_request_id: Option<&str>,
    root_parent_request_id: Option<&str>,
    tool_call_id: &str,
) -> Option<SubagentLineage> {
    let parent_request_id = parent_request_id.filter(|value| !value.is_empty())?;
    Some(SubagentLineage {
        parent_request_id: Some(parent_request_id.to_string()),
        root_parent_request_id: Some(
            root_parent_request_id
                .filter(|value| !value.is_empty())
                .unwrap_or(parent_request_id)
                .to_string(),
        ),
        parent_agent_tool_call_id: (!tool_call_id.is_empty()).then(|| tool_call_id.to_string()),
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestContextInfo {
    pub os_version: Option<String>,
    pub shell: Option<String>,
    pub time_zone: Option<String>,
    pub transcripts_folder: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestContextProjection {
    pub os_version: Option<String>,
    pub shell: Option<String>,
    pub time_zone: Option<String>,
    pub agent_transcripts_folder: Option<String>,
    pub smart_mode_classifier_auto_mode_enabled: bool,
    pub rules: Vec<String>,
    pub rules_info_complete: bool,
    pub agent_skills: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandRequestContextExecutor {
    pub include_transcripts: bool,
    pub auto_review_enforce_enabled: bool,
}

impl SandRequestContextExecutor {
    pub fn execute(
        &self,
        info: RequestContextInfo,
        rules: Option<Vec<String>>,
        agent_skills: Vec<String>,
    ) -> RequestContextProjection {
        RequestContextProjection {
            os_version: info.os_version,
            shell: info.shell,
            time_zone: info.time_zone,
            agent_transcripts_folder: self
                .include_transcripts
                .then_some(info.transcripts_folder)
                .flatten(),
            smart_mode_classifier_auto_mode_enabled: self.auto_review_enforce_enabled,
            rules_info_complete: rules.is_some(),
            rules: rules.unwrap_or_default(),
            agent_skills,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SubagentAdapterArgs {
    pub resume_agent_id: Option<String>,
    pub subagent_type: String,
    pub tool_call_id: String,
    pub prompt: String,
    pub readonly: bool,
    pub selected_videos: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchReview {
    pub allowed: bool,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SubagentRunDispatch {
    pub subagent_agent_id: String,
    pub subagent_type: String,
    pub tool_call_id: String,
    pub prompt: String,
    pub lineage: Option<SubagentLineage>,
    pub inference_request_id: Option<String>,
    pub selected_videos: Vec<Value>,
    pub background_reason: &'static str,
    pub tool_call_count: usize,
}

#[derive(Default)]
pub struct SandSubagentHostAdapter {
    sessions: HashSet<String>,
    running: HashSet<String>,
    computer_window_owner: Option<String>,
}

impl SandSubagentHostAdapter {
    pub fn create_or_resume_session(
        &mut self,
        args: &SubagentAdapterArgs,
    ) -> Result<String, SandSubagentDispatchError> {
        if let Some(id) = args.resume_agent_id.as_deref() {
            if self.sessions.contains(id) {
                if self.running.contains(id) {
                    return Err(SandSubagentDispatchError(
                        "That background subagent is still running, so it can't be resumed yet — use MessageSubagent or StopSubagent while it runs.".to_string(),
                    ));
                }
                return Ok(id.to_string());
            }
        }

        let id = args
            .resume_agent_id
            .clone()
            .unwrap_or_else(|| format!("subagent-{}", Uuid::new_v4()));
        if is_computer_use_subagent_type(&args.subagent_type) {
            if self
                .computer_window_owner
                .as_deref()
                .is_some_and(|owner| owner != id)
            {
                return Err(SandSubagentDispatchError(
                    "A computerUse subagent is already using the box's desktop. Only one can run at a time.".to_string(),
                ));
            }
            self.computer_window_owner = Some(id.clone());
        }
        self.sessions.insert(id.clone());
        Ok(id)
    }

    pub fn run_session(
        &mut self,
        agent_id: &str,
        args: &SubagentAdapterArgs,
        parent_request_id: Option<&str>,
        root_parent_request_id: Option<&str>,
        review: Option<&LaunchReview>,
    ) -> Result<SubagentRunDispatch, String> {
        if !self.sessions.contains(agent_id) {
            return Err(format!("Unknown Fabushi subagent: {agent_id}"));
        }
        if self.running.contains(agent_id) {
            return Err("That background subagent is already running.".to_string());
        }
        if let Some(review) = review.filter(|review| !review.allowed) {
            self.release_session(agent_id);
            return Err(review.reason.clone());
        }
        let lineage = derive_sand_subagent_request_lineage(
            parent_request_id,
            root_parent_request_id,
            &args.tool_call_id,
        );
        self.running.insert(agent_id.to_string());
        Ok(SubagentRunDispatch {
            subagent_agent_id: agent_id.to_string(),
            subagent_type: if args.subagent_type.is_empty() {
                "generalPurpose".to_string()
            } else {
                args.subagent_type.clone()
            },
            tool_call_id: args.tool_call_id.clone(),
            prompt: args.prompt.clone(),
            lineage,
            inference_request_id: (!args.tool_call_id.is_empty())
                .then(|| compute_subagent_request_id(&args.tool_call_id)),
            selected_videos: args.selected_videos.clone(),
            background_reason: "agent-request",
            tool_call_count: 0,
        })
    }

    pub fn mark_settled(&mut self, agent_id: &str) {
        self.running.remove(agent_id);
    }

    pub fn release_session(&mut self, agent_id: &str) {
        if self.running.contains(agent_id) {
            return;
        }
        self.sessions.remove(agent_id);
        if self.computer_window_owner.as_deref() == Some(agent_id) {
            self.computer_window_owner = None;
        }
    }

    pub fn is_running(&self, agent_id: &str) -> bool {
        self.running.contains(agent_id)
    }

    pub fn has_session(&self, agent_id: &str) -> bool {
        self.sessions.contains(agent_id)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentUpdate {
    TextDelta(String),
    ThinkingDelta(String),
    TurnEnded(TurnEndedUsage),
    ToolCall {
        phase: String,
        call_id: String,
        tool_call: Option<OutlineToolCall>,
        model_call_id: Option<String>,
    },
    SummaryStarted,
    SummaryCompleted,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardedUpdate {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        text: String,
    },
    TurnEnded {
        usage: Option<TurnUsage>,
    },
    ToolCall {
        id: String,
        name: String,
        status: String,
        summary: Option<String>,
        args: Option<String>,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForwardingOutcome {
    pub updates: Vec<ForwardedUpdate>,
    pub surface_unresolved_pending: Option<(String, ForwardedUpdate)>,
    pub summary_lifecycle: bool,
}

pub fn forward_agent_update(
    canceled: bool,
    update: AgentUpdate,
    resolved_tool_name: Option<&str>,
) -> ForwardingOutcome {
    match update {
        AgentUpdate::TextDelta(text) => ForwardingOutcome {
            updates: vec![ForwardedUpdate::TextDelta { text }],
            ..ForwardingOutcome::default()
        },
        AgentUpdate::ThinkingDelta(text) => ForwardingOutcome {
            updates: vec![ForwardedUpdate::ThinkingDelta { text }],
            ..ForwardingOutcome::default()
        },
        AgentUpdate::TurnEnded(usage) => ForwardingOutcome {
            updates: vec![ForwardedUpdate::TurnEnded {
                usage: turn_usage_from_turn_ended(&usage),
            }],
            ..ForwardingOutcome::default()
        },
        AgentUpdate::ToolCall {
            phase,
            call_id,
            tool_call,
            ..
        } => {
            let Some(tool_call) = tool_call else {
                return ForwardingOutcome::default();
            };
            let outline_name = get_outline_tool_call_name(&tool_call);
            let name = resolved_tool_name.unwrap_or(&outline_name).to_string();
            let forwarded = ForwardedUpdate::ToolCall {
                id: call_id.clone(),
                name: name.clone(),
                status: get_outline_tool_call_status(&phase, &tool_call).to_string(),
                summary: get_outline_tool_call_summary(&tool_call),
                args: get_tool_call_activity_args(&tool_call),
            };
            let unresolved = matches!(forwarded, ForwardedUpdate::ToolCall { ref status, .. } if status == "pending")
                && matches!(name.as_str(), "shellToolCall" | "readToolCall" | "awaitToolCall");
            ForwardingOutcome {
                updates: vec![forwarded.clone()],
                surface_unresolved_pending: unresolved.then_some((call_id, forwarded)),
                summary_lifecycle: false,
            }
        }
        AgentUpdate::SummaryStarted | AgentUpdate::SummaryCompleted => ForwardingOutcome {
            summary_lifecycle: !canceled,
            ..ForwardingOutcome::default()
        },
        AgentUpdate::Other => ForwardingOutcome::default(),
    }
}
