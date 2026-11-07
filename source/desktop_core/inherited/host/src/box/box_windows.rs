use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::OnceLock;

use thiserror::Error;
use uuid::Uuid;

use crate::ports::r#box::{
    SAND_BOX_PRIMARY_WINDOW_INDEX, SandBoxNoMonitorAvailableError, is_primary_window_index,
};

use super::box_shell_command::{HostShellArgs, HostShellArgsInput, build_host_shell_args};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct SandBoxWindowError(pub String);

pub const SAND_BOX_FORK_ROUTER_PORT: u16 = 1339;
pub const SAND_BOX_DISPLAY_HEADER: &str = "x-sand-display";
pub const SAND_BOX_WINDOW_OWNER_HEADER: &str = "x-sand-window-owner";
pub const SAND_BOX_WINDOW_UNAVAILABLE_EXIT_CODE: i32 = 75;
pub const DEFAULT_SAND_BOX_MAX_WINDOWS: u32 = 100;

pub fn is_valid_sand_window_owner_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

pub fn mint_sand_window_owner_token() -> String {
    Uuid::new_v4().to_string()
}

fn parse_js_decimal_integer(value: &str) -> Option<i64> {
    let trimmed = value.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    let bytes = trimmed.as_bytes();
    let mut index = 0usize;
    let mut sign = 1i64;
    if bytes.first() == Some(&b'+') {
        index = 1;
    } else if bytes.first() == Some(&b'-') {
        index = 1;
        sign = -1;
    }
    let start = index;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
    }
    if index == start {
        return None;
    }
    let parsed = trimmed[start..index].parse::<i64>().ok()?;
    parsed.checked_mul(sign)
}

pub fn env_int(name: &str, fallback: u32, environment: &BTreeMap<String, String>) -> u32 {
    let Some(raw) = environment.get(name) else {
        return fallback;
    };
    let Some(value) = parse_js_decimal_integer(raw) else {
        return fallback;
    };
    u32::try_from(value).ok().filter(|value| *value > 0).unwrap_or(fallback)
}

pub fn sand_box_max_windows() -> u32 {
    static VALUE: OnceLock<u32> = OnceLock::new();
    *VALUE.get_or_init(|| {
        let environment = std::env::vars().collect::<BTreeMap<_, _>>();
        env_int("SAND_BOX_MAX_WINDOWS", DEFAULT_SAND_BOX_MAX_WINDOWS, &environment)
    })
}

pub fn sand_box_display_token(window_index: u32) -> String {
    window_index.to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellExecutionOutcome {
    Success { exit_code: i32, stderr: String },
    Failure { case: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellExecutionResult {
    pub result: ShellExecutionOutcome,
}

pub trait ShellAccessor<Ctx> {
    type Error;

    fn execute(
        &mut self,
        ctx: &Ctx,
        args: HostShellArgs,
    ) -> Result<ShellExecutionResult, Self::Error>;
}

#[derive(Debug)]
pub enum RunWindowScriptError<E> {
    Transport(E),
    Window(SandBoxWindowError),
    NoMonitor(SandBoxNoMonitorAvailableError),
}

impl<E: fmt::Display> fmt::Display for RunWindowScriptError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => error.fmt(formatter),
            Self::Window(error) => error.fmt(formatter),
            Self::NoMonitor(error) => error.fmt(formatter),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for RunWindowScriptError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            Self::Window(error) => Some(error),
            Self::NoMonitor(error) => Some(error),
        }
    }
}

pub fn run_window_script<Ctx, Accessor>(
    ctx: &Ctx,
    accessor: &mut Accessor,
    label: &str,
    window_index: u32,
    owner_token: Option<&str>,
    mut report_guard_refused: Option<&mut dyn FnMut(&str)>,
) -> Result<(), RunWindowScriptError<Accessor::Error>>
where
    Accessor: ShellAccessor<Ctx>,
{
    if is_primary_window_index(window_index) {
        if let Some(report) = report_guard_refused.as_mut() {
            report(label);
        }
        return Ok(());
    }
    if owner_token.is_some_and(|value| !is_valid_sand_window_owner_token(value)) {
        return Err(RunWindowScriptError::Window(SandBoxWindowError(format!(
            "refusing to run {label} with a malformed owner token"
        ))));
    }

    let command = match owner_token {
        Some(owner_token) => format!("/usr/local/bin/{label} {window_index} {owner_token}"),
        None => format!("/usr/local/bin/{label} {window_index}"),
    };
    let result = accessor
        .execute(
            ctx,
            build_host_shell_args(HostShellArgsInput {
                command,
                name: label.to_string(),
                working_directory: "/workspace".into(),
                tool_call_id: format!("sand-{label}"),
            }),
        )
        .map_err(RunWindowScriptError::Transport)?;

    match result.result {
        ShellExecutionOutcome::Failure { case } => Err(RunWindowScriptError::Window(
            SandBoxWindowError(format!("{label} failed ({case})")),
        )),
        ShellExecutionOutcome::Success { exit_code, stderr }
            if exit_code == SAND_BOX_WINDOW_UNAVAILABLE_EXIT_CODE =>
        {
            Err(RunWindowScriptError::NoMonitor(
                SandBoxNoMonitorAvailableError(format!(
                    "{label} could not claim display :{window_index}: it is a live fork owned by a different agent"
                )),
            ))
        }
        ShellExecutionOutcome::Success { exit_code, stderr } if exit_code != 0 => {
            Err(RunWindowScriptError::Window(SandBoxWindowError(format!(
                "{label} exited {exit_code}: {stderr}"
            ))))
        }
        ShellExecutionOutcome::Success { .. } => Ok(()),
    }
}

pub fn run_start_window<Ctx, Accessor>(
    ctx: &Ctx,
    accessor: &mut Accessor,
    window_index: u32,
    owner_token: Option<&str>,
) -> Result<(), RunWindowScriptError<Accessor::Error>>
where
    Accessor: ShellAccessor<Ctx>,
{
    run_window_script(ctx, accessor, "start-window", window_index, owner_token, None)
}

pub fn run_stop_window<Ctx, Accessor>(
    ctx: &Ctx,
    accessor: &mut Accessor,
    window_index: u32,
) -> Result<(), RunWindowScriptError<Accessor::Error>>
where
    Accessor: ShellAccessor<Ctx>,
{
    run_window_script(ctx, accessor, "stop-window", window_index, None, None)
}

pub fn touch_sand_monitor_busy_lease<Ctx, Accessor>(
    ctx: &Ctx,
    accessor: &mut Accessor,
    window_index: u32,
) where
    Accessor: ShellAccessor<Ctx>,
{
    if window_index < SAND_BOX_PRIMARY_WINDOW_INDEX {
        return;
    }
    let _ = accessor.execute(
        ctx,
        build_host_shell_args(HostShellArgsInput {
            command: format!("touch /tmp/sand-monitor-busy-{window_index}"),
            name: "touch".into(),
            working_directory: "/workspace".into(),
            tool_call_id: "sand-monitor-busy-lease".into(),
        }),
    );
}

pub fn sand_box_window_key(agent_id: &str, window_index: u32) -> String {
    format!("{agent_id}#{window_index}")
}

pub fn clear_agent_window_connections<T>(
    connections: &mut HashMap<String, T>,
    agent_id: &str,
) {
    let prefix = format!("{agent_id}#");
    connections.retain(|key, _| !key.starts_with(&prefix));
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxConnection<Accessor> {
    pub remote_accessor: Accessor,
    pub vnc_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandBoxWindow<Accessor> {
    pub window_index: u32,
    pub computer_use: Accessor,
    pub vnc_url: String,
}

pub fn primary_sand_box_window<Accessor>(
    connection: BoxConnection<Accessor>,
) -> SandBoxWindow<Accessor> {
    SandBoxWindow {
        window_index: SAND_BOX_PRIMARY_WINDOW_INDEX,
        computer_use: connection.remote_accessor,
        vnc_url: connection.vnc_url,
    }
}
