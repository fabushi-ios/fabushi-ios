use crate::box_shell_command::{
    HostShellArgs, HostShellArgsInput, build_host_shell_args,
};
use std::collections::BTreeMap;

pub const SAND_BOX_PRIMARY_WINDOW_INDEX: u32 = 1;
pub const SAND_BOX_FIRST_FORK_WINDOW_INDEX: u32 = 2;
pub const SAND_BOX_FORK_ROUTER_PORT: u16 = 1339;
pub const SAND_BOX_DISPLAY_HEADER: &str = "x-sand-display";
pub const SAND_BOX_WINDOW_OWNER_HEADER: &str = "x-sand-window-owner";
pub const SAND_BOX_WINDOW_UNAVAILABLE_EXIT_CODE: i32 = 75;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandBoxWindowError {
    MalformedOwnerToken,
    NoMonitorAvailable(String),
    RemoteCommand(String),
}

pub fn is_primary_window_index(window_index: u32) -> bool {
    window_index <= SAND_BOX_PRIMARY_WINDOW_INDEX
}

pub fn valid_sand_window_owner_token(token: &str) -> bool {
    !token.is_empty()
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

pub fn mint_sand_window_owner_token(
    generate: impl FnOnce() -> String,
) -> Result<String, SandBoxWindowError> {
    let token = generate();
    if valid_sand_window_owner_token(&token) {
        Ok(token)
    } else {
        Err(SandBoxWindowError::MalformedOwnerToken)
    }
}

pub fn env_int(
    name: &str,
    fallback: u32,
    env: &BTreeMap<String, String>,
) -> u32 {
    env.get(name)
        .and_then(|raw| raw.trim().parse::<u32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

pub fn sand_box_display_token(window_index: u32) -> String {
    window_index.to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowShellResult {
    pub exit_code: i32,
    pub stderr: String,
}

pub trait WindowScriptRunner<Context> {
    type Error: std::fmt::Display;

    async fn execute_remote_shell(
        &self,
        context: &Context,
        args: HostShellArgs,
    ) -> Result<WindowShellResult, Self::Error>;
}

pub async fn run_window_script<Context, Runner>(
    context: &Context,
    runner: &Runner,
    label: &str,
    window_index: u32,
    owner_token: Option<&str>,
    report_guard_refused: impl FnOnce(&str),
) -> Result<(), SandBoxWindowError>
where
    Runner: WindowScriptRunner<Context>,
{
    if is_primary_window_index(window_index) {
        report_guard_refused(label);
        return Ok(());
    }
    if owner_token.is_some_and(|token| !valid_sand_window_owner_token(token)) {
        return Err(SandBoxWindowError::MalformedOwnerToken);
    }

    let command = match owner_token {
        Some(token) => format!("/usr/local/bin/{label} {window_index} {token}"),
        None => format!("/usr/local/bin/{label} {window_index}"),
    };
    let result = runner
        .execute_remote_shell(
            context,
            build_host_shell_args(HostShellArgsInput {
                command,
                name: label.to_owned(),
                working_directory: "/workspace".into(),
                tool_call_id: format!("sand-{label}"),
            }),
        )
        .await
        .map_err(|error| SandBoxWindowError::RemoteCommand(error.to_string()))?;

    if result.exit_code == SAND_BOX_WINDOW_UNAVAILABLE_EXIT_CODE {
        return Err(SandBoxWindowError::NoMonitorAvailable(format!(
            "{label} could not claim display :{window_index}: it is a live fork owned by a different agent"
        )));
    }
    if result.exit_code != 0 {
        return Err(SandBoxWindowError::RemoteCommand(format!(
            "{label} exited {}: {}",
            result.exit_code, result.stderr
        )));
    }
    Ok(())
}

pub async fn run_start_window<Context, Runner>(
    context: &Context,
    runner: &Runner,
    window_index: u32,
    owner_token: Option<&str>,
) -> Result<(), SandBoxWindowError>
where
    Runner: WindowScriptRunner<Context>,
{
    run_window_script(
        context,
        runner,
        "start-window",
        window_index,
        owner_token,
        |_| {},
    )
    .await
}

pub async fn run_stop_window<Context, Runner>(
    context: &Context,
    runner: &Runner,
    window_index: u32,
) -> Result<(), SandBoxWindowError>
where
    Runner: WindowScriptRunner<Context>,
{
    run_window_script(
        context,
        runner,
        "stop-window",
        window_index,
        None,
        |_| {},
    )
    .await
}

pub async fn touch_sand_monitor_busy_lease<Context, Runner>(
    context: &Context,
    runner: &Runner,
    window_index: u32,
) where
    Runner: WindowScriptRunner<Context>,
{
    if window_index < 1 {
        return;
    }
    let _ = runner
        .execute_remote_shell(
            context,
            build_host_shell_args(HostShellArgsInput {
                command: format!("touch /tmp/sand-monitor-busy-{window_index}"),
                name: "touch".into(),
                working_directory: "/workspace".into(),
                tool_call_id: "sand-monitor-busy-lease".into(),
            }),
        )
        .await;
}

pub fn sand_box_window_key(agent_id: &str, window_index: u32) -> String {
    format!("{agent_id}#{window_index}")
}

pub fn clear_agent_window_connections<T>(
    connections: &mut BTreeMap<String, T>,
    agent_id: &str,
) {
    let prefix = format!("{agent_id}#");
    connections.retain(|key, _| !key.starts_with(&prefix));
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrimarySandBoxWindow<T> {
    pub window_index: u32,
    pub computer_use: T,
    pub vnc_url: String,
}

pub fn primary_sand_box_window<T>(
    computer_use: T,
    vnc_url: impl Into<String>,
) -> PrimarySandBoxWindow<T> {
    PrimarySandBoxWindow {
        window_index: SAND_BOX_PRIMARY_WINDOW_INDEX,
        computer_use,
        vnc_url: vnc_url.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;
    use std::sync::Mutex;

    struct Runner {
        requests: Mutex<Vec<HostShellArgs>>,
        result: WindowShellResult,
    }

    impl WindowScriptRunner<()> for Runner {
        type Error = Infallible;
        async fn execute_remote_shell(
            &self,
            _context: &(),
            args: HostShellArgs,
        ) -> Result<WindowShellResult, Self::Error> {
            self.requests.lock().unwrap().push(args);
            Ok(self.result.clone())
        }
    }

    #[tokio::test]
    async fn primary_window_is_guarded_without_remote_execution() {
        let runner = Runner {
            requests: Mutex::new(Vec::new()),
            result: WindowShellResult { exit_code: 0, stderr: String::new() },
        };
        let refused = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let marker = refused.clone();
        run_window_script(&(), &runner, "stop-window", 1, None, move |_| {
            marker.store(true, std::sync::atomic::Ordering::SeqCst);
        })
        .await
        .unwrap();
        assert!(refused.load(std::sync::atomic::Ordering::SeqCst));
        assert!(runner.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn fork_window_routes_typed_shell_request_to_runner() {
        let runner = Runner {
            requests: Mutex::new(Vec::new()),
            result: WindowShellResult { exit_code: 0, stderr: String::new() },
        };
        run_start_window(&(), &runner, 2, Some("owner_2"))
            .await
            .unwrap();
        let requests = runner.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].command,
            "/usr/local/bin/start-window 2 owner_2"
        );
        assert!(requests[0].skip_approval);
    }

    #[test]
    fn token_and_connection_helpers_match_reference_contract() {
        assert!(valid_sand_window_owner_token("abc_DEF-123"));
        assert!(!valid_sand_window_owner_token("bad token"));
        let mut map = BTreeMap::from([
            ("a#1".to_owned(), 1),
            ("a#2".to_owned(), 2),
            ("b#1".to_owned(), 3),
        ]);
        clear_agent_window_connections(&mut map, "a");
        assert_eq!(map.keys().cloned().collect::<Vec<_>>(), vec!["b#1"]);
    }
}
