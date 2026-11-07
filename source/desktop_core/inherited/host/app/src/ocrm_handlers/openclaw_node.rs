use std::sync::Arc;
use mahayana_host_runtime::extensions::local_exec::gateway_local_exec_sand_box::GatewayLocalToolScope;
use mahayana_host_runtime::extensions::node_host::invocation_runtime::NodeHostInvocationRuntime;
use mahayana_host_runtime::extensions::node_host::invoke_protocol::{
    NodeInvokeRequest, NodeInvokeResult, MAX_PARAMS_JSON_BYTES,
};
use mahayana_host_runtime::gateway_server::GatewayCommandError;
use crate::{started_at_ms, UnifiedGatewayApi};

pub(crate) fn call_openclaw_node_register(
    api: &UnifiedGatewayApi,
    args: &serde_json::Value,
) -> Result<serde_json::Value, GatewayCommandError> {
    let node_id = args.get("nodeId")
        .or_else(|| args.get("client").and_then(|c| c.get("id")))
        .or_else(|| args.get("device").and_then(|d| d.get("id")))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| GatewayCommandError::BadRequest(
            "node.register requires nodeId, client.id, or device.id".into(),
        ))?;

    // The local host is the only node that can be admitted without a
    // pairing request. Remote nodes must enter the canonical pairing
    // owner so registration cannot manufacture a connected node that is
    // absent from the durable device roster.
    if node_id != "sand-local-node" {
        let mut pairing_args = args.clone();
        if pairing_args
            .get("deviceId")
            .and_then(serde_json::Value::as_str)
            .is_none()
        {
            pairing_args["deviceId"] = serde_json::Value::String(node_id.to_string());
        }
        let pending = api.device_pairing.handle_pair_request(&pairing_args)?;
        return Ok(serde_json::json!({
            "ok": true,
            "status": pending
                .get("status")
                .cloned()
                .unwrap_or_else(|| serde_json::json!("pending")),
            "nodeId": node_id,
            "requestId": pending.get("requestId").cloned().unwrap_or(serde_json::Value::Null),
            "pairing": pending,
        }));
    }

    let node = api
        .node_host_runtime
        .get(node_id)
        .ok_or_else(|| GatewayCommandError::Internal("local node session is not admitted".into()))?;
    Ok(serde_json::json!({
        "ok": true,
        "status": "approved",
        "nodeId": node.node_id,
        "displayName": node.display_name,
        "platform": node.platform,
        "deviceFamily": node.device_family,
        "commands": node.commands,
        "capabilities": node.capabilities,
        "paired": node.paired,
        "connected": node.connected,
        "active": node.active,
        "gatewayLocal": node.gateway_local,
        "generation": node.generation,
    }))
}

pub(crate) fn call_openclaw_node_list(
    api: &UnifiedGatewayApi,
    _args: &serde_json::Value,
) -> Result<serde_json::Value, GatewayCommandError> {
    let pair_list = api.device_pairing.handle_pair_list(&serde_json::json!({}))?;

    // The session registry retains disconnected snapshots so generation
    // fencing and invocation cleanup can reconcile late callbacks. Those
    // historical rows are not live transport evidence and must not mask
    // the authoritative paired-device projection below.
    let live_nodes = api
        .node_host_runtime
        .list()
        .into_iter()
        .filter(|node| node.connected)
        .collect::<Vec<_>>();
    let mut nodes = live_nodes
        .iter()
        .map(|node| {
            serde_json::json!({
                "nodeId": node.node_id,
                "displayName": node.display_name,
                "platform": node.platform,
                "deviceFamily": node.device_family,
                "commands": node.commands,
                "connected": node.connected,
                "paired": node.paired,
                "active": node.active,
                "gatewayLocal": node.gateway_local,
                "generation": node.generation,
                "capabilities": node.capabilities
            })
        })
        .collect::<Vec<_>>();
    let live_node_ids = live_nodes
        .iter()
        .map(|node| node.node_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let active_node_id = live_nodes
        .iter()
        .find(|node| node.active)
        .map(|node| node.node_id.clone());

    if let Some(paired) = pair_list.get("paired").and_then(serde_json::Value::as_array) {
        for p in paired {
            if let Some(dev_id) = p.get("deviceId").and_then(serde_json::Value::as_str) {
                if live_node_ids.contains(dev_id) {
                    continue;
                }
                nodes.push(serde_json::json!({
                    "nodeId": dev_id,
                    "displayName": p.get("displayName").and_then(serde_json::Value::as_str).unwrap_or(dev_id),
                    "platform": p.get("platform").and_then(serde_json::Value::as_str).unwrap_or("unknown"),
                    "deviceFamily": "remote",
                    "commands": [],
                    "connected": false,
                    "paired": true,
                    "active": false,
                    "gatewayLocal": false,
                    "capabilities": []
                }));
            }
        }
    }

    Ok(serde_json::json!({
        "ts": started_at_ms(),
        "activeNodeId": active_node_id,
        "nodes": nodes
    }))
}

pub(crate) fn call_openclaw_node_status(
    api: &UnifiedGatewayApi,
    args: &serde_json::Value,
) -> Result<serde_json::Value, GatewayCommandError> {
    let node_id = args.get("nodeId")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| GatewayCommandError::BadRequest("node.status requires nodeId".into()))?;

    if let Some(node) = api.node_host_runtime.get(node_id) {
        // A retained disconnected remote snapshot is historical transport
        // state; resolve the durable pairing projection below instead of
        // reporting stale session metadata. The local node remains owned
        // by this runtime even if it is temporarily disconnected.
        if node.connected || node.gateway_local {
            return Ok(serde_json::json!({
                "ts": started_at_ms(),
                "nodeId": node.node_id,
                "displayName": node.display_name,
                "platform": node.platform,
                "deviceFamily": node.device_family,
                "commands": node.commands,
                "capabilities": node.capabilities,
                "connected": node.connected,
                "paired": node.paired,
                "active": node.active,
                "gatewayLocal": node.gateway_local,
                "generation": node.generation
            }));
        }
    }

    let status = api.device_pairing.handle_pair_status(&serde_json::json!({ "deviceId": node_id }))?;
    if status.get("status").and_then(serde_json::Value::as_str) == Some("approved") {
        let device = status.get("device");
        Ok(serde_json::json!({
            "ts": started_at_ms(),
            "nodeId": node_id,
            "displayName": device.and_then(|d| d.get("displayName")).and_then(serde_json::Value::as_str).unwrap_or(node_id),
            "platform": device.and_then(|d| d.get("platform")).and_then(serde_json::Value::as_str).unwrap_or("unknown"),
            "deviceFamily": device.and_then(|d| d.get("deviceFamily")).and_then(serde_json::Value::as_str).unwrap_or("remote"),
            "commands": [],
            "capabilities": [],
            "connected": false,
            "paired": true,
            "active": false,
            "gatewayLocal": false
        }))
    } else {
        Err(GatewayCommandError::BadRequest(format!("unknown nodeId: {node_id}")))
    }
}

pub(crate) fn call_openclaw_node_invoke(
    api: &UnifiedGatewayApi,
    args: &serde_json::Value,
) -> Result<serde_json::Value, GatewayCommandError> {
    let node_id = args.get("nodeId")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| GatewayCommandError::BadRequest("node.invoke requires nodeId".into()))?;

    let command = args.get("command")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| GatewayCommandError::BadRequest("node.invoke requires command".into()))?;

    let invoke_id = args.get("idempotencyKey")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| GatewayCommandError::BadRequest("node.invoke requires idempotencyKey".into()))?;

    if node_id != "sand-local-node" {
        let status = api.device_pairing.handle_pair_status(&serde_json::json!({ "deviceId": node_id }))?;
        if status.get("status").and_then(serde_json::Value::as_str) != Some("approved") {
            return Err(GatewayCommandError::Internal("node not connected".into()));
        }
        return Err(GatewayCommandError::Internal(
            "remote node transport is not connected to the canonical Host node owner".into(),
        ));
    }

    if command != "system.run" {
        return Err(GatewayCommandError::BadRequest(format!(
            "unsupported local node command: {command}"
        )));
    }
    let params = args.get("params").unwrap_or(&serde_json::Value::Null);
    let received_params_json = serde_json::to_string(params).map_err(|error| {
        GatewayCommandError::BadRequest(format!(
            "node.invoke system.run params could not be encoded as paramsJSON: {error}"
        ))
    })?;
    if received_params_json.len() > MAX_PARAMS_JSON_BYTES {
        return Err(GatewayCommandError::BadRequest(
            "node.invoke system.run paramsJSON exceeds 256 KiB".into(),
        ));
    }
    let shell_command = params
        .get("command")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            GatewayCommandError::BadRequest(
                "node.invoke system.run requires params.command".into(),
            )
        })?;
    let working_directory = params
        .get("workingDirectory")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("/workspace");
    let tool_call_id = args
        .get("idempotencyKey")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("node-invoke");
    let scope = GatewayLocalToolScope {
        agent_id: args
            .get("agentId")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        tool_call_id: Some(tool_call_id.to_string()),
        action: Some("node.invoke.system.run".into()),
    };
    // The public Host gateway uses its legacy `params` object, while the
    // node-host wire protocol uses `paramsJSON`. Adapt that request at
    // this boundary, then let the canonical runtime bind node identity
    // and generation before the local side-effect owner runs it.
    let request = NodeInvokeRequest {
        id: invoke_id.to_string(),
        node_id: node_id.to_string(),
        command: command.to_string(),
        params: params.clone(),
        received_params_bytes: received_params_json.len(),
        received_params_json: Some(received_params_json),
        timeout_ms: args.get("timeoutMs").and_then(serde_json::Value::as_u64),
        idempotency_key: Some(tool_call_id.to_string()),
        session_key: args
            .get("sessionKey")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
    };
    let mut invocation = NodeHostInvocationRuntime::start(
        Arc::clone(&api.node_host_runtime),
        api.node_host_local_session.clone(),
        &request,
    )
        .map_err(GatewayCommandError::BadRequest)?;
    let result = api
        .local_exec
        .sandbox(api.local_tool_permission.clone())
        .exec_manager()
        .execute_shell_with_timeout(
            &scope,
            shell_command,
            working_directory,
            tool_call_id,
            args.get("timeoutMs")
                .and_then(serde_json::Value::as_u64)
                .map(std::time::Duration::from_millis),
        );
    let terminal_result = match &result {
        Ok(result) => NodeInvokeResult::success_payload(
            invoke_id,
            node_id,
            serde_json::json!({
                "exitCode": result.exit_code,
                "stdout": result.stdout,
                "stderr": result.stderr,
                "cwd": result.cwd,
                "aborted": result.aborted,
                "outcomeUnknown": result.outcome_unknown,
                "stdoutTruncated": result.stdout_truncated,
                "stderrTruncated": result.stderr_truncated,
            }),
        ),
        Err(error) => NodeInvokeResult::failure(
            invoke_id,
            node_id,
            "EXECUTION_ERROR",
            error.to_string(),
            None,
        ),
    };
    let public_result = serde_json::to_value(&terminal_result)
        .map_err(|error| GatewayCommandError::Internal(error.to_string()))?;
    invocation
        .complete(terminal_result)
        .map_err(GatewayCommandError::Internal)?;

    let response = match public_result {
        serde_json::Value::Object(mut object) => {
            object.insert("command".into(), serde_json::Value::String(command.to_string()));
            object.insert("payloadJSON".into(), serde_json::Value::Null);
            serde_json::Value::Object(object)
        }
        _ => {
            return Err(GatewayCommandError::Internal(
                "node.invoke result did not serialize as an object".into(),
            ));
        }
    };
    Ok(response)
}
