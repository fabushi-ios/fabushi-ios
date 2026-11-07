use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};

use mahayana_host_runtime::r#box::box_transfer::TransferBox;
use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderMessage, ProviderSessionError,
};
use mahayana_host_runtime::runner::box_tool_access::{
    RunnerBoxReadRequest, RunnerBoxResourcePort, RunnerBoxShellRequest,
    RunnerBoxWriteRequest,
};
use mahayana_host_runtime::runner::runner_prompt_glue::{
    RunnerPromptAutomationState, RunnerPromptGlue, RunnerPromptGlueOwner,
    RunnerPromptMcpState, RunnerPromptProfileState, RunnerPromptRemoteState,
};
use mahayana_host_runtime::runner::sand_agent_profile_prompt::{
    AgentProfileIdentity, AgentProfilePromptSnapshot,
};
use mahayana_host_runtime::runner::system_prompt_assembly::{
    ComputerPromptState, RemoteBoxPromptState, RunnerPromptRole,
};
use mahayana_host_runtime::runner::tools::sand_file_transfer_tools::{
    FileTransferController, UserComputerHandle,
};
use serde_json::{Value, json};
use mahayana_host_runtime::runner::shell_terminal_watch::WatermarkResult;

#[derive(Default)]
struct MemoryBox {
    files: Mutex<HashMap<String, Vec<u8>>>,
}
impl TransferBox<()> for MemoryBox {
    type Error = io::Error;
    fn download_file(
        &self,
        _ctx: &(),
        _agent_id: &str,
        path: &str,
    ) -> Result<Vec<u8>, Self::Error> {
        self.files
            .lock()
            .unwrap()
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "missing"))
    }
    fn upload_file(
        &self,
        _ctx: &(),
        _agent_id: &str,
        path: &str,
        data: &[u8],
    ) -> Result<(), Self::Error> {
        self.files
            .lock()
            .unwrap()
            .insert(path.to_string(), data.to_vec());
        Ok(())
    }
}

#[derive(Default)]
struct BoxResources {
    writes: Mutex<Vec<RunnerBoxWriteRequest>>,
}
impl RunnerBoxResourcePort for BoxResources {
    fn execute_shell(
        &self,
        _request: RunnerBoxShellRequest,
    ) -> Result<Value, ProviderSessionError> {
        unreachable!()
    }
    fn execute_read(
        &self,
        _request: RunnerBoxReadRequest,
    ) -> Result<Value, ProviderSessionError> {
        unreachable!()
    }
    fn execute_write(
        &self,
        request: RunnerBoxWriteRequest,
    ) -> Result<(), ProviderSessionError> {
        self.writes.lock().unwrap().push(request);
        Ok(())
    }
}

fn transfer_controller() -> FileTransferController<MemoryBox, MemoryBox> {
    let agent = Arc::new(MemoryBox::default());
    let computer = Arc::new(MemoryBox::default());
    FileTransferController {
        agent_box: agent,
        user_computers: vec![UserComputerHandle {
            id: "mac".into(),
            label: "Mac".into(),
            connected: true,
            box_: computer,
        }],
        default_computer_id: Some("mac".into()),
        computer_agent_id: "computer-agent".into(),
        box_id: "box-agent".into(),
        box_preparing: false,
    }
}

#[test]
fn glue_reuses_file_transfer_owner_and_prompt_collector_projection() {
    let glue = RunnerPromptGlue::new(transfer_controller(), false);
    assert_eq!(glue.file_transfer_controller().box_id, "box-agent");
    let projected = glue.project_provider_messages(
        &json!({
            "messageId":"user-7",
            "attachmentPaths":["/tmp/a.txt"],
            "boxPathByHostPath":{"/tmp/a.txt":"/workspace/uploads/a.txt"}
        }),
        &[ProviderMessage {
            role: "user".into(),
            content: "Read this".into(),
        }],
    );
    let user = projected
        .messages
        .iter()
        .find(|message| message.role == "user")
        .expect("projected user");
    assert!(user.content.contains("user-7"));
    assert!(user.content.contains("/workspace/uploads/a.txt"));
}

#[test]
fn glue_applies_single_large_output_spill_policy() {
    let resources = BoxResources::default();
    let result = json!({
        "content":[{"type":"text","text":"0123456789abcdef"}]
    });

    let disabled = RunnerPromptGlue::new(transfer_controller(), false);
    let unchanged = disabled.spill_mcp_text_result_with_threshold(
        &resources,
        result.clone(),
        "tool-1",
        4,
    );
    assert_eq!(unchanged, result);
    assert!(resources.writes.lock().unwrap().is_empty());

    let enabled = RunnerPromptGlue::new(transfer_controller(), true);
    let spilled = enabled.spill_mcp_text_result_with_threshold(
        &resources,
        result,
        "tool-2",
        4,
    );
    assert_eq!(resources.writes.lock().unwrap().len(), 1);
    assert_eq!(spilled["content"][0]["type"], "text");
    assert_eq!(spilled["content"][0]["text"], "");
    assert!(spilled["content"][0]["outputLocation"]["filePath"]
        .as_str()
        .unwrap()
        .starts_with(".sand/tools/"));
}


#[test]
fn live_owner_reads_mutable_prompt_inputs_at_phase_time() {
    let profile_name = Arc::new(Mutex::new("Live Agent".to_string()));
    let mcp_state = Arc::new(Mutex::new(RunnerPromptMcpState {
        installed_servers: vec![json!({
            "name":"Acme",
            "status":"connected",
            "customInstructions":"Use the live rows."
        })],
        discovery_unavailable: false,
    }));
    let remote_available = Arc::new(Mutex::new(true));
    let automation_text = Arc::new(Mutex::new(Some("Routine live now.".to_string())));
    let committed = Arc::new(Mutex::new(Vec::<Option<String>>::new()));

    let owner = RunnerPromptGlueOwner {
        is_subagent_runner: false,
        confirmed_user_watermark_for_turn: Arc::new(|| {
            Ok(WatermarkResult {
                last_user_message_id: None,
                has_user_turn: false,
            })
        }),
        profile_for_turn: {
            let profile_name = Arc::clone(&profile_name);
            Arc::new(move || -> Result<RunnerPromptProfileState, String> {
                let name = profile_name.lock().unwrap().clone();
                let identity = AgentProfileIdentity {
                    name: name.clone(),
                    description: "live profile".into(),
                };
                Ok(RunnerPromptProfileState {
                    system_section: Some(format!("## Agent profile\nName: {name}")),
                    profile_update: Some(format!("profile-update:{name}")),
                    announcement: Some((
                        AgentProfilePromptSnapshot {
                            version: 1,
                            profile_section: format!("## Agent profile\nName: {name}"),
                            system_identity: identity.clone(),
                            announced_identity: AgentProfileIdentity::default(),
                            compaction_epoch: 3,
                        },
                        identity,
                    )),
                })
            })
        },
        mcp_for_turn: {
            let state = Arc::clone(&mcp_state);
            Arc::new(move || -> Result<RunnerPromptMcpState, String> {
                Ok(state.lock().unwrap().clone())
            })
        },
        remote_for_turn: {
            let available = Arc::clone(&remote_available);
            Arc::new(move || -> Result<RunnerPromptRemoteState, String> {
                let available = *available.lock().unwrap();
                Ok(RunnerPromptRemoteState {
                    remote_box: RemoteBoxPromptState {
                        role: RunnerPromptRole::Main,
                        available,
                        runtime_state: if available { "ready" } else { "offline" }.into(),
                        desktop_capable: true,
                        desktop_ready: available,
                    },
                    computer: ComputerPromptState {
                        role: RunnerPromptRole::Main,
                        box_available: available,
                        desktop_capable: true,
                        desktop_ready: available,
                        control_lease_active: false,
                        human_takeover_pending: false,
                        browser_use_offered: false,
                        window_index: Some(4),
                    },
                })
            })
        },
        automation_for_turn: {
            let text = Arc::clone(&automation_text);
            Arc::new(move || -> Result<RunnerPromptAutomationState, String> {
                Ok(RunnerPromptAutomationState {
                    status_reminder: text.lock().unwrap().clone(),
                    compaction_epoch: 9,
                    is_silence_allowed: false,
                })
            })
        },
        note_automation_status: {
            let committed = Arc::clone(&committed);
            Arc::new(move |reminder, _epoch| {
                committed.lock().unwrap().push(reminder);
            })
        },
        spotlight_enabled_for_turn: Arc::new(|| true),
    };

    let args = json!({"messageId":"msg-2"});
    let mut first = vec![ProviderMessage {
        role: "user".into(),
        content: "current".into(),
    }];
    let profile = owner.append_profile(&mut first).expect("profile");
    owner
        .append_live_runtime_sections(&mut first, profile.profile_update.as_deref())
        .expect("runtime sections");
    let first_text = first.iter().map(|row| row.content.as_str()).collect::<Vec<_>>().join("\n");
    assert!(first_text.contains("Live Agent"));
    assert!(first_text.contains("Use the live rows."));
    assert!(first_text.contains("Routine live now."));
    assert!(first_text.contains("## Your box"));
    assert!(first_text.contains("## Untrusted content"));
    assert_eq!(committed.lock().unwrap().as_slice(), &[Some("Routine live now.".into())]);

    *profile_name.lock().unwrap() = "Changed Agent".into();
    *remote_available.lock().unwrap() = false;
    *automation_text.lock().unwrap() = Some("Routine changed.".into());
    *mcp_state.lock().unwrap() = RunnerPromptMcpState {
        installed_servers: Vec::new(),
        discovery_unavailable: true,
    };

    let mut second = vec![ProviderMessage {
        role: "user".into(),
        content: "second".into(),
    }];
    let profile = owner.append_profile(&mut second).expect("changed profile");
    owner
        .append_live_runtime_sections(&mut second, profile.profile_update.as_deref())
        .expect("changed runtime sections");
    let second_text = second.iter().map(|row| row.content.as_str()).collect::<Vec<_>>().join("\n");
    assert!(second_text.contains("Changed Agent"));
    assert!(second_text.contains("Routine changed."));
    assert!(second_text.contains("<mcp_status>"));
    assert!(second_text.contains("unavailable this turn"));

    let projected = owner.project_provider_messages(
        &args,
        &[ProviderMessage { role: "user".into(), content: "hello".into() }],
    );
    assert!(projected.messages.iter().any(|row| row.content.contains("msg-2")));
}

#[test]
fn shipping_host_routes_mutable_prompt_surfaces_through_runner_owner() {
    const HOST: &str = include_str!("../app/src/main.rs");
    for needle in [
        "let prompt_owner = RunnerPromptGlueOwner",
        "prompt_owner.project_provider_messages(&args, &lifecycle_messages)",
        ".prepend_unconfirmed_user_messages(&args, &mut provider_messages)",
        ".append_profile(&mut provider_messages)",
        ".append_live_runtime_sections(",
        "prompt_owner.spotlight_enabled()",
    ] {
        assert!(
            HOST.contains(needle),
            "shipping Host must route mutable prompt surface through Runner owner: {needle}"
        );
    }
    for stale in [
        "project_provider_messages_for_turn(&args, &lifecycle_messages)",
        "append_mcp_runtime_sections_for_turn(\n        &mut provider_messages",
        "append_remote_runtime_sections_for_turn(\n        &mut provider_messages",
        "apply_dynamic_user_context_for_turn(\n        &mut provider_messages",
    ] {
        assert!(
            !HOST.contains(stale),
            "shipping Host must not bypass Runner prompt glue with {stale}"
        );
    }
}
