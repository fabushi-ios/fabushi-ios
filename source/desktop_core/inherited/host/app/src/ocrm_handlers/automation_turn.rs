use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mahayana_host_runtime::extensions::automations::sand_automation_fire_consumer::FireCompletion;
use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderSessionError, RoutedProvider,
};
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::telemetry::agent_error_telemetry::AgentErrorReport;
use mahayana_host_runtime::extensions::telemetry::host_telemetry_service::HostStructuredLogTelemetry;
use mahayana_host_runtime::extensions::transcript::agent_lifecycle::{
    KickstartRunError, KickstartTurnOutcome,
};
use mahayana_host_runtime::extensions::transcript::automation_run_path::{
    AutomationExecutionResult, AutomationRunFenceSnapshot, FireAutomationOutcome,
};
use mahayana_host_runtime::extensions::transcript::production_runtime::ProductionSendError;
use mahayana_host_runtime::extensions::transcript::sand_upgrade_resume_store::UpgradeResumeMarker;
use mahayana_host_runtime::extensions::transcript::send_pipeline::PersistedSendContext;
use mahayana_host_runtime::extensions::trays::extension::HostTraysExtension;

use crate::{
    build_upgrade_resume_prompt, classify_agent_error, provider_failure_tray, sand_error_detail,
    start_routed_provider_task, LocalRoutedRunnerDeps, RUNNER_INFERENCE_EVENT_CHANNEL,
};

pub(crate) fn start_local_upgrade_resume_turn(
    deps: LocalRoutedRunnerDeps,
    provider: RoutedProvider,
    marker: &UpgradeResumeMarker,
    automation_wake: Option<serde_json::Value>,
) -> Result<(), String> {
    if provider == RoutedProvider::Cursor {
        return Err("Cursor upgrade resume stays on the compatibility production path".into());
    }

    let resumed_source = match marker.source.as_deref() {
        Some("notification") => "background-revival".to_string(),
        Some(source) if !source.trim().is_empty() => source.to_string(),
        _ => "handoff-resume".to_string(),
    };
    let prompt = build_upgrade_resume_prompt(&resumed_source);
    let session_key = if let Some(automation_id) = marker.automation_id.as_deref() {
        resolve_admitted_automation_session_key(
            &deps.session_workers,
            &marker.agent_id,
            automation_id,
        )?
    } else {
        deps.session_workers
            .read_session_work_admission(&marker.agent_id)?
            .map(|record| record.session_key)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                format!(
                    "upgrade resume requires an existing admitted session identity for {}",
                    marker.agent_id
                )
            })?
    };
    let stream_id = format!("upgrade-resume-{}", uuid::Uuid::new_v4());
    let client_nonce = format!(
        "upgrade-resume:{}:{}",
        marker.agent_id,
        uuid::Uuid::new_v4()
    );
    let ack_token = if matches!(resumed_source.as_str(), "turn" | "handoff-resume") {
        deps.ack_obligations
            .mint_ack_run_token(&marker.agent_id)
            .map_err(|error| error.to_string())?
    } else {
        None
    };

    let mut admission_args = serde_json::json!({
        "agentId": marker.agent_id,
        "sessionKey": session_key.clone(),
        "prompt": prompt,
        "clientNonce": client_nonce,
        "streamId": stream_id,
        "appendUserMessage": false,
        "hidden": true,
        "requestSource": "upgrade-resume",
        "ackToken": ack_token,
        "skipAckObligation": true,
    });
    if let Some(automation_wake) = automation_wake.clone() {
        admission_args["automationWake"] = automation_wake;
    }

    deps.session_workers
        .assert_session_work_admission(
            &marker.agent_id,
            admission_args.get("sessionKey").and_then(serde_json::Value::as_str),
        )
        .map_err(|error| error.to_string())?;

    if let Err(error) = deps.transcript_runtime.accept_routed_send(&admission_args, |_| {
        Ok::<_, ProductionSendError>(PersistedSendContext::default())
    }) {
        deps.ack_obligations
            .retire_ack_run_token(&marker.agent_id, ack_token.as_deref());
        return Err(error.to_string());
    }

    let mut runner_args = serde_json::json!({
        "provider": provider.as_str(),
        "agentId": marker.agent_id,
        "sessionKey": session_key,
        "streamId": stream_id,
        "requestSource": resumed_source,
        "hidden": true,
        "isSilenceAllowed": matches!(resumed_source.as_str(), "automation" | "background-revival"),
        "upgradeResume": true,
        "messages": [{
            "role": "user",
            "content": prompt,
        }],
    });
    if let Some(ack_token) = ack_token {
        runner_args["ackToken"] = serde_json::Value::String(ack_token);
    }
    if let Some(automation_wake) = automation_wake {
        runner_args["automationWake"] = automation_wake;
    }

    start_routed_provider_task(
        deps.routed_tool_relay,
        deps.mcp_service,
        deps.events.clone(),
        deps.host_tx,
        deps.data_dir,
        deps.request_context,
        deps.auth,
        deps.auto_review,
        deps.experiments,
        deps.settings,
        deps.inference,
        Arc::clone(&deps.session_workers),
        Arc::clone(&deps.runner_registry),
        deps.ack_obligations,
        deps.transcript_runtime,
        deps.transcript_manager,
        deps.generated_agent_runtime,
        deps.completion_revivals,
        deps.forever_box,
        deps.local_exec,
        deps.local_tool_permission,
        deps.session_handoff,
        deps.trays,
        deps.telemetry_logs,
        deps.product_analytics,
        deps.production_action_auditor,
        deps.cloud_agents,
        deps.cloud_agent_watches,
        deps.background_shell_watches,
        deps.host_runner_composition,
        deps.box_store_sync,
        Arc::clone(&deps.automations_lifecycle),
        Arc::clone(&deps.generated_subagent_cleanup),
        None,
        runner_args,
    )
    .map(|_| ())
    .map_err(|error| error.to_string())
}

pub(crate) fn listener_connection_state_or_disconnected(
    platform: &str,
    result: Result<bool, String>,
) -> bool {
    match result {
        Ok(connected) => connected,
        Err(_) => {
            eprintln!(
                "[sand:listener-integrations] {platform} connection read degraded to disconnected"
            );
            false
        }
    }
}

pub(crate) fn automation_fire_completion(
    result: Result<Option<FireAutomationOutcome>, String>,
) -> Option<FireCompletion> {
    match result {
        Ok(Some(FireAutomationOutcome::Ok)) => Some(FireCompletion::succeeded()),
        Ok(Some(FireAutomationOutcome::Error)) => Some(FireCompletion::failed(
            "Automation run failed on the Sand box",
        )),
        Ok(Some(FireAutomationOutcome::Interrupted)) => Some(FireCompletion::failed(
            "Automation run was interrupted on the Sand box",
        )),
        Ok(None) => None,
        Err(error) => Some(FireCompletion::failed(error)),
    }
}

pub(crate) fn automation_terminal_from_event(
    event: &serde_json::Value,
    stream_id: &str,
) -> Option<Result<AutomationExecutionResult, String>> {
    if event.get("channel").and_then(serde_json::Value::as_str)
        != Some(RUNNER_INFERENCE_EVENT_CHANNEL)
    {
        return None;
    }
    let payload = event.get("payload")?;
    if payload.get("streamId").and_then(serde_json::Value::as_str) != Some(stream_id) {
        return None;
    }
    match payload.get("type").and_then(serde_json::Value::as_str) {
        Some("completed") => Some(Ok(AutomationExecutionResult::Completed)),
        Some("cancelled") => Some(Ok(AutomationExecutionResult::Interrupted {
            detail: "Interrupted before it finished.".into(),
            quiesced_for_upgrade: false,
        })),
        Some("failed") => Some(Ok(AutomationExecutionResult::Failed {
            detail: payload
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("automation Runner failed")
                .to_string(),
        })),
        _ => None,
    }
}

pub(crate) fn resolve_admitted_automation_session_key(
    session_workers: &ProductionSessionWorkers,
    agent_id: &str,
    automation_id: &str,
) -> Result<String, String> {
    let expected_key = format!("agent:{agent_id}:main");
    let Some(record) = session_workers.read_session_work_admission(agent_id)? else {
        return Err(format!(
            "automation {automation_id} requires an existing owner-admitted session for {agent_id}"
        ));
    };
    if record.session_key != expected_key {
        return Err(format!(
            "automation {automation_id} has no owner-admitted main session identity for {agent_id}"
        ));
    }
    if record.session_key.trim().is_empty() {
        return Err(format!(
            "automation {automation_id} has an empty owner-admitted session identity for {agent_id}"
        ));
    }
    session_workers.assert_session_work_admission(agent_id, Some(&expected_key))?;
    Ok(record.session_key)
}

pub(crate) fn run_local_automation_turn(
    deps: LocalRoutedRunnerDeps,
    provider: RoutedProvider,
    agent_id: &str,
    automation_id: &str,
    automation_name: &str,
    prompt: &str,
) -> Result<AutomationExecutionResult, String> {
    if provider == RoutedProvider::Cursor {
        return Err("Cursor automation turns remain on the compatibility path".into());
    }
    let stream_id = format!("automation-{}", uuid::Uuid::new_v4());
    let client_nonce = format!(
        "automation:{}:{}:{}",
        agent_id,
        automation_id,
        uuid::Uuid::new_v4()
    );
    let automation_wake = serde_json::json!({
        "id": automation_id,
        "name": automation_name,
    });
    let session_key = resolve_admitted_automation_session_key(
        &deps.session_workers,
        agent_id,
        automation_id,
    )?;
    let admission_args = serde_json::json!({
        "agentId": agent_id,
        "sessionKey": session_key.clone(),
        "prompt": prompt,
        "clientNonce": client_nonce,
        "streamId": stream_id,
        "appendUserMessage": false,
        "hidden": true,
        "requestSource": "automation",
        "automationWake": automation_wake,
        "skipAckObligation": true,
    });
    deps.session_workers
        .assert_session_work_admission(
            agent_id,
            admission_args.get("sessionKey").and_then(serde_json::Value::as_str),
        )
        .map_err(|error| error.to_string())?;
    deps.transcript_runtime
        .accept_routed_send(&admission_args, |_| {
            Ok::<_, ProductionSendError>(PersistedSendContext::default())
        })
        .map_err(|error| error.to_string())?;

    let receiver = deps.events.subscribe();
    let runner_args = serde_json::json!({
        "provider": provider.as_str(),
        "agentId": agent_id,
        "sessionKey": session_key,
        "streamId": stream_id,
        "requestSource": "automation",
        "automationWake": {
            "id": automation_id,
            "name": automation_name,
        },
        "messages": [{
            "role": "user",
            "content": prompt,
        }],
    });
    let runner_deps = deps.clone();
    if let Err(error) = start_routed_provider_task(
        runner_deps.routed_tool_relay,
        runner_deps.mcp_service,
        runner_deps.events.clone(),
        runner_deps.host_tx,
        runner_deps.data_dir,
        runner_deps.request_context,
        runner_deps.auth,
        runner_deps.auto_review,
        runner_deps.experiments,
        runner_deps.settings,
        runner_deps.inference,
        Arc::clone(&runner_deps.session_workers),
        Arc::clone(&runner_deps.runner_registry),
        runner_deps.ack_obligations,
        runner_deps.transcript_runtime,
        runner_deps.transcript_manager,
        runner_deps.generated_agent_runtime,
        runner_deps.completion_revivals,
        runner_deps.forever_box,
        runner_deps.local_exec,
        runner_deps.local_tool_permission,
        runner_deps.session_handoff,
        runner_deps.trays,
        runner_deps.telemetry_logs,
        runner_deps.product_analytics,
        runner_deps.production_action_auditor,
        runner_deps.cloud_agents,
        runner_deps.cloud_agent_watches,
        runner_deps.background_shell_watches,
        runner_deps.host_runner_composition,
        runner_deps.box_store_sync,
        Arc::clone(&runner_deps.automations_lifecycle),
        Arc::clone(&runner_deps.generated_subagent_cleanup),
        None,
        runner_args,
    )
    .map_err(|error| error.to_string()) {
        return Err(error);
    }

    let deadline = Instant::now() + Duration::from_secs(30 * 60);
    loop {
        let now = Instant::now();
        if now >= deadline {
            let _ = deps
                .runner_registry
                .cancel_stream(&stream_id, "automation turn timed out");
            return Ok(attach_automation_run_fences(
                &deps,
                agent_id,
                &stream_id,
                AutomationExecutionResult::Interrupted {
                    detail: "Interrupted before it finished.".into(),
                    quiesced_for_upgrade: deps.runner_registry.is_quiescing_for_upgrade(),
                },
            ));
        }
        let wait = deadline
            .saturating_duration_since(now)
            .min(Duration::from_millis(250));
        match receiver.recv_timeout(wait) {
            Ok(event) => {
                if let Some(mut result) = automation_terminal_from_event(&event, &stream_id) {
                    if let Ok(AutomationExecutionResult::Interrupted {
                        quiesced_for_upgrade,
                        ..
                    }) = &mut result
                    {
                        *quiesced_for_upgrade = deps.runner_registry.is_quiescing_for_upgrade();
                    }
                    return Ok(attach_automation_run_fences(
                        &deps,
                        agent_id,
                        &stream_id,
                        result?,
                    ));
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("Host event bus disconnected during automation turn".into());
            }
        }
    }
}

pub(crate) fn attach_automation_run_fences(
    deps: &LocalRoutedRunnerDeps,
    agent_id: &str,
    stream_id: &str,
    result: AutomationExecutionResult,
) -> AutomationExecutionResult {
    let Some(fences) = read_automation_run_fences(deps, agent_id, stream_id) else {
        return result;
    };
    match result {
        AutomationExecutionResult::Completed => {
            AutomationExecutionResult::CompletedWithFences { fences }
        }
        AutomationExecutionResult::Interrupted {
            detail,
            quiesced_for_upgrade,
        } => AutomationExecutionResult::InterruptedWithFences {
            detail,
            quiesced_for_upgrade,
            fences,
        },
        AutomationExecutionResult::Failed { detail } => {
            AutomationExecutionResult::FailedWithFences { detail, fences }
        }
        other => other,
    }
}

pub(crate) fn read_automation_run_fences(
    deps: &LocalRoutedRunnerDeps,
    agent_id: &str,
    stream_id: &str,
) -> Option<AutomationRunFenceSnapshot> {
    let admission = deps
        .session_workers
        .read_session_work_admission(agent_id)
        .ok()??;
    if admission
        .admitted_work_id
        .as_deref()
        .is_some_and(|work_id| work_id != stream_id)
    {
        return None;
    }
    let active_streams = deps.runner_registry.active_stream_ids_for_agent(agent_id);
    if active_streams.iter().any(|active| active != stream_id) {
        return None;
    }
    let unsettled_subagent = deps
        .generated_agent_runtime
        .lock()
        .ok()?
        .running_subagent_ids_for_parent(agent_id)
        .len()
        > 0;
    Some(AutomationRunFenceSnapshot {
        active_controller: active_streams.iter().any(|active| active == stream_id),
        pending_outgoing_media: admission.admitted_media_work_id.is_some(),
        unsettled_subagent,
    })
}

pub(crate) fn run_local_kickstart_turn(
    deps: LocalRoutedRunnerDeps,
    provider: RoutedProvider,
    agent_id: &str,
    source: &str,
    prompt: &str,
) -> Result<KickstartTurnOutcome, KickstartRunError> {
    let stream_id = format!("kickstart-{}", uuid::Uuid::new_v4());
    let client_nonce = format!("kickstart:{}:{}", agent_id, uuid::Uuid::new_v4());
    let session_key = deps
        .session_workers
        .read_session_work_admission(agent_id)
        .map_err(|error| KickstartRunError {
            request_id: Some(stream_id.clone()),
            message: error,
        })?
        .map(|record| record.session_key)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| KickstartRunError {
            request_id: Some(stream_id.clone()),
            message: format!(
                "kickstart requires an existing admitted session identity for {agent_id}"
            ),
        })?;
    let admission_args = serde_json::json!({
        "agentId": agent_id,
        "sessionKey": session_key.clone(),
        "prompt": prompt,
        "clientNonce": client_nonce,
        "streamId": stream_id,
        "appendUserMessage": false,
        "hidden": true,
        "requestSource": source,
        "skipAckObligation": true,
    });
    deps.session_workers
        .assert_session_work_admission(
            agent_id,
            admission_args.get("sessionKey").and_then(serde_json::Value::as_str),
        )
        .map_err(|error| KickstartRunError {
            request_id: Some(stream_id.clone()),
            message: error.to_string(),
        })?;
    deps.transcript_runtime
        .accept_routed_send(&admission_args, |_| {
            Ok::<_, ProductionSendError>(PersistedSendContext::default())
        })
        .map_err(|error| KickstartRunError {
            request_id: Some(stream_id.clone()),
            message: error.to_string(),
        })?;

    let receiver = deps.events.subscribe();
    let runner_args = serde_json::json!({
        "provider": provider.as_str(),
        "agentId": agent_id,
        "sessionKey": session_key,
        "streamId": stream_id,
        "requestSource": source,
        "hidden": true,
        "messages": [{
            "role": "user",
            "content": prompt,
        }],
    });
    start_routed_provider_task(
        deps.routed_tool_relay,
        deps.mcp_service,
        deps.events.clone(),
        deps.host_tx,
        deps.data_dir,
        deps.request_context,
        deps.auth,
        deps.auto_review,
        deps.experiments,
        deps.settings,
        deps.inference,
        Arc::clone(&deps.session_workers),
        Arc::clone(&deps.runner_registry),
        deps.ack_obligations,
        deps.transcript_runtime.clone(),
        deps.transcript_manager,
        deps.generated_agent_runtime,
        deps.completion_revivals,
        deps.forever_box,
        deps.local_exec,
        deps.local_tool_permission,
        deps.session_handoff,
        deps.trays,
        deps.telemetry_logs,
        deps.product_analytics,
        deps.production_action_auditor,
        deps.cloud_agents,
        deps.cloud_agent_watches,
        deps.background_shell_watches,
        deps.host_runner_composition,
        deps.box_store_sync,
        Arc::clone(&deps.automations_lifecycle),
        Arc::clone(&deps.generated_subagent_cleanup),
        None,
        runner_args,
    )
    .map_err(|error| KickstartRunError {
        request_id: Some(stream_id.clone()),
        message: error.to_string(),
    })?;

    let deadline = Instant::now() + Duration::from_secs(30 * 60);
    loop {
        let now = Instant::now();
        if now >= deadline {
            let _ = deps
                .runner_registry
                .cancel_stream(&stream_id, "created-agent kickstart timed out");
            return Ok(KickstartTurnOutcome {
                aborted: true,
                quiesced_for_upgrade: false,
                sent_message_count: 0,
                reacted: false,
            });
        }
        let wait = deadline
            .saturating_duration_since(now)
            .min(Duration::from_millis(250));
        match receiver.recv_timeout(wait) {
            Ok(event) => {
                if event.get("channel").and_then(serde_json::Value::as_str)
                    != Some(RUNNER_INFERENCE_EVENT_CHANNEL)
                {
                    continue;
                }
                let Some(payload) = event.get("payload") else {
                    continue;
                };
                if payload.get("streamId").and_then(serde_json::Value::as_str)
                    != Some(stream_id.as_str())
                {
                    continue;
                }
                match payload.get("type").and_then(serde_json::Value::as_str) {
                    Some("completed") => {
                        return Ok(KickstartTurnOutcome {
                            aborted: false,
                            quiesced_for_upgrade: false,
                            sent_message_count: payload
                                .get("sentMessageCount")
                                .and_then(serde_json::Value::as_u64)
                                .and_then(|count| usize::try_from(count).ok())
                                .unwrap_or_default(),
                            reacted: payload
                                .get("reacted")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(false),
                        });
                    }
                    Some("cancelled") => {
                        return Ok(KickstartTurnOutcome {
                            aborted: true,
                            quiesced_for_upgrade: deps
                                .transcript_runtime
                                .is_quiescing_for_upgrade(),
                            sent_message_count: 0,
                            reacted: false,
                        });
                    }
                    Some("failed") => {
                        return Err(KickstartRunError {
                            request_id: Some(stream_id.clone()),
                            message: payload
                                .get("message")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("created-agent kickstart Runner failed")
                                .to_string(),
                        });
                    }
                    _ => {}
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(KickstartRunError {
                    request_id: Some(stream_id.clone()),
                    message: "Host event bus disconnected during created-agent kickstart".into(),
                });
            }
        }
    }
}

pub(crate) fn report_handoff_resume_error(
    telemetry_logs: &HostStructuredLogTelemetry,
    trays: &HostTraysExtension,
    agent_id: &str,
    request_id: Option<&str>,
    title: &str,
    error: &str,
) {
    let classified_error = ProviderSessionError::Tool(error.to_string());
    let report = AgentErrorReport {
        source: "resume".into(),
        conversation_id: agent_id.to_string(),
        request_id: request_id.map(str::to_string),
        error: classify_agent_error(&classified_error),
        detail: Some(sand_error_detail(error)),
    };
    if let Err(telemetry_error) = telemetry_logs.report_agent_error(&report) {
        eprintln!(
            "mahayana-host handoff_resume_telemetry_failed agent={} error={telemetry_error}",
            agent_id
        );
    }
    let mut tray = provider_failure_tray(agent_id, error, crate::started_at_ms() as i64);
    tray.title = title.to_string();
    trays.push_error(tray);
}
