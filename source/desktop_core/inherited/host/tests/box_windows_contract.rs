use std::collections::{BTreeMap, HashMap};
use std::io;

use mahayana_host_runtime::r#box::box_shell_command::HostShellArgs;
use mahayana_host_runtime::r#box::box_windows::{
    BoxConnection, RunWindowScriptError, SAND_BOX_DISPLAY_HEADER, SAND_BOX_FORK_ROUTER_PORT,
    SAND_BOX_WINDOW_OWNER_HEADER, SAND_BOX_WINDOW_UNAVAILABLE_EXIT_CODE, ShellAccessor,
    ShellExecutionOutcome, ShellExecutionResult, clear_agent_window_connections, env_int,
    is_valid_sand_window_owner_token, mint_sand_window_owner_token, primary_sand_box_window,
    run_start_window, run_window_script, sand_box_display_token, sand_box_window_key,
    touch_sand_monitor_busy_lease,
};
use mahayana_host_runtime::ports::r#box::{
    SAND_BOX_PRIMARY_WINDOW_INDEX, SandBoxNoMonitorAvailableError,
};

struct RecordingShell {
    calls: Vec<HostShellArgs>,
    result: ShellExecutionResult,
    fail_transport: bool,
}

impl ShellAccessor<String> for RecordingShell {
    type Error = io::Error;

    fn execute(
        &mut self,
        _ctx: &String,
        args: HostShellArgs,
    ) -> Result<ShellExecutionResult, Self::Error> {
        self.calls.push(args);
        if self.fail_transport {
            Err(io::Error::other("transport failed"))
        } else {
            Ok(self.result.clone())
        }
    }
}

fn successful_shell() -> RecordingShell {
    RecordingShell {
        calls: Vec::new(),
        result: ShellExecutionResult {
            result: ShellExecutionOutcome::Success {
                exit_code: 0,
                stderr: String::new(),
            },
        },
        fail_transport: false,
    }
}

#[test]
fn box_windows_use_the_grok_shell_builder_for_real_window_commands() {
    let mut shell = successful_shell();
    run_start_window(
        &"ctx".to_string(),
        &mut shell,
        2,
        Some("agent_owner-7"),
    )
    .expect("start-window should succeed");

    assert_eq!(shell.calls.len(), 1);
    let args = &shell.calls[0];
    assert_eq!(args.command, "/usr/local/bin/start-window 2 agent_owner-7");
    assert_eq!(args.working_directory, "/workspace");
    assert_eq!(args.tool_call_id, "sand-start-window");
    assert!(args.skip_approval);
    assert_eq!(args.parsing_result.executable_commands[0].name, "start-window");
    assert_eq!(
        args.parsing_result.executable_commands[0].full_text,
        args.command
    );

    assert_eq!(SAND_BOX_FORK_ROUTER_PORT, 1339);
    assert_eq!(SAND_BOX_DISPLAY_HEADER, "x-sand-display");
    assert_eq!(SAND_BOX_WINDOW_OWNER_HEADER, "x-sand-window-owner");
    assert_eq!(sand_box_display_token(4), "4");
}

#[test]
fn box_windows_preserve_guard_owner_token_and_unavailable_monitor_semantics() {
    let mut shell = successful_shell();
    let mut refused = Vec::<String>::new();
    {
        let mut report = |label: &str| refused.push(label.to_string());
        run_window_script(
            &"ctx".to_string(),
            &mut shell,
            "start-window",
            SAND_BOX_PRIMARY_WINDOW_INDEX,
            None,
            Some(&mut report),
        )
        .expect("primary window should be guarded without shell execution");
    }
    assert_eq!(refused, vec!["start-window"]);
    assert!(shell.calls.is_empty());

    let malformed = run_start_window(
        &"ctx".to_string(),
        &mut shell,
        2,
        Some("bad token"),
    )
    .expect_err("malformed owner token must fail closed");
    assert!(matches!(malformed, RunWindowScriptError::Window(_)));

    shell.result = ShellExecutionResult {
        result: ShellExecutionOutcome::Success {
            exit_code: SAND_BOX_WINDOW_UNAVAILABLE_EXIT_CODE,
            stderr: "busy".into(),
        },
    };
    let unavailable = run_start_window(&"ctx".to_string(), &mut shell, 2, None)
        .expect_err("busy fork must surface no-monitor");
    match unavailable {
        RunWindowScriptError::NoMonitor(SandBoxNoMonitorAvailableError(message)) => {
            assert!(message.contains("could not claim display :2"));
        }
        other => panic!("unexpected error: {other:?}"),
    }

    assert!(is_valid_sand_window_owner_token("abc_DEF-123"));
    assert!(!is_valid_sand_window_owner_token(""));
    assert!(!is_valid_sand_window_owner_token("not valid"));
    assert!(is_valid_sand_window_owner_token(&mint_sand_window_owner_token()));
}

#[test]
fn box_windows_preserve_env_integer_lease_key_and_primary_projection_helpers() {
    let mut environment = BTreeMap::new();
    environment.insert("COUNT".into(), " 12workers".into());
    assert_eq!(env_int("COUNT", 100, &environment), 12);
    environment.insert("COUNT".into(), "0".into());
    assert_eq!(env_int("COUNT", 100, &environment), 100);
    environment.insert("COUNT".into(), "-2".into());
    assert_eq!(env_int("COUNT", 100, &environment), 100);

    let mut shell = successful_shell();
    touch_sand_monitor_busy_lease(&"ctx".to_string(), &mut shell, 3);
    assert_eq!(shell.calls.len(), 1);
    assert_eq!(
        shell.calls[0].command,
        "touch /tmp/sand-monitor-busy-3"
    );
    assert_eq!(shell.calls[0].tool_call_id, "sand-monitor-busy-lease");

    shell.fail_transport = true;
    touch_sand_monitor_busy_lease(&"ctx".to_string(), &mut shell, 4);

    assert_eq!(sand_box_window_key("agent-a", 2), "agent-a#2");
    let mut connections = HashMap::from([
        ("agent-a#2".to_string(), 2),
        ("agent-a#3".to_string(), 3),
        ("agent-b#2".to_string(), 4),
    ]);
    clear_agent_window_connections(&mut connections, "agent-a");
    assert_eq!(connections, HashMap::from([("agent-b#2".to_string(), 4)]));

    let primary = primary_sand_box_window(BoxConnection {
        remote_accessor: "computer",
        vnc_url: "http://127.0.0.1/vnc.html".into(),
    });
    assert_eq!(primary.window_index, SAND_BOX_PRIMARY_WINDOW_INDEX);
    assert_eq!(primary.computer_use, "computer");
}
