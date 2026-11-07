use std::sync::Arc;

use serde_json::Value;

use crate::r#box::box_transfer::TransferBox;
use crate::extensions::inference::provider_session::ProviderMessage;

use super::box_tool_access::RunnerBoxResourcePort;
use super::large_output_spill::{
    MCP_TEXT_FILE_THRESHOLD_BYTES, is_large_output_spill_enabled, maybe_spill_mcp_text_result,
};
use super::prompt_collector_glue::{
    PromptCollectorDynamicUserContext, ProviderPromptProjection,
    append_mcp_runtime_sections_for_turn, append_profile_system_section_for_turn,
    append_remote_runtime_sections_for_turn, apply_dynamic_user_context_for_turn,
    prepend_unconfirmed_user_messages_with_watermark_and_collect_for_turn,
    project_provider_messages_for_turn,
};
use super::sand_agent_profile_prompt::{AgentProfileIdentity, AgentProfilePromptSnapshot};
use super::shell_terminal_watch::{MaterializedUserMessage, WatermarkResult};
use super::system_prompt_assembly::{ComputerPromptState, RemoteBoxPromptState};
use super::tools::sand_file_transfer_tools::FileTransferController;
use super::tools::sand_spotlight_tools::spotlight_prompt_section;


#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunnerPromptProfileState {
    pub system_section: Option<String>,
    pub profile_update: Option<String>,
    pub announcement: Option<(AgentProfilePromptSnapshot, AgentProfileIdentity)>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunnerPromptMcpState {
    pub installed_servers: Vec<Value>,
    pub discovery_unavailable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerPromptRemoteState {
    pub remote_box: RemoteBoxPromptState,
    pub computer: ComputerPromptState,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunnerPromptAutomationState {
    pub status_reminder: Option<String>,
    pub compaction_epoch: u64,
    pub is_silence_allowed: bool,
}

pub type RunnerPromptWatermarkGetter =
    Arc<dyn Fn() -> Result<WatermarkResult, String> + Send + Sync>;
pub type RunnerPromptProfileGetter =
    Arc<dyn Fn() -> Result<RunnerPromptProfileState, String> + Send + Sync>;
pub type RunnerPromptMcpGetter =
    Arc<dyn Fn() -> Result<RunnerPromptMcpState, String> + Send + Sync>;
pub type RunnerPromptRemoteGetter =
    Arc<dyn Fn() -> Result<RunnerPromptRemoteState, String> + Send + Sync>;
pub type RunnerPromptAutomationGetter =
    Arc<dyn Fn() -> Result<RunnerPromptAutomationState, String> + Send + Sync>;
pub type RunnerPromptAutomationCommit =
    Arc<dyn Fn(Option<String>, u64) + Send + Sync>;
pub type RunnerPromptFeatureGate = Arc<dyn Fn() -> bool + Send + Sync>;

/// The production Runner-owned join for prompt inputs that remain mutable
/// outside the Runner. Canonical Session/MCP/Box/Automation/Experiments owners
/// retain their state; this join resolves each value at the phase that consumes
/// it instead of snapshotting those services when the Host constructs a turn.
#[derive(Clone)]
pub struct RunnerPromptGlueOwner {
    pub is_subagent_runner: bool,
    pub confirmed_user_watermark_for_turn: RunnerPromptWatermarkGetter,
    pub profile_for_turn: RunnerPromptProfileGetter,
    pub mcp_for_turn: RunnerPromptMcpGetter,
    pub remote_for_turn: RunnerPromptRemoteGetter,
    pub automation_for_turn: RunnerPromptAutomationGetter,
    pub note_automation_status: RunnerPromptAutomationCommit,
    pub spotlight_enabled_for_turn: RunnerPromptFeatureGate,
}

impl RunnerPromptGlueOwner {
    pub fn project_provider_messages(
        &self,
        args: &Value,
        messages: &[ProviderMessage],
    ) -> ProviderPromptProjection {
        project_provider_messages_for_turn(args, messages)
    }

    pub fn prepend_unconfirmed_user_messages(
        &self,
        args: &Value,
        messages: &mut Vec<ProviderMessage>,
    ) -> Result<Vec<MaterializedUserMessage>, String> {
        let watermark = (self.confirmed_user_watermark_for_turn)()?;
        Ok(prepend_unconfirmed_user_messages_with_watermark_and_collect_for_turn(
            args,
            messages,
            &watermark,
        ))
    }

    pub fn append_profile(
        &self,
        messages: &mut Vec<ProviderMessage>,
    ) -> Result<RunnerPromptProfileState, String> {
        let profile = (self.profile_for_turn)()?;
        if let Some(section) = profile.system_section.as_deref() {
            append_profile_system_section_for_turn(messages, section);
        }
        Ok(profile)
    }

    pub fn append_live_runtime_sections(
        &self,
        messages: &mut Vec<ProviderMessage>,
        profile_update: Option<&str>,
    ) -> Result<(), String> {
        let mcp = (self.mcp_for_turn)()?;
        append_mcp_runtime_sections_for_turn(
            messages,
            &mcp.installed_servers,
            mcp.discovery_unavailable,
            self.is_subagent_runner,
        );

        let remote = (self.remote_for_turn)()?;
        append_remote_runtime_sections_for_turn(
            messages,
            &remote.remote_box,
            &remote.computer,
        );

        let automation = (self.automation_for_turn)()?;
        let dynamic_applied = apply_dynamic_user_context_for_turn(
            messages,
            PromptCollectorDynamicUserContext {
                automation_status_reminder: automation.status_reminder.as_deref(),
                profile_update,
                is_silence_allowed: automation.is_silence_allowed,
            },
        );
        if dynamic_applied && automation.status_reminder.is_some() {
            (self.note_automation_status)(
                automation.status_reminder.clone(),
                automation.compaction_epoch,
            );
        }

        if (self.spotlight_enabled_for_turn)() {
            messages.push(ProviderMessage {
                role: "system".into(),
                content: spotlight_prompt_section(true),
            });
        }
        Ok(())
    }

    pub fn spotlight_enabled(&self) -> bool {
        (self.spotlight_enabled_for_turn)()
    }
}

#[derive(Debug, Clone)]
pub struct RunnerPromptGlue<AgentBoxType, UserBoxType> {
    file_transfer_controller: FileTransferController<AgentBoxType, UserBoxType>,
    mcp_text_spill_enabled: bool,
}

impl<AgentBoxType, UserBoxType> RunnerPromptGlue<AgentBoxType, UserBoxType> {
    pub fn new(
        file_transfer_controller: FileTransferController<AgentBoxType, UserBoxType>,
        mcp_text_spill_enabled: bool,
    ) -> Self {
        Self {
            file_transfer_controller,
            mcp_text_spill_enabled,
        }
    }

    pub fn from_environment(
        file_transfer_controller: FileTransferController<AgentBoxType, UserBoxType>,
    ) -> Self {
        Self::new(file_transfer_controller, is_large_output_spill_enabled())
    }

    pub fn file_transfer_controller(&self) -> &FileTransferController<AgentBoxType, UserBoxType> {
        &self.file_transfer_controller
    }

    pub fn project_provider_messages(
        &self,
        args: &Value,
        messages: &[ProviderMessage],
    ) -> ProviderPromptProjection {
        project_provider_messages_for_turn(args, messages)
    }

    pub fn mcp_text_spill_enabled(&self) -> bool {
        self.mcp_text_spill_enabled
    }

    pub fn spill_mcp_text_result(
        &self,
        box_resources: &dyn RunnerBoxResourcePort,
        result: Value,
        tool_call_id: &str,
    ) -> Value {
        self.spill_mcp_text_result_with_threshold(
            box_resources,
            result,
            tool_call_id,
            MCP_TEXT_FILE_THRESHOLD_BYTES,
        )
    }

    pub fn spill_mcp_text_result_with_threshold(
        &self,
        box_resources: &dyn RunnerBoxResourcePort,
        result: Value,
        tool_call_id: &str,
        threshold_bytes: usize,
    ) -> Value {
        if !self.mcp_text_spill_enabled {
            return result;
        }
        maybe_spill_mcp_text_result(
            box_resources,
            result,
            tool_call_id,
            threshold_bytes,
        )
    }
}

pub fn create_runner_prompt_glue<AgentBoxType, UserBoxType>(
    file_transfer_controller: FileTransferController<AgentBoxType, UserBoxType>,
) -> RunnerPromptGlue<AgentBoxType, UserBoxType> {
    RunnerPromptGlue::from_environment(file_transfer_controller)
}

pub fn assert_transfer_box_bound<Ctx, AgentBoxType, UserBoxType>(
    glue: &RunnerPromptGlue<AgentBoxType, UserBoxType>,
) where
    AgentBoxType: TransferBox<Ctx>,
    UserBoxType: TransferBox<Ctx>,
{
    let _ = glue.file_transfer_controller();
}
