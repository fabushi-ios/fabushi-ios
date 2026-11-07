use std::sync::Arc;
use mahayana_host_runtime::gateway_server::{GatewayCommandContext, GatewayCommandError};
use mahayana_host_runtime::extensions::session::session_paths::is_root_main_session_key;
use mahayana_host_runtime::extensions::transcript::agent_lifecycle::AgentLifecycleGatewayError;
use crate::{
    reject_agent_wide_session_route, resolve_gateway_session_route, session_kind_label,
    started_at_ms, UnifiedGatewayApi,
};
use super::openclaw_system_config::provider_review_projection;

pub(crate) fn call_openclaw_sessions_list(
    api: &UnifiedGatewayApi,
    args: &serde_json::Value,
) -> Result<serde_json::Value, GatewayCommandError> {
    let active = api.transcript_manager.active_agent_id();
    let summaries = api.session_workers
        .list_agent_summaries(active.as_deref())
        .map_err(GatewayCommandError::Internal)?;

    if summaries.iter().any(|summary| summary.lifecycle_revision.trim().is_empty()) {
        return Err(GatewayCommandError::Internal(
            "session lifecycle revision unavailable".into(),
        ));
    }

    let limit = args.get("limit").and_then(serde_json::Value::as_u64).unwrap_or(100) as usize;
    let offset = args.get("offset").and_then(serde_json::Value::as_u64).unwrap_or(0) as usize;

    let total = summaries.len();
    let rows: Vec<serde_json::Value> = summaries
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|s| {
            let key = format!("agent:{}:main", s.id);
            let route = resolve_gateway_session_route(
                &serde_json::json!({
                    "chatType": if s.is_group { "group" } else { "direct" },
                }),
                &key,
            )?;
            let kind = session_kind_label(route.kind);
            let active_run_ids = api.runner_registry.active_stream_ids_for_agent(&s.id);
            let has_active_run = !active_run_ids.is_empty();
            let status = if s.is_running || has_active_run { "running" } else { "done" };
            let mut row = serde_json::json!({
                "key": key,
                "sessionId": s.id,
                "agentId": s.id,
                "lifecycleRevision": s.lifecycle_revision,
                "kind": kind,
                "displayName": s.name,
                "updatedAt": s.updated_at,
                "pinned": false,
                "unread": s.has_unread,
                "status": status,
                "hasActiveRun": has_active_run,
                "activeRunIds": active_run_ids,
            });
            if let Some(review) = provider_review_projection(
                api,
                &s.id,
                &key,
                &s.id,
            )? {
                row["providerReview"] = review;
            }
            Ok(row)
        })
        .collect::<Result<_, GatewayCommandError>>()?;

    Ok(serde_json::json!({
        "sessions": rows,
        "total": total,
        "offset": offset,
        "limit": limit,
    }))
}

pub(crate) fn call_openclaw_sessions_get(
    api: &UnifiedGatewayApi,
    args: &serde_json::Value,
) -> Result<serde_json::Value, GatewayCommandError> {
    let raw_key = args.get("key")
        .or_else(|| args.get("sessionKey"))
        .or_else(|| args.get("sessionId"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .unwrap_or_default();

    if raw_key.is_empty() {
        return Err(GatewayCommandError::BadRequest(
            "sessions.get requires key, sessionKey, or sessionId".into(),
        ));
    }

    let route = resolve_gateway_session_route(args, raw_key)?;
    reject_agent_wide_session_route(&route, "sessions.get")?;
    let agent_id = route.agent_id.as_str();
    let canonical_key = format!("agent:{agent_id}:main");
    let session_kind = session_kind_label(route.kind);

    let summary = api
        .session_workers
        .summarize_agent_by_id(agent_id, None)
        .map_err(GatewayCommandError::Internal)?;

    if summary
        .as_ref()
        .is_some_and(|value| value.lifecycle_revision.trim().is_empty())
    {
        return Err(GatewayCommandError::Internal(
            "session lifecycle revision unavailable".into(),
        ));
    }

    let entries = api
        .session_workers
        .read_agent_transcript_entries(agent_id)
        .map_err(GatewayCommandError::Internal)?;

    let limit = args.get("limit").and_then(serde_json::Value::as_u64).unwrap_or(200) as usize;
    let start = entries.len().saturating_sub(limit);
    let mut messages = Vec::new();
    for entry in entries.into_iter().skip(start) {
        let id = entry.get("id").and_then(serde_json::Value::as_str).unwrap_or_default().to_string();
        let role = entry
            .get("role")
            .or_else(|| entry.get("source"))
            .and_then(serde_json::Value::as_str)
            .or_else(|| (entry.get("kind").and_then(serde_json::Value::as_str) == Some("send-message")).then_some("user"))
            .unwrap_or("user")
            .to_string();
        let text = entry
            .get("text")
            .or_else(|| entry.get("content"))
            .or_else(|| entry.get("message").and_then(|message| message.get("content")))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let timestamp = entry
            .get("timestamp")
            .or_else(|| entry.get("createdAt"))
            .or_else(|| entry.get("timestampMs"))
            .and_then(|value| value.as_i64().or_else(|| value.as_f64().map(|value| value as i64)))
            .unwrap_or(0);
        messages.push(serde_json::json!({
            "id": id,
            "role": role,
            "text": text,
            "timestamp": timestamp,
        }));
    }

    let mut response = serde_json::json!({
        "messages": messages,
        "key": raw_key,
        "kind": session_kind,
        "sessionId": summary.as_ref().map(|value| value.id.clone()),
        "updatedAt": summary.as_ref().map(|value| value.updated_at),
        "lifecycleRevision": summary
            .as_ref()
            .map(|value| value.lifecycle_revision.clone()),
    });
    if let Some(session_id) = summary.as_ref().map(|value| value.id.as_str())
        && let Some(review) = provider_review_projection(api, agent_id, &canonical_key, session_id)?
    {
        response["providerReview"] = review;
    }
    Ok(response)
}

pub(crate) fn call_openclaw_sessions_create(
    api: &UnifiedGatewayApi,
    args: &serde_json::Value,
) -> Result<serde_json::Value, GatewayCommandError> {
    let display_name = args.get("displayName")
        .or_else(|| args.get("label"))
        .or_else(|| args.get("name"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("New Session");

    let create_args = serde_json::json!({
        "name": display_name,
        "title": display_name,
    });

    let minted = api.transcript_manager
        .dispatch_agent_lifecycle_gateway_call(
            &api.agent_deletion_runtime,
            Some(api.created_agent_kickstart_hook()),
            "createAgent",
            &create_args,
        )
        .ok_or_else(|| GatewayCommandError::Internal("createAgent not dispatched".into()))?
        .map_err(|e| match e {
            AgentLifecycleGatewayError::BadRequest(m) => GatewayCommandError::BadRequest(m),
            AgentLifecycleGatewayError::Internal(m) => GatewayCommandError::Internal(m),
        })?;

    let agent_id = minted.get("agent")
        .and_then(|a| a.get("id"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| GatewayCommandError::Internal(
            "createAgent returned no durable agent identity".into(),
        ))?;

    let session_key = format!("agent:{agent_id}:main");
    Ok(serde_json::json!({
        "ok": true,
        "key": session_key,
        "sessionId": agent_id,
        "entry": minted,
        "runStarted": false,
    }))
}

pub(crate) fn call_openclaw_sessions_delete(
    api: &UnifiedGatewayApi,
    args: &serde_json::Value,
) -> Result<serde_json::Value, GatewayCommandError> {
    let raw_key = args.get("key")
        .or_else(|| args.get("sessionKey"))
        .or_else(|| args.get("sessionId"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .unwrap_or_default();

    if raw_key.is_empty() {
        return Err(GatewayCommandError::BadRequest("sessions.delete requires key or sessionId".into()));
    }

    let route = resolve_gateway_session_route(args, raw_key)?;
    reject_agent_wide_session_route(&route, "sessions.delete")?;

    if is_root_main_session_key(raw_key) {
        return Err(GatewayCommandError::BadRequest("Cannot delete root main session".into()));
    }

    let agent_id = route.agent_id.as_str();
    let session_kind = session_kind_label(route.kind);
    if agent_id.is_empty() {
        return Err(GatewayCommandError::BadRequest(
            "sessions.delete requires a non-empty agent identity".into(),
        ));
    }

    if let Some(expected) = args.get("expectedSessionId").and_then(serde_json::Value::as_str) {
        if expected != agent_id {
            return Err(GatewayCommandError::BadRequest("SESSION_LIFECYCLE_CHANGED".into()));
        }
    }

    let mut delete_args = serde_json::json!({ "id": agent_id });
    if let Some(expected) = args
        .get("expectedSessionId")
        .and_then(serde_json::Value::as_str)
    {
        delete_args["expectedSessionId"] = serde_json::Value::String(expected.to_string());
    }
    if let Some(expected) = args
        .get("expectedLifecycleRevision")
        .and_then(serde_json::Value::as_str)
    {
        delete_args["expectedLifecycleRevision"] =
            serde_json::Value::String(expected.to_string());
    }
    if args.get("expectedSessionUpdatedAt").is_some() {
        let expected = args
            .get("expectedSessionUpdatedAt")
            .and_then(serde_json::Value::as_f64)
            .filter(|value| value.is_finite())
            .ok_or_else(|| {
                GatewayCommandError::BadRequest(
                    "expectedSessionUpdatedAt must be a finite number".into(),
                )
            })?;
        delete_args["expectedSessionUpdatedAt"] = serde_json::Value::from(expected);
    }
    let result = api.transcript_manager
        .dispatch_agent_lifecycle_gateway_call(
            &api.agent_deletion_runtime,
            None,
            "deleteAgent",
            &delete_args,
        )
        .ok_or_else(|| GatewayCommandError::Internal("deleteAgent not dispatched".into()))?
        .map_err(|e| match e {
            AgentLifecycleGatewayError::BadRequest(m) => GatewayCommandError::BadRequest(m),
            AgentLifecycleGatewayError::Internal(m) => GatewayCommandError::Internal(m),
        })?;

    api.host_runner_composition.forget_local_tool_permission(agent_id);
    api.delete_production_automation_schedules(agent_id);
    api.refresh_production_automations();

    Ok(serde_json::json!({
        "ok": true,
        "key": format!("agent:{agent_id}:main"),
        "sessionId": agent_id,
        "kind": session_kind,
        "deleted": true,
        "result": result,
    }))
}

pub(crate) fn call_openclaw_sessions_patch(
    api: &UnifiedGatewayApi,
    args: &serde_json::Value,
    context: &GatewayCommandContext,
) -> Result<serde_json::Value, GatewayCommandError> {
    if !context.authenticated {
        return Err(GatewayCommandError::Forbidden(
            "sessions.patch requires an authenticated Gateway request".into(),
        ));
    }
    let raw_key = args
        .get("key")
        .or_else(|| args.get("sessionKey"))
        .or_else(|| args.get("sessionId"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| GatewayCommandError::BadRequest(
            "sessions.patch requires key or sessionKey".into(),
        ))?;
    let policy = args.get("communicationPolicy").map(|value| {
        serde_json::from_value::<
            mahayana_host_runtime::extensions::session::agent_session::SessionCommunicationPolicyOverride,
        >(value.clone())
        .map_err(|error| {
            GatewayCommandError::BadRequest(format!(
                "sessions.patch communicationPolicy is invalid: {error}"
            ))
        })
    }).transpose()?;
    let archived = match args.get("archived") {
        Some(value) => Some(value.as_bool().ok_or_else(|| {
            GatewayCommandError::BadRequest(
                "sessions.patch archived must be a boolean".into(),
            )
        })?),
        None => None,
    };
    if policy.is_some() && archived.is_some() {
        return Err(GatewayCommandError::BadRequest(
            "sessions.patch cannot combine communicationPolicy with archived".into(),
        ));
    }
    if policy.is_none() && archived.is_none() {
        return Err(GatewayCommandError::BadRequest(
            "sessions.patch requires archived or communicationPolicy".into(),
        ));
    }
    let route = resolve_gateway_session_route(args, raw_key)?;
    reject_agent_wide_session_route(&route, "sessions.patch")?;
    let expected_key = format!("agent:{}:main", route.agent_id);
    if !raw_key.eq_ignore_ascii_case(&expected_key) {
        return Err(GatewayCommandError::BadRequest(
            "sessions.patch only supports canonical main sessions".into(),
        ));
    }
    let expected_session_id = args
        .get("expectedSessionId")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| GatewayCommandError::BadRequest(
            "sessions.patch requires expectedSessionId".into(),
        ))?;
    if expected_session_id != route.agent_id.as_str() {
        return Err(GatewayCommandError::BadRequest(
            "SESSION_LIFECYCLE_CHANGED".into(),
        ));
    }
    let expected_revision = args
        .get("expectedLifecycleRevision")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| GatewayCommandError::BadRequest(
            "sessions.patch requires expectedLifecycleRevision".into(),
        ))?;
    if let Some(policy) = policy {
        api.session_workers
            .write_session_communication_policy(
                &route.agent_id,
                expected_session_id,
                expected_revision,
                &policy,
            )
            .map_err(|error| {
                if error == "SESSION_LIFECYCLE_CHANGED"
                    || error.contains("not admitted")
                {
                    GatewayCommandError::Conflict(error)
                } else {
                    GatewayCommandError::Internal(error)
                }
            })?;
        return Ok(serde_json::json!({
            "ok": true,
            "key": raw_key,
            "sessionId": expected_session_id,
            "communicationPolicy": policy,
            "lifecycleRevision": expected_revision,
        }));
    }
    let archived = archived.ok_or_else(|| {
        GatewayCommandError::BadRequest(
            "sessions.patch requires archived or communicationPolicy".into(),
        )
    })?;
    let reason = args
        .get("archiveReason")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if archived
        && !api
            .generated_agent_runtime
            .lock()
            .map_err(|_| {
                GatewayCommandError::Internal(
                    "generated subagent runtime lock is unavailable".into(),
                )
            })?
            .running_subagent_records_for_parent(&route.agent_id)
            .is_empty()
    {
        return Err(GatewayCommandError::Conflict(
            "session has active subagent work; archive requires subagent settlement before retry"
                .into(),
        ));
    }
    if archived && !api
        .runner_registry
        .active_stream_ids_for_agent(&route.agent_id)
        .is_empty()
    {
        return Err(GatewayCommandError::Conflict(
            "session has an active foreground turn; archive requires terminal settlement before retry".into(),
        ));
    }
    let record = api
        .session_workers
        .transition_session_work_archive(
            &route.agent_id,
            &expected_key,
            expected_session_id,
            Some(expected_revision),
            archived,
            Some("gateway-operator"),
            reason,
            started_at_ms() as f64,
        )
        .map_err(|error| {
            if error.contains("identity")
                || error.contains("lifecycle")
                || error.contains("admitted work")
            {
                GatewayCommandError::Conflict(error)
            } else {
                GatewayCommandError::Internal(error)
            }
        })?;
    Ok(serde_json::json!({
        "ok": true,
        "key": record.session_key,
        "sessionId": record.session_id,
        "archived": record.archived_at_ms.is_some(),
        "archivedAt": record.archived_at_ms,
        "archivedBy": record.archived_by,
        "archiveReason": record.archive_reason,
        "lifecycleRevision": record.lifecycle_revision,
    }))
}

pub(crate) fn delete_generated_subagent(
    api: &UnifiedGatewayApi,
    agent_id: &str,
    lifecycle_revision: &str,
) -> Result<(), String> {
    let args = serde_json::json!({
        "id": agent_id,
        "expectedSessionId": agent_id,
        "expectedLifecycleRevision": lifecycle_revision,
    });
    api.transcript_manager
        .dispatch_agent_lifecycle_gateway_call(
            &api.agent_deletion_runtime,
            None,
            "deleteAgent",
            &args,
        )
        .ok_or_else(|| "generated subagent delete owner unavailable".to_string())?
        .map_err(|error| match error {
            AgentLifecycleGatewayError::BadRequest(message)
            | AgentLifecycleGatewayError::Internal(message) => message,
        })?;
    api.host_runner_composition
        .forget_local_tool_permission(agent_id);
    api.delete_production_automation_schedules(agent_id);
    api.refresh_production_automations();
    Ok(())
}
