//! OpenClaw Node Host Command Dispatch (Computer, PTY, CLI, Browser, and Canvas Handlers)
//!
//! Mirrors:
//! - `src/node-host/computer-command.ts`
//! - `src/node-host/pty-command.ts`
//! - `src/node-host/desktop-stream-command.ts`
//! - `src/node-host/portal-stream-command.ts`
//! - `src/node-host/invoke-agent-cli-claude-handler.ts`
//! - `src/node-host/claude-skill-session.ts`
//! - `src/node-host/exec-policy.ts`
//! - `src/node-host/browser-proxy.ts`
//! - `src/node-host/canvas-present.ts`
//! - `src/node-host/device-status.ts`
//! - `src/node-host/device-apps.ts`
//! - `src/node-host/location-get.ts`

use serde::{Deserialize, Serialize};
use serde_json::json;

#[path = "node_system_run.rs"]
pub mod node_system_run;
#[path = "node_browser_canvas.rs"]
pub mod node_browser_canvas;
#[path = "node_device_status.rs"]
pub mod node_device_status;

pub use self::node_system_run::*;
pub use self::node_browser_canvas::*;
pub use self::node_device_status::*;

pub const COMPUTER_CONTRACT_MISMATCH: &str =
    "COMPUTER_CONTRACT_MISMATCH: generation mismatch between provider and worker";
pub const COMPUTER_DRIVER_UNAVAILABLE: &str =
    "COMPUTER_DRIVER_UNAVAILABLE: worker computer provider is unavailable";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ComputerOperation {
    Close,
    Capabilities,
    Act,
    Snapshot,
}

impl ComputerOperation {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "close" => Some(ComputerOperation::Close),
            "capabilities" => Some(ComputerOperation::Capabilities),
            "act" => Some(ComputerOperation::Act),
            "snapshot" => Some(ComputerOperation::Snapshot),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerCommandInput {
    pub operation: ComputerOperation,
    /// Opaque upstream provider generation; do not coerce it to a number.
    pub provider_generation: Option<String>,
    pub action_params_json: Option<String>,
}

/// The node-host dispatcher owns protocol validation and generation fencing.
/// Platform authorization and side effects stay with the injected native owner.
pub trait NodeWorkerComputerExecutor {
    fn capabilities(&self, generation: &str) -> Result<String, String>;
    fn act(&self, generation: &str, params: &serde_json::Value) -> Result<String, String>;
    fn snapshot(&self, generation: &str, params: &serde_json::Value) -> Result<String, String>;
}

pub fn dispatch_node_worker_computer_command_with_executor<E: NodeWorkerComputerExecutor + ?Sized>(
    input: &ComputerCommandInput,
    current_generation: Option<&str>,
    executor: &E,
) -> Result<String, String> {
    if input.operation == ComputerOperation::Close {
        return Ok(json!({"status": "closed"}).to_string());
    }

    let generation = current_generation
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| COMPUTER_DRIVER_UNAVAILABLE.to_string())?;
    if input.operation != ComputerOperation::Capabilities {
        let req_gen = input
            .provider_generation
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| COMPUTER_CONTRACT_MISMATCH.to_string())?;
        if req_gen != generation {
            return Err(COMPUTER_CONTRACT_MISMATCH.to_string());
        }
    }

    let params = match input.action_params_json.as_deref() {
        Some(raw) => serde_json::from_str(raw)
            .map_err(|_| "COMPUTER_INVALID_ACTION: action parameters are invalid JSON".to_string())?,
        None => serde_json::Value::Null,
    };
    match input.operation {
        ComputerOperation::Capabilities => executor.capabilities(generation),
        ComputerOperation::Act => executor.act(generation, &params),
        ComputerOperation::Snapshot => executor.snapshot(generation, &params),
        ComputerOperation::Close => unreachable!(),
    }
}

pub fn dispatch_node_worker_computer_command(
    input: &ComputerCommandInput,
    current_generation: Option<&str>,
) -> Result<String, &'static str> {
    if input.operation == ComputerOperation::Close {
        return Ok(json!({"status": "closed"}).to_string());
    }
    let _ = current_generation;
    Err(COMPUTER_DRIVER_UNAVAILABLE)
}

/// Execution context for dispatching unified Node Host commands
#[derive(Default)]
pub struct NodeHostCommandContext<'a> {
    pub current_generation: Option<String>,
    pub computer_executor: Option<&'a dyn NodeWorkerComputerExecutor>,
    pub approval_plan: Option<ExecApprovalPlan>,
    pub now_ms: u64,
}

/// Unified Node Host Command Dispatcher routing all cataloged operations:
/// - `system.run`, `system.run.prepare`, `system.which`, `system.notify`
/// - `computer.*` (`capabilities`, `act`, `snapshot`, `close`)
/// - `browser.proxy`
/// - `canvas.present`
/// - `location.get`
/// - `device.apps`, `device.status`
pub fn dispatch_node_host_command(
    command: &str,
    params_json: Option<&str>,
    ctx: &NodeHostCommandContext,
) -> Result<serde_json::Value, String> {
    let raw_params = params_json.unwrap_or("{}");
    match command {
        "system.run" => {
            let input: SystemRunCommandInput = serde_json::from_str(raw_params)
                .map_err(|e| format!("SYSTEM_RUN_INVALID_PARAMS: {e}"))?;
            let res = dispatch_system_run(&input, ctx.approval_plan.as_ref(), ctx.now_ms)?;
            serde_json::to_value(res).map_err(|e| e.to_string())
        }
        "system.run.prepare" => {
            let input: SystemRunPrepareInput = serde_json::from_str(raw_params)
                .map_err(|e| format!("SYSTEM_RUN_PREPARE_INVALID_PARAMS: {e}"))?;
            let res = dispatch_system_run_prepare(&input)?;
            serde_json::to_value(res).map_err(|e| e.to_string())
        }
        "system.which" => {
            let input: SystemWhichInput = serde_json::from_str(raw_params)
                .map_err(|e| format!("SYSTEM_WHICH_INVALID_PARAMS: {e}"))?;
            let res = dispatch_system_which(&input)?;
            serde_json::to_value(res).map_err(|e| e.to_string())
        }
        "system.notify" => {
            let input: SystemNotifyInput = serde_json::from_str(raw_params)
                .map_err(|e| format!("SYSTEM_NOTIFY_INVALID_PARAMS: {e}"))?;
            dispatch_system_notify(&input)
        }
        cmd if cmd.starts_with("computer.") => {
            let op_str = cmd.strip_prefix("computer.").unwrap_or("");
            let op = ComputerOperation::parse(op_str)
                .ok_or_else(|| format!("COMPUTER_UNKNOWN_OP: unknown computer operation '{op_str}'"))?;
            let params_val: serde_json::Value = serde_json::from_str(raw_params)
                .unwrap_or(serde_json::Value::Null);
            let provider_gen = params_val
                .get("providerGeneration")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let action_params_json = params_val
                .get("actionParams")
                .map(|v| v.to_string());

            let comp_input = ComputerCommandInput {
                operation: op,
                provider_generation: provider_gen,
                action_params_json,
            };

            let out_str = if let Some(exec) = ctx.computer_executor {
                dispatch_node_worker_computer_command_with_executor(
                    &comp_input,
                    ctx.current_generation.as_deref(),
                    exec,
                )?
            } else {
                dispatch_node_worker_computer_command(&comp_input, ctx.current_generation.as_deref())
                    .map_err(|e| e.to_string())?
            };
            serde_json::from_str(&out_str).map_err(|e| e.to_string())
        }
        "browser.proxy" => {
            let input: BrowserProxyInput = serde_json::from_str(raw_params)
                .map_err(|e| format!("BROWSER_PROXY_INVALID_PARAMS: {e}"))?;
            let res = dispatch_browser_proxy(&input)?;
            serde_json::to_value(res).map_err(|e| e.to_string())
        }
        "canvas.present" => {
            let input: CanvasPresentInput = serde_json::from_str(raw_params)
                .map_err(|e| format!("CANVAS_PRESENT_INVALID_PARAMS: {e}"))?;
            let res = dispatch_canvas_present(&input)?;
            serde_json::to_value(res).map_err(|e| e.to_string())
        }
        "location.get" => {
            let input: LocationGetInput = serde_json::from_str(raw_params)
                .unwrap_or(LocationGetInput {
                    desired_accuracy: None,
                    timeout_ms: None,
                    require_fresh: None,
                });
            let res = dispatch_location_get(&input, ctx.now_ms)?;
            serde_json::to_value(res).map_err(|e| e.to_string())
        }
        "device.apps" => {
            let input: DeviceAppsInput = serde_json::from_str(raw_params)
                .unwrap_or(DeviceAppsInput {
                    include_system_apps: None,
                    query: None,
                });
            let res = dispatch_device_apps(&input)?;
            serde_json::to_value(res).map_err(|e| e.to_string())
        }
        "device.status" => {
            let res = dispatch_device_status()?;
            serde_json::to_value(res).map_err(|e| e.to_string())
        }
        _ => Err(format!("NODE_COMMAND_UNKNOWN: unknown command '{command}'")),
    }
}
