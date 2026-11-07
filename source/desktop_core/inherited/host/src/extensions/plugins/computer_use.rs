use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

pub const COMPUTER_EXECUTION_ID_PATTERN: &str =
    "^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$";

pub const COMPUTER_USE_V2_ACTION_NAMES: &[&str] = &[
    "screenshot",
    "left_click",
    "right_click",
    "middle_click",
    "double_click",
    "triple_click",
    "mouse_move",
    "left_click_drag",
    "left_mouse_down",
    "left_mouse_up",
    "scroll",
    "type",
    "key",
    "hold_key",
    "wait",
    "list_apps",
    "list_windows",
    "get_accessibility_tree",
    "get_cursor_position",
    "get_window_state",
    "launch_app",
    "kill_app",
    "bring_to_front",
    "set_value",
    "zoom",
    "get_browser_state",
    "browser_prepare",
    "browser_navigate",
    "browser_click",
    "browser_type",
    "browser_dialog",
    "browser_set_input_files",
    "browser_download",
    "browser_pointer",
    "escalate_scope",
    "get_recording_state",
    "start_recording",
    "stop_recording",
    "replay_trajectory",
    "invoke_menu",
];

pub fn computer_use_v1_action_names() -> &'static [&'static str] {
    &COMPUTER_USE_V2_ACTION_NAMES[0..15]
}

pub fn computer_act_v1_action_names() -> &'static [&'static str] {
    &COMPUTER_USE_V2_ACTION_NAMES[1..14]
}

pub const COMPUTER_CONTRACT_MISMATCH: &str = "COMPUTER_CONTRACT_MISMATCH";
pub const COMPUTER_STALE_OBSERVATION: &str = "COMPUTER_STALE_OBSERVATION";
pub const COMPUTER_HOST_BUSY: &str = "COMPUTER_HOST_BUSY: another provider execution owns this computer";

pub const COMPUTER_SCROLL_DIRECTIONS: &[&str] = &["up", "down", "left", "right"];

pub const COMPUTER_ESCALATION_REASONS: &[&str] = &[
    "ax_tree_pixel_mismatch",
    "background_delivery_failed",
    "foreground_ineffective",
    "no_window_target",
    "other",
];

pub fn is_valid_computer_execution_id(id: &str) -> bool {
    if id.len() != 36 {
        return false;
    }
    let parts: Vec<&str> = id.split('-').collect();
    if parts.len() != 5 {
        return false;
    }
    let hex_check = |s: &str| {
        s.bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    };
    parts[0].len() == 8
        && hex_check(parts[0])
        && parts[1].len() == 4
        && hex_check(parts[1])
        && parts[2].len() == 4
        && hex_check(parts[2])
        && matches!(parts[2].as_bytes().first(), Some(b'1'..=b'5'))
        && parts[3].len() == 4
        && hex_check(parts[3])
        && matches!(parts[3].as_bytes().first(), Some(b'8'..=b'b'))
        && parts[4].len() == 12
        && hex_check(parts[4])
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenSnapshotParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_frame_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen_index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenSnapshotResult {
    pub data: String,
    pub width: u32,
    pub height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerActParams {
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub button: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modifiers: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerActResult {
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerUseCapabilityDescriptor {
    pub supported_actions: Vec<String>,
    pub max_displays: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_modes: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct ActiveComputerExecution {
    pub execution_id: String,
    pub session_key: Option<String>,
}

pub struct ComputerExecutionManager {
    current_execution: Mutex<Option<ActiveComputerExecution>>,
    closed_executions: Mutex<HashSet<String>>,
}

impl ComputerExecutionManager {
    pub fn new() -> Self {
        Self {
            current_execution: Mutex::new(None),
            closed_executions: Mutex::new(HashSet::new()),
        }
    }

    pub fn open_execution(&self, execution_id: &str, session_key: Option<String>) -> Result<(), String> {
        if !is_valid_computer_execution_id(execution_id) {
            return Err("COMPUTER_INVALID_REQUEST: executionId is required".to_string());
        }

        let mut lock = self.current_execution.lock().unwrap();
        if let Some(active) = lock.as_ref() {
            if active.execution_id != execution_id {
                return Err(COMPUTER_HOST_BUSY.to_string());
            }
        } else {
            *lock = Some(ActiveComputerExecution {
                execution_id: execution_id.to_string(),
                session_key,
            });
        }
        Ok(())
    }

    pub fn verify_execution(&self, execution_id: Option<&str>) -> Result<(), String> {
        let lock = self.current_execution.lock().unwrap();
        match (lock.as_ref(), execution_id) {
            (Some(active), Some(id)) => {
                if active.execution_id != id {
                    Err(COMPUTER_HOST_BUSY.to_string())
                } else {
                    Ok(())
                }
            }
            (Some(_), None) => Err("COMPUTER_INVALID_REQUEST: executionId is required".to_string()),
            (None, _) => Ok(()),
        }
    }

    pub fn close_execution(&self, execution_id: &str, _reason: &str) -> Result<(), String> {
        let mut lock = self.current_execution.lock().unwrap();
        if let Some(active) = lock.as_ref() {
            if active.execution_id == execution_id {
                self.closed_executions.lock().unwrap().insert(execution_id.to_string());
                *lock = None;
                return Ok(());
            }
        }
        Ok(())
    }

    pub fn is_active(&self) -> bool {
        self.current_execution.lock().unwrap().is_some()
    }
}
