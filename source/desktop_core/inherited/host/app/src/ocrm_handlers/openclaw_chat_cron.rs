use std::sync::Arc;
use mahayana_host_runtime::extensions::session::agent_session::SandAgentSessionStore;
use mahayana_host_runtime::gateway_server::GatewayApi;
use mahayana_host_runtime::gateway_server::GatewayCommandError;
use crate::{
    reject_agent_wide_session_route, resolve_gateway_session_route, session_kind_label,
    started_at_ms, UnifiedGatewayApi,
};

pub(crate) fn call_openclaw_chat_send(
    api: &UnifiedGatewayApi,
    args: serde_json::Value,
) -> Result<serde_json::Value, GatewayCommandError> {
    let session_key = args
        .get("sessionKey")
        .or_else(|| args.get("sessionId"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| GatewayCommandError::BadRequest("chat.send requires sessionKey".into()))?;

    let route = resolve_gateway_session_route(&args, session_key)?;
    reject_agent_wide_session_route(&route, "chat.send")?;
    let session_kind = session_kind_label(route.kind);

    let message = args.get("message")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| GatewayCommandError::BadRequest("chat.send requires message".into()))?;

    let idempotency_key = args.get("idempotencyKey")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| GatewayCommandError::BadRequest("chat.send requires idempotencyKey".into()))?;

    let agent_id = route.agent_id.as_str();

    let agent_exists = api.session_workers
        .summarize_agent_by_id(agent_id, None)
        .map_err(GatewayCommandError::Internal)?
        .is_some();
    if !agent_exists {
        return Err(GatewayCommandError::BadRequest(format!("unknown agent session: {agent_id}")));
    }

    let active_streams_before = api.runner_registry.active_stream_ids_for_agent(agent_id);

    let prompt_args = serde_json::json!({
        "agentId": agent_id,
        "sessionKey": session_key,
        "chatType": args.get("chatType").cloned().unwrap_or(serde_json::Value::Null),
        "spawnedBy": args.get("spawnedBy").cloned().unwrap_or(serde_json::Value::Null),
        "prompt": message,
        "clientNonce": idempotency_key,
    });

    let send_result = api.call_send_prompt(prompt_args, None)?;
    let active_streams_after = api.runner_registry.active_stream_ids_for_agent(agent_id);
    let new_stream_id = active_streams_after
        .into_iter()
        .find(|stream_id| !active_streams_before.iter().any(|before| before == stream_id));
    let status = send_result
        .get("status")
        .cloned()
        .unwrap_or_else(|| serde_json::json!("accepted"));
    let message_seq = send_result.get("messageSeq").cloned();
    let interrupted_active_run = send_result.get("interruptedActiveRun").cloned();
    let mut response = serde_json::json!({
        "ok": true,
        "status": status,
        "kind": session_kind,
        "result": send_result,
    });
    if let Some(stream_id) = new_stream_id {
        response["runId"] = serde_json::Value::String(stream_id);
    }
    if let Some(message_seq) = message_seq {
        response["messageSeq"] = message_seq;
    }
    if let Some(interrupted_active_run) = interrupted_active_run {
        response["interruptedActiveRun"] = interrupted_active_run;
    }
    Ok(response)
}

pub(crate) fn call_openclaw_chat_abort(
    api: &UnifiedGatewayApi,
    args: &serde_json::Value,
) -> Result<serde_json::Value, GatewayCommandError> {
    let session_key = args
        .get("sessionKey")
        .or_else(|| args.get("sessionId"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| GatewayCommandError::BadRequest("chat.abort requires sessionKey".into()))?;

    let route = resolve_gateway_session_route(args, session_key)?;
    reject_agent_wide_session_route(&route, "chat.abort")?;
    let session_kind = session_kind_label(route.kind);
    let agent_id = route.agent_id.as_str();
    if agent_id.is_empty() {
        return Err(GatewayCommandError::BadRequest(
            "chat.abort requires a non-empty agent identity".into(),
        ));
    }

    let active_streams = api.runner_registry.active_stream_ids_for_agent(agent_id);
    let requested_run_id = match args.get("runId") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => Some(
            value
                .as_str()
                .map(str::trim)
                .filter(|run_id| !run_id.is_empty())
                .ok_or_else(|| {
                    GatewayCommandError::BadRequest(
                        "chat.abort runId must be a non-empty string".into(),
                    )
                })?,
        ),
    };
    let run_ids = active_streams
        .into_iter()
        .filter(|active_id| requested_run_id.is_none_or(|run_id| active_id == run_id))
        .collect::<Vec<_>>();
    let cancelled_count = run_ids
        .iter()
        .filter(|run_id| api.runner_registry.cancel_stream(run_id, "rpc"))
        .count();

    Ok(serde_json::json!({
        "ok": true,
        "aborted": cancelled_count > 0,
        "kind": session_kind,
        "runIds": run_ids,
    }))
}

pub(crate) fn call_openclaw_cron_list(
    api: &UnifiedGatewayApi,
    args: &serde_json::Value,
) -> Result<serde_json::Value, GatewayCommandError> {
    let store = SandAgentSessionStore::new(Arc::clone(&api.session_workers));
    let agents = store.list_agents().map_err(GatewayCommandError::Internal)?;

    let mut jobs = Vec::new();
    for agent in agents {
        let records = store
            .list_agent_automations(&agent.id)
            .map_err(GatewayCommandError::Internal)?;
        for r in records {
            let schedule = if r.schedule.trim().is_empty() {
                serde_json::json!({ "kind": "event" })
            } else {
                serde_json::json!({
                    "kind": "cron",
                    "expr": r.schedule,
                    "tz": "UTC"
                })
            };
            let last_run = r.runs.first();
            let updated_at_ms = r
                .last_run_at
                .unwrap_or(r.created_at)
                .clamp(i64::MIN as f64, i64::MAX as f64) as i64;
            jobs.push(serde_json::json!({
                "id": r.id,
                "jobId": r.id,
                "name": r.name,
                "description": r.prompt,
                "enabled": r.is_enabled,
                "createdAtMs": r.created_at as i64,
                "updatedAtMs": updated_at_ms,
                "agentId": agent.id,
                "schedule": schedule,
                "sessionTarget": "main",
                "wakeMode": "now",
                "payload": {
                    "kind": "agentTurn",
                    "message": r.prompt
                },
                "state": {
                    "nextRunAtMs": r.next_run_at.map(|v| v as i64),
                    "lastRunAtMs": r.last_run_at.map(|v| v as i64),
                    "lastRunStatus": last_run.map(|run| run.status.clone()),
                    "lastRunError": last_run.and_then(|run| run.detail.clone())
                }
            }));
        }
    }

    let total = jobs.len();
    let limit = args.get("limit").and_then(serde_json::Value::as_u64).unwrap_or(50) as usize;
    let offset = args.get("offset").and_then(serde_json::Value::as_u64).unwrap_or(0) as usize;

    let paged_jobs: Vec<serde_json::Value> = jobs.into_iter().skip(offset).take(limit).collect();
    Ok(serde_json::json!({
        "total": total,
        "hasMore": offset + paged_jobs.len() < total,
        "nextOffset": if offset + paged_jobs.len() < total { Some(offset + paged_jobs.len()) } else { None },
        "jobs": paged_jobs,
    }))
}

pub(crate) fn call_openclaw_cron_trigger(
    api: &UnifiedGatewayApi,
    args: &serde_json::Value,
) -> Result<serde_json::Value, GatewayCommandError> {
    let job_id = args.get("id")
        .or_else(|| args.get("jobId"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| GatewayCommandError::BadRequest("cron.trigger requires id or jobId".into()))?;

    let store = SandAgentSessionStore::new(Arc::clone(&api.session_workers));
    let agents = store.list_agents().map_err(GatewayCommandError::Internal)?;
    let mut target_agent_id = None;
    let mut previous_run_id = None;

    for agent in &agents {
        let records = store
            .list_agent_automations(&agent.id)
            .map_err(GatewayCommandError::Internal)?;
        if let Some(record) = records.iter().find(|r| r.id == job_id) {
            target_agent_id = Some(agent.id.clone());
            previous_run_id = record.runs.first().map(|run| run.id.clone());
            break;
        }
    }

    let Some(agent_id) = target_agent_id else {
        return Err(GatewayCommandError::BadRequest(format!("unknown cron job id: {job_id}")));
    };

    let run_args = serde_json::json!({
        "agentId": agent_id,
        "automationId": job_id,
    });
    let _ = api.call("runAgentAutomationNow", run_args)?;

    let observed = store
        .list_agent_automations(&agent_id)
        .map_err(GatewayCommandError::Internal)?
        .into_iter()
        .find(|record| record.id == job_id)
        .and_then(|record| {
            record
                .runs
                .first()
                .filter(|run| previous_run_id.as_deref() != Some(run.id.as_str()))
                .cloned()
        });

    let now = started_at_ms();
    let finished = observed.as_ref().and_then(|run| run.finished_at).is_some();
    let mut response = serde_json::json!({
        "ok": true,
        "enqueued": true,
        "processInstanceId": format!("sand-host-{}", std::process::id()),
        "finished": finished,
        "run": {
            "jobId": job_id,
            "action": if finished { "finished" } else { "accepted" },
            "status": observed.as_ref().map(|run| run.status.as_str()).unwrap_or("accepted"),
            "detail": observed.as_ref().and_then(|run| run.detail.as_deref()),
            "ts": now,
        }
    });
    if let Some(run) = observed {
        let run_id = serde_json::Value::String(run.id);
        response["runId"] = run_id.clone();
        response["run"]["runId"] = run_id;
    }
    Ok(response)
}
