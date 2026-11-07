use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use mahayana_host_runtime::extensions::inference::provider_session::RoutedProvider;
use mahayana_host_runtime::extensions::transcript::group_chat_orchestrator::GroupMemberTurnRequest;
use mahayana_host_runtime::extensions::transcript::production_runtime::ProductionSendError;
use mahayana_host_runtime::extensions::transcript::send_group_fanout::{
    GroupMemberTurnExecutor, LocalGroupFanoutDisposition,
};
use mahayana_host_runtime::extensions::transcript::send_pipeline::PersistedSendContext;

use crate::{
    collect_new_member_send_messages, configured_routed_provider, group_room_entry_observer,
    should_redrive_group_member_after_preemption, start_routed_provider_task, started_at_ms,
    LocalRoutedRunnerDeps, GROUP_MEMBER_DM_PREEMPTED_ERROR, RUNNER_INFERENCE_EVENT_CHANNEL,
};

pub(crate) fn post_agent_message_to_group(
    deps: LocalRoutedRunnerDeps,
    from_agent_id: &str,
    group_id: &str,
    message: &str,
    is_priority: bool,
) -> Result<String, String> {
    post_agent_message_to_group_with_session(
        deps,
        None,
        from_agent_id,
        group_id,
        message,
        is_priority,
    )
}

pub(crate) fn post_agent_message_to_group_with_session(
    deps: LocalRoutedRunnerDeps,
    from_session_key: Option<&str>,
    from_agent_id: &str,
    group_id: &str,
    message: &str,
    is_priority: bool,
) -> Result<String, String> {
    if message == "(pass)" {
        return Ok("Nothing was posted: \"(pass)\" means staying silent in a group chat.".into());
    }
    if deps.transcript_runtime.is_quiescing_for_upgrade()
        || configured_routed_provider(&deps.data_dir.join("settings.json")).is_none()
    {
        return Err("Messaging isn't available right now.".into());
    }
    let posted = deps
        .transcript_manager
        .shared_rooms()
        .post_local_agent_message_with_session(
            from_agent_id,
            from_session_key,
            group_id,
            message,
            started_at_ms() as f64,
        )?;
    let _ = deps.product_analytics.track_event(
        "sand.agent_message.sent",
        &serde_json::json!({
            "from_agent_id": from_agent_id,
            "to_agent_id": group_id,
            "is_group_target": true,
            "is_priority": is_priority,
        }),
    );
    let room_id = group_id.to_string();
    thread::Builder::new()
        .name(format!("mahayana-agent-group-post-{room_id}"))
        .spawn(move || {
            if let Err(error) = run_agent_posted_group_turn(deps, &room_id) {
                eprintln!(
                    "mahayana-host agent_group_post_fanout_failed room={} error={error}",
                    room_id
                );
            }
        })
        .map_err(|error| format!("could not schedule group reply turns: {error}"))?;
    Ok(format!(
        "Posted to \"{}\". Its members will see it and reply on their own turns.",
        posted.group_name
    ))
}

pub(crate) fn run_agent_posted_group_turn(
    deps: LocalRoutedRunnerDeps,
    group_id: &str,
) -> Result<(), String> {
    let Some(provider) = configured_routed_provider(&deps.data_dir.join("settings.json")) else {
        return Err("no routed provider configured for agent-posted group turn".into());
    };
    let session_key = deps
        .session_workers
        .read_session_work_admission(group_id)?
        .map(|record| record.session_key)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            format!(
                "agent-posted group turn requires an existing admitted session identity for {group_id}"
            )
        })?;
    let args = serde_json::json!({
        "agentId": group_id,
        "sessionKey": session_key,
        "prompt": "[agent] group post",
        "clientNonce": format!("agent-group:{}:{}", group_id, uuid::Uuid::new_v4()),
        "appendUserMessage": false,
        "hidden": true,
        "requestSource": "agent-inbound",
        "skipAckObligation": true,
    });
    let dispatch_runtime = Arc::clone(&deps.transcript_runtime);
    let dispatch_group_chat = deps.transcript_manager.group_chat();
    let dispatch_deps = deps.clone();
    let dispatch_events = deps.events.clone();
    let room_id = group_id.to_string();
    deps.transcript_runtime
        .execute_send(
            &args,
            move || {
                let remote_executor = dispatch_runtime.shared_group_remote_executor();
                let member_deps = dispatch_deps.clone();
                let member_room_id = room_id.clone();
                let executor: GroupMemberTurnExecutor = Arc::new(move |request| {
                    run_local_group_member_turn(
                        member_deps.clone(),
                        provider,
                        Some(&member_room_id),
                        request,
                    )
                });
                let epoch = dispatch_runtime.current_turn_epoch(&room_id);
                match dispatch_group_chat.run_group_turn(
                    Arc::clone(&dispatch_runtime),
                    &room_id,
                    epoch,
                    executor,
                    remote_executor,
                    Some(group_room_entry_observer(dispatch_events.clone())),
                )
                .map_err(ProductionSendError::Internal)?
                {
                    LocalGroupFanoutDisposition::NotGroup => {
                        Err(ProductionSendError::Rejected(format!(
                            "{room_id} is not a group chat."
                        )))
                    }
                    LocalGroupFanoutDisposition::DeferredRemote { .. } => Ok(serde_json::json!({
                        "accepted": true,
                        "groupFanoutDeferred": true,
                    })),
                    LocalGroupFanoutDisposition::Completed {
                        posted_messages,
                        member_failures,
                    } => Ok(serde_json::json!({
                        "accepted": true,
                        "groupFanout": true,
                        "postedMessages": posted_messages,
                        "memberFailureCount": member_failures.len(),
                    })),
                }
            },
            |_| Ok(PersistedSendContext::default()),
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub(crate) fn run_local_group_member_turn(
    deps: LocalRoutedRunnerDeps,
    provider: RoutedProvider,
    group_room_id: Option<&str>,
    request: GroupMemberTurnRequest,
) -> Result<Vec<String>, String> {
    let member_id = request.member.id.clone();
    let member_name = request.member.name.clone();
    let group_room_id = group_room_id.map(ToOwned::to_owned);
    let session_key = match group_room_id.as_deref() {
        Some(room_id) => format!("agent:{member_id}:group:{room_id}"),
        None => deps
            .session_workers
            .read_session_work_admission(&member_id)?
            .map(|record| record.session_key)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                format!(
                    "group-member turn requires an existing admitted session identity for {member_id}"
                )
            })?,
    };
    let before = deps
        .session_workers
        .read_agent_transcript_entries(&member_id)?;
    let stream_id = format!("group-member-{}", uuid::Uuid::new_v4());
    let client_nonce = format!("group-member:{}:{}", member_id, uuid::Uuid::new_v4());
    let admission_args = serde_json::json!({
        "agentId": member_id,
        "sessionKey": session_key,
        "prompt": request.prompt,
        "clientNonce": client_nonce,
        "streamId": stream_id,
        "appendUserMessage": false,
        "requestSource": "group-member",
        "groupMemberTurn": true,
        "skipAckObligation": true,
    });
    deps.session_workers
        .assert_session_work_admission(
            &member_id,
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
        "agentId": member_id,
        "sessionKey": admission_args["sessionKey"].clone(),
        "streamId": stream_id,
        "requestSource": "group-member",
        "groupMemberTurn": true,
        "groupRoomId": group_room_id,
        "groupMemberName": member_name,
        "messages": [
            {
                "role": "system",
                "content": request.system_prompt,
            },
            {
                "role": "user",
                "content": request.prompt,
            }
        ],
    });
    if let Err(error) = start_routed_provider_task(
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
    .map_err(|error| error.to_string()) {
        return Err(error);
    }

    let deadline = Instant::now() + Duration::from_secs(30 * 60);
    loop {
        let now = Instant::now();
        if now >= deadline {
            let _ = deps.runner_registry.cancel_stream(
                &stream_id,
                "group member turn timed out",
            );
            return Err(format!("group member turn timed out: {member_id}"));
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
                if payload
                    .get("streamId")
                    .and_then(serde_json::Value::as_str)
                    != Some(stream_id.as_str())
                {
                    continue;
                }
                match payload.get("type").and_then(serde_json::Value::as_str) {
                    Some("completed") => break,
                    Some("failed" | "cancelled") => {
                        if deps.runner_registry.take_group_member_preempted(&member_id) {
                            let after = deps
                                .session_workers
                                .read_agent_transcript_entries(&member_id)?;
                            let sent = collect_new_member_send_messages(&before, &after);
                            let sent_message_count = payload
                                .get("sentMessageCount")
                                .and_then(serde_json::Value::as_u64)
                                .unwrap_or(sent.len() as u64)
                                .max(sent.len() as u64);
                            let reacted = payload
                                .get("reacted")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(false);
                            if !should_redrive_group_member_after_preemption(
                                sent_message_count,
                                reacted,
                            ) {
                                return Ok(sent);
                            }
                            return Err(GROUP_MEMBER_DM_PREEMPTED_ERROR.to_string());
                        }
                        return Err(payload
                            .get("message")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("group member Runner failed")
                            .to_string());
                    }
                    _ => {}
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("Host event bus disconnected during group member turn".into());
            }
        }
    }

    let after = deps
        .session_workers
        .read_agent_transcript_entries(&member_id)?;
    Ok(collect_new_member_send_messages(&before, &after))
}
