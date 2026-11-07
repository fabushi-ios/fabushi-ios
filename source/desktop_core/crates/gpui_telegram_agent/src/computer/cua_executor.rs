use gpui_telegram_core::error::{Result, TelegramError};
use serde::{Deserialize, Serialize};

pub use mahayana_host_protocol::{
    ComputerAction, ComputerActionKind, ComputerMouseButton, ComputerPoint, ComputerActionResult,
};
pub use mahayana_node_agent_coordinator::gateway::box_vnc_proxy::{
    VncProxyDescriptor, FORK_NOVNC_PORT, PRIMARY_NOVNC_PORT, proxify_box_vnc_url,
};

use super::lease::ComputerControlLease;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action_type", rename_all = "snake_case")]
pub enum CuaAction {
    Click { x: i32, y: i32, button: String },
    MoveCursor { x: i32, y: i32 },
    KeyPress { key: String },
    TypeString { text: String },
    TakeScreenshot,
    Wait { wait_ms: u32 },
}

pub struct CuaExecutor;

impl CuaExecutor {
    pub fn execute_action(action: &CuaAction, lease_valid: bool) -> Result<String> {
        if !lease_valid {
            return Err(TelegramError::AuthFailed(
                "Cannot execute CUA action without active control lease".into(),
            ));
        }
        Ok(format!("Executed {:?}", action))
    }

    pub fn execute_action_fenced(
        action: &CuaAction,
        lease: &ComputerControlLease,
        current_broker_gen: u64,
    ) -> Result<String> {
        if lease.generation != current_broker_gen || !lease.is_valid() {
            return Err(TelegramError::AuthFailed(
                "Stale lease generation or preempted by human takeover".into(),
            ));
        }
        Self::execute_action(action, true)
    }

    /// Converts CuaAction to inherited host protocol ComputerAction
    pub fn action_to_protocol(action: &CuaAction) -> ComputerAction {
        match action {
            CuaAction::Click { x, y, button } => {
                let mouse_button = match button.to_lowercase().as_str() {
                    "right" => Some(ComputerMouseButton::Right),
                    "middle" => Some(ComputerMouseButton::Middle),
                    _ => Some(ComputerMouseButton::Left),
                };
                ComputerAction {
                    action: ComputerActionKind::Click,
                    x: Some(*x as i32),
                    y: Some(*y as i32),
                    x2: None,
                    y2: None,
                    path: None,
                    button: mouse_button,
                    click_count: Some(1),
                    text: None,
                    key: None,
                    wait_ms: None,
                    direction: None,
                    amount: None,
                    description: None,
                }
            }
            CuaAction::MoveCursor { x, y } => ComputerAction {
                action: ComputerActionKind::Move,
                x: Some(*x as i32),
                y: Some(*y as i32),
                x2: None,
                y2: None,
                path: None,
                button: None,
                click_count: None,
                text: None,
                key: None,
                wait_ms: None,
                direction: None,
                amount: None,
                description: None,
            },
            CuaAction::KeyPress { key } => ComputerAction {
                action: ComputerActionKind::Key,
                x: None,
                y: None,
                x2: None,
                y2: None,
                path: None,
                button: None,
                click_count: None,
                text: None,
                key: Some(key.clone()),
                wait_ms: None,
                direction: None,
                amount: None,
                description: None,
            },
            CuaAction::TypeString { text } => ComputerAction {
                action: ComputerActionKind::Type,
                x: None,
                y: None,
                x2: None,
                y2: None,
                path: None,
                button: None,
                click_count: None,
                text: Some(text.clone()),
                key: None,
                wait_ms: None,
                direction: None,
                amount: None,
                description: None,
            },
            CuaAction::TakeScreenshot => ComputerAction {
                action: ComputerActionKind::Screenshot,
                x: None,
                y: None,
                x2: None,
                y2: None,
                path: None,
                button: None,
                click_count: None,
                text: None,
                key: None,
                wait_ms: None,
                direction: None,
                amount: None,
                description: None,
            },
            CuaAction::Wait { wait_ms } => ComputerAction {
                action: ComputerActionKind::Wait,
                x: None,
                y: None,
                x2: None,
                y2: None,
                path: None,
                button: None,
                click_count: None,
                text: None,
                key: None,
                wait_ms: Some(*wait_ms as u64),
                direction: None,
                amount: None,
                description: None,
            },
        }
    }

    /// RFB standard PointerEvent (message type 5, 6 bytes):
    /// [0] = 5
    /// [1] = button-mask (bit 0: left, bit 1: middle, bit 2: right)
    /// [2..4] = x-position (u16 BE)
    /// [4..6] = y-position (u16 BE)
    pub fn to_rfb_pointer_event(x: i32, y: i32, button_mask: u8) -> [u8; 6] {
        let x_u16 = (x.max(0) as u16).to_be_bytes();
        let y_u16 = (y.max(0) as u16).to_be_bytes();
        [5, button_mask, x_u16[0], x_u16[1], y_u16[0], y_u16[1]]
    }

    /// RFB standard KeyEvent (message type 4, 8 bytes):
    /// [0] = 4
    /// [1] = down-flag (1 = down, 0 = up)
    /// [2..4] = padding (0)
    /// [4..8] = key (keysym u32 BE)
    pub fn to_rfb_key_event(down_flag: bool, key_code: u32) -> [u8; 8] {
        let key_bytes = key_code.to_be_bytes();
        [
            4,
            if down_flag { 1 } else { 0 },
            0,
            0,
            key_bytes[0],
            key_bytes[1],
            key_bytes[2],
            key_bytes[3],
        ]
    }

    /// Builds fork URL from VNC proxy descriptor
    pub fn build_vnc_fork_url(descriptor: &VncProxyDescriptor, token: Option<&str>) -> String {
        let base = descriptor.fork_base_url.trim_end_matches('/');
        let token_part = token
            .filter(|t| !t.is_empty())
            .map(|t| format!("token={}&", t))
            .unwrap_or_default();
        format!(
            "{}/vnc.html?{}network_token={}",
            base, token_part, descriptor.network_token
        )
    }
}
