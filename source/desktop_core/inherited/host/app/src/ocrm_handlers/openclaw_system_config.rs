use mahayana_host_runtime::gateway_server::GatewayCommandError;
use crate::{
    process_uptime_ms, report_agent_roster_security_findings, started_at_ms, UnifiedGatewayApi,
};

pub(crate) fn call_openclaw_config_get(
    api: &UnifiedGatewayApi,
    args: &serde_json::Value,
) -> Result<serde_json::Value, GatewayCommandError> {
    let settings = api.settings.get_host_settings();
    let serialized = serde_json::to_string(&settings)
        .map_err(|e| GatewayCommandError::Internal(e.to_string()))?;
    let hash = mahayana_host_runtime::sha256::sha256_hex(serialized.as_bytes());
    let matches = args.get("baseHash")
        .and_then(serde_json::Value::as_str)
        .map(|bh| bh == hash);

    let mut res = serde_json::json!({
        "settings": settings.clone(),
        "config": settings,
        "hash": hash,
        "configSource": api.settings.get_config_source_metadata(),
    });
    if let Some(m) = matches {
        res["matches"] = serde_json::json!(m);
    }
    Ok(res)
}

pub(crate) fn call_openclaw_config_set(
    api: &UnifiedGatewayApi,
    args: &serde_json::Value,
) -> Result<serde_json::Value, GatewayCommandError> {
    let parsed_raw: Option<serde_json::Value> = if let Some(raw) = args.get("raw").and_then(serde_json::Value::as_str) {
        Some(serde_json::from_str(raw).map_err(|e| GatewayCommandError::BadRequest(format!("malformed json in raw: {e}")))?)
    } else {
        None
    };
    let target = parsed_raw.as_ref()
        .or_else(|| args.get("config"))
        .or_else(|| args.get("settings"))
        .ok_or_else(|| GatewayCommandError::BadRequest(
            "config.set requires raw, config, or settings".into(),
        ))?;
    if !target.is_object() {
        return Err(GatewayCommandError::BadRequest(
            "config.set target must be a JSON object".into(),
        ));
    }

    if let Some(base_hash) = args.get("baseHash").and_then(serde_json::Value::as_str) {
        let current = api.settings.get_host_settings();
        let current_str = serde_json::to_string(&current)
            .map_err(|e| GatewayCommandError::Internal(e.to_string()))?;
        let current_hash = mahayana_host_runtime::sha256::sha256_hex(current_str.as_bytes());
        if base_hash != current_hash {
            return Err(GatewayCommandError::Conflict(format!(
                "config baseHash mismatch: expected {current_hash}, got {base_hash}"
            )));
        }
    }

    let permission_changed = target.get("localToolPermission").is_some();
    let result = api.settings.set_host_settings(target)
        .map_err(|e| GatewayCommandError::Internal(format!("Failed to persist settings: {e}")))?;
    report_agent_roster_security_findings(&result);
    api.roster_bookkeeping
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .apply_config(&result);
    if permission_changed {
        api.local_tool_permission.note_permission_changed();
    }

    let serialized = serde_json::to_string(&result)
        .map_err(|e| GatewayCommandError::Internal(e.to_string()))?;
    let hash = mahayana_host_runtime::sha256::sha256_hex(serialized.as_bytes());

    Ok(serde_json::json!({
        "ok": true,
        "config": result.clone(),
        "settings": result,
        "hash": hash,
        "configSource": api.settings.get_config_source_metadata(),
    }))
}

pub(crate) fn call_openclaw_system_info(
    _api: &UnifiedGatewayApi,
    _args: &serde_json::Value,
) -> Result<serde_json::Value, GatewayCommandError> {
    mahayana_host_runtime::host_diagnostics::report_diagnostic_event(
        "system.info.queried",
        serde_json::json!({ "timestamp": started_at_ms() }),
    );
    let pid = std::process::id();
    let cpu_count = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let memory = mahayana_host_runtime::host_system_info::sample_host_memory()
        .map_err(GatewayCommandError::Internal)?;
    let release = mahayana_host_runtime::host_system_info::sample_host_os_release()
        .map_err(GatewayCommandError::Internal)?;
    let hostname = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(Ok)
        .unwrap_or_else(|| mahayana_host_runtime::host_system_info::sample_host_hostname())
        .map_err(GatewayCommandError::Internal)?;
    let uptime_ms = process_uptime_ms();

    Ok(serde_json::json!({
        "machineName": hostname.clone(),
        "hostname": hostname,
        "platform": std::env::consts::OS,
        "release": release,
        "arch": std::env::consts::ARCH,
        "osLabel": std::env::consts::OS,
        "pid": pid,
        "cpuCount": cpu_count,
        "memoryTotalBytes": memory.total_bytes,
        "memoryFreeBytes": memory.available_bytes,
        "uptimeMs": uptime_ms,
        "processInstanceId": format!("sand-host-{pid}"),
        "nodeVersion": env!("CARGO_PKG_VERSION"),
    }))
}

pub(crate) fn provider_review_projection(
    api: &UnifiedGatewayApi,
    agent_id: &str,
    session_key: &str,
    session_id: &str,
) -> Result<Option<serde_json::Value>, GatewayCommandError> {
    let Some(record) = api
        .session_workers
        .read_session_work_admission(agent_id)
        .map_err(GatewayCommandError::Internal)?
    else {
        return Ok(None);
    };
    let Some(review) = record.provider_review else {
        return Ok(None);
    };
    if record.session_id != session_id
        || record.session_key != session_key
        || review.session_id != session_id
    {
        return Ok(None);
    }
    let can_continue = record.provider_review_phase.as_deref() == Some("pending")
        && session_key == format!("agent:{agent_id}:main")
        && review.provider == "openai"
        && review
            .continuation_message
            .as_deref()
            .is_some_and(|message| !message.trim().is_empty())
        && ((review.runtime_id == "codex"
            && review
                .native_thread_id
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            && review
                .native_turn_id
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty()))
            || (review.runtime_id == "openclaw"
                && review.api.as_deref() == Some("openai-chatgpt-responses")));
    let mut projection = serde_json::json!({
        "id": review.id,
        "runId": review.run_id,
        "explanation": review.explanation,
        "canContinue": can_continue,
    });
    if can_continue {
        if let Some(message) = review
            .continuation_message
            .as_deref()
            .filter(|message| !message.trim().is_empty())
        {
            projection["continuationMessage"] =
                serde_json::Value::String(message.to_string());
        }
    }
    Ok(Some(projection))
}
