use std::cell::RefCell;
use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::inference::provider_session::{ProviderSessionError, RoutedToolDefinition};
use mahayana_host_runtime::runner::routed_provider_runtime::RoutedToolBridge;
use mahayana_host_runtime::runner::box_tool_access::{
    RunnerBoxReadRequest, RunnerBoxResourcePort, RunnerBoxShellRequest, RunnerBoxWriteRequest,
};
use serde_json::Value;

use mahayana_host_runtime::r#box::box_shell_command::HostShellArgsInput;
use mahayana_host_runtime::runner::host_computer_tool_dependencies::{
    ComputerProjectionError, GeneratedComputerAction, GeneratedComputerUseResult,
    ProductionComputerToolExecutor, decode_generated_computer_use_result,
    encode_generated_computer_use_args, execute_host_shell,
    from_generated_computer_use_result, resolve_browser_window_index,
    to_generated_computer_use_args,
};
use mahayana_host_runtime::runner::sand_auto_review::SandAutoReviewMode;
use mahayana_host_runtime::runner::tools::sand_computer_tool::{
    ComputerActionArgs, ComputerActionName, ComputerCoordinate, ComputerToolExecutor,
    ComputerToolExposure, SandComputerToolBridge,
    ComputerUseResult, MouseButtonInput, ReportedComputerAction, ScrollDirectionInput,
    build_computer_action_sequence, computer_tool_definitions, describe_outcome, drag_path,
    parse_remote_computer_action, persist_computer_screenshot, reported_batch_position, to_action,
    to_exact_action_value, validate_computer_action,
};

#[test]
fn computer_schema_core_validates_drag_wait_followups_and_enforce_description() {
    let mut drag = ComputerActionArgs::simple(ComputerActionName::Drag);
    assert!(validate_computer_action(&drag, None).is_err());
    drag.x = Some(1);
    drag.y = Some(2);
    drag.x2 = Some(3);
    drag.y2 = Some(4);
    assert_eq!(
        drag_path(&drag),
        Some(vec![
            ComputerCoordinate { x: 1, y: 2 },
            ComputerCoordinate { x: 3, y: 4 }
        ])
    );
    assert!(validate_computer_action(&drag, Some(SandAutoReviewMode::Enforce)).is_err());
    drag.description = Some("Drag the selected card".into());
    assert!(validate_computer_action(&drag, Some(SandAutoReviewMode::Enforce)).is_ok());

    let mut wait = ComputerActionArgs::simple(ComputerActionName::Wait);
    wait.duration_ms = Some(30_001);
    assert!(validate_computer_action(&wait, None).is_err());

    let mut primary = ComputerActionArgs::simple(ComputerActionName::Move);
    primary.then_actions.push(ComputerActionArgs::simple(ComputerActionName::Click));
    assert!(validate_computer_action(&primary, Some(SandAutoReviewMode::Enforce)).is_err());
}

#[test]
fn actions_and_batch_reporting_match_frozen_defaults() {
    let mut click = ComputerActionArgs::simple(ComputerActionName::Click);
    click.x = Some(10);
    click.y = Some(20);
    let projected = to_action(&click).expect("click");
    assert_eq!(projected.action_case, "click");
    assert_eq!(projected.value["button"], "LEFT");
    assert_eq!(projected.value["count"], 1);

    let mut scroll = ComputerActionArgs::simple(ComputerActionName::Scroll);
    scroll.x = Some(5);
    scroll.y = Some(6);
    scroll.direction = Some(ScrollDirectionInput::Up);
    scroll.amount = Some(4);
    let position = reported_batch_position(&[
        {
            let mut drag = ComputerActionArgs::simple(ComputerActionName::Drag);
            drag.path = vec![
                ComputerCoordinate { x: 1, y: 2 },
                ComputerCoordinate { x: 3, y: 4 },
            ];
            drag
        },
        scroll.clone(),
    ])
    .expect("reported");
    assert_eq!(position, ReportedComputerAction::Scroll { x: 5, y: 6 });

    click.button = Some(MouseButtonInput::Right);
    click.count = Some(2);
    let action = to_action(&click).expect("right click");
    assert_eq!(action.value["button"], "RIGHT");
    assert_eq!(action.value["count"], 2);
}

#[test]
fn double_click_is_explicit_but_reuses_the_frozen_click_protocol() {
    let parsed = parse_remote_computer_action(&serde_json::json!({
        "action": "doubleClick",
        "x": 10,
        "y": 20,
        "description": "Open the selected item"
    }))
    .expect("parse doubleClick");
    assert_eq!(parsed.action, ComputerActionName::DoubleClick);
    assert!(validate_computer_action(&parsed, Some(SandAutoReviewMode::Enforce)).is_ok());

    let projected = to_action(&parsed).expect("project doubleClick");
    assert_eq!(projected.action_case, "click");
    assert_eq!(projected.value["count"], 2);
    assert_eq!(
        reported_batch_position(&[parsed.clone()]),
        Some(ReportedComputerAction::Click {
            x: 10,
            y: 20,
            button: "left".into(),
            count: 2,
        })
    );

    let mut invalid = parsed.clone();
    invalid.count = Some(3);
    assert!(validate_computer_action(&invalid, None).is_err());

    let definitions = computer_tool_definitions();
    let computer = definitions.iter().find(|tool| tool.name == "Computer").expect("Computer");
    assert!(computer.input_schema["properties"]["action"]["enum"]
        .as_array()
        .expect("action enum")
        .iter()
        .any(|value| value.as_str() == Some("doubleClick")));
}

#[test]
fn computer_sequence_adds_exactly_one_final_screenshot() {
    let mut primary = ComputerActionArgs::simple(ComputerActionName::Type);
    primary.text = Some("hello".into());
    primary.then_actions.push(ComputerActionArgs::simple(ComputerActionName::Wait));
    let sequence = build_computer_action_sequence(&primary, None).expect("sequence");
    assert_eq!(sequence.len(), 3);
    assert_eq!(sequence[0].action_case, "type");
    assert_eq!(sequence[1].action_case, "wait");
    assert_eq!(sequence[2].action_case, "screenshot");

    let screenshot = build_computer_action_sequence(
        &ComputerActionArgs::simple(ComputerActionName::Screenshot),
        None,
    )
    .expect("screenshot");
    assert_eq!(screenshot.len(), 1);
}

#[test]
fn outcome_render_and_screenshot_persistence_match_frozen_contract() {
    let mut result = ComputerUseResult::Success(
        mahayana_host_runtime::runner::tools::sand_computer_tool::ComputerUseSuccess {
            screenshot: Some("AQID".into()),
            screenshot_path: None,
            cursor_position: Some(ComputerCoordinate { x: 9, y: 8 }),
        },
    );
    persist_computer_screenshot(
        &mut result,
        Some(|bytes: &[u8], mime: &str| {
            assert_eq!(bytes, &[1, 2, 3]);
            assert_eq!(mime, "image/webp");
            Some("file:///saved.webp".to_string())
        }),
    )
    .expect("persist");
    let rendered = describe_outcome(&result, false);
    assert!(rendered.contains("Screenshot saved to file:///saved.webp."));
    assert!(rendered.contains("Cursor is at (9, 8)."));

    assert_eq!(
        describe_outcome(&ComputerUseResult::Error("boom".into()), true),
        "Screenshot failed: boom"
    );
}

#[test]
fn host_projection_validates_protocol_and_normalizes_generated_results() {
    let mut click = ComputerActionArgs::simple(ComputerActionName::Click);
    click.x = Some(11);
    click.y = Some(12);
    click.button = Some(MouseButtonInput::Middle);
    let protocol = to_action(&click).expect("protocol");
    let generated = to_generated_computer_use_args(
        "tool-1",
        &[protocol],
        Some(true),
        Some("target"),
    )
    .expect("generated");
    assert_eq!(generated.tool_call_id, "tool-1");
    assert_eq!(generated.bind_unmapped_characters, Some(true));
    assert!(matches!(
        &generated.actions[0],
        GeneratedComputerAction::Click { button, count, .. }
            if button == "MIDDLE" && *count == 1.0
    ));

    let invalid = mahayana_host_runtime::runner::tools::sand_computer_tool::ComputerProtocolAction {
        action_case: "click".into(),
        value: serde_json::json!({"button":"BOGUS","count":1}),
    };
    assert!(matches!(
        to_generated_computer_use_args("", &[invalid], None, None),
        Err(ComputerProjectionError(_))
    ));

    let normalized = from_generated_computer_use_result(
        GeneratedComputerUseResult::Success {
            screenshot: Some("abc".into()),
            cursor_position: Some(ComputerCoordinate { x: 1, y: 2 }),
        },
    );
    assert!(matches!(normalized, ComputerUseResult::Success(_)));
}

#[test]
fn host_shell_projection_runs_barrier_then_audit_then_generated_executor() {
    let order = RefCell::new(Vec::<String>::new());
    let result = execute_host_shell(
        HostShellArgsInput {
            command: "pwd".into(),
            name: "pwd".into(),
            working_directory: "/workspace".into(),
            tool_call_id: "tool".into(),
        },
        || order.borrow_mut().push("barrier".into()),
        |command| order.borrow_mut().push(format!("audit:{command}")),
        |args| {
            order.borrow_mut().push(format!("execute:{}", args.command));
            assert!(args.skip_approval);
            42
        },
    );
    assert_eq!(result, 42);
    assert_eq!(
        order.into_inner(),
        vec!["barrier", "audit:pwd", "execute:pwd"]
    );
}

#[test]
fn browser_identity_projection_waits_for_box_before_window_lookup() {
    let order = RefCell::new(Vec::<String>::new());
    let index = resolve_browser_window_index(
        &(),
        "box-7",
        |_, box_id| -> Result<(), &'static str> {
            order.borrow_mut().push(format!("ready:{box_id}"));
            Ok(())
        },
        |box_id| {
            order.borrow_mut().push(format!("window:{box_id}"));
            Some(3)
        },
    )
    .expect("window");
    assert_eq!(index, Some(3));
    assert_eq!(
        order.into_inner(),
        vec!["ready:box-7", "window:box-7"]
    );
}


#[derive(Default)]
struct ComputerTransportPort {
    requests: Mutex<Vec<Vec<u8>>>,
}

impl RunnerBoxResourcePort for ComputerTransportPort {
    fn execute_shell(
        &self,
        _request: RunnerBoxShellRequest,
    ) -> Result<Value, ProviderSessionError> {
        Err(ProviderSessionError::Tool("unused shell".into()))
    }

    fn execute_read(
        &self,
        _request: RunnerBoxReadRequest,
    ) -> Result<Value, ProviderSessionError> {
        Err(ProviderSessionError::Tool("unused read".into()))
    }

    fn execute_write(
        &self,
        _request: RunnerBoxWriteRequest,
    ) -> Result<(), ProviderSessionError> {
        Err(ProviderSessionError::Tool("unused write".into()))
    }

    fn execute_computer_use_protobuf(
        &self,
        protobuf_args: Vec<u8>,
    ) -> Result<Vec<u8>, ProviderSessionError> {
        self.requests.lock().expect("requests").push(protobuf_args);
        // ComputerUseResult { success: ComputerUseSuccess { screenshot: "AQID" } }
        Ok(vec![0x0a, 0x06, 0x1a, 0x04, b'A', b'Q', b'I', b'D'])
    }
}

#[test]
fn generated_computer_protobuf_matches_frozen_wire_contract() {
    let mut click = ComputerActionArgs::simple(ComputerActionName::Click);
    click.x = Some(7);
    click.y = Some(9);
    let action = to_action(&click).expect("click");
    let generated = to_generated_computer_use_args(
        "tool-wire",
        &[action],
        Some(true),
        Some("click target"),
    )
    .expect("generated");
    let encoded = encode_generated_computer_use_args(&generated).expect("encode");
    assert!(!encoded.is_empty());
    assert_eq!(encoded[0], 0x0a, "tool_call_id must remain field 1");

    let decoded = decode_generated_computer_use_result(&[
        0x0a, 0x0c,
        0x1a, 0x03, b'a', b'b', b'c',
        0x32, 0x05, 0x08, 0x01, 0x10, 0x02, 0x00,
    ]);
    assert!(decoded.is_err(), "trailing malformed coordinate bytes must fail closed");

    let decoded = decode_generated_computer_use_result(&[
        0x0a, 0x0b,
        0x1a, 0x03, b'a', b'b', b'c',
        0x32, 0x04, 0x08, 0x01, 0x10, 0x02,
    ])
    .expect("decode success");
    assert!(matches!(
        decoded,
        GeneratedComputerUseResult::Success {
            screenshot: Some(ref screenshot),
            cursor_position: Some(ComputerCoordinate { x: 1, y: 2 }),
        } if screenshot == "abc"
    ));
}

#[test]
fn production_computer_executor_uses_host_box_transport_and_persists_screenshot() {
    let port = Arc::new(ComputerTransportPort::default());
    let persist_calls = Arc::new(Mutex::new(Vec::<(Vec<u8>, String)>::new()));
    let persist_capture = Arc::clone(&persist_calls);
    let post_actions = Arc::new(Mutex::new(Vec::<String>::new()));
    let post_actions_capture = Arc::clone(&post_actions);
    let executor = ProductionComputerToolExecutor::new(
        Arc::clone(&port) as Arc<dyn RunnerBoxResourcePort>,
    )
    .with_persist_image_callback(Arc::new(move |bytes, mime| {
        persist_capture
            .lock()
            .expect("persist")
            .push((bytes.to_vec(), mime.to_string()));
        Some("file:///saved-computer.webp".into())
    }))
    .with_post_action_callback(Arc::new(move |tool_call_id| {
        post_actions_capture.lock().expect("post actions").push(tool_call_id.to_string());
    }));

    let result = executor
        .execute(
            &ComputerActionArgs::simple(ComputerActionName::Screenshot),
            "tool-live",
        )
        .expect("computer execute");

    let requests = port.requests.lock().expect("requests");
    assert_eq!(requests.len(), 1);
    assert!(!requests[0].is_empty());
    drop(requests);

    assert!(matches!(
        result,
        ComputerUseResult::Success(ref success)
            if success.screenshot.as_deref() == Some("AQID")
                && success.screenshot_path.as_deref() == Some("file:///saved-computer.webp")
    ));
    assert_eq!(
        persist_calls.lock().expect("persist").as_slice(),
        &[(vec![1, 2, 3], "image/webp".into())]
    );
    assert_eq!(
        post_actions.lock().expect("post actions").as_slice(),
        &["tool-live".to_string()]
    );
}


#[derive(Default)]
struct EmptyToolBridge;

impl RoutedToolBridge for EmptyToolBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(Vec::new())
    }

    fn call_tool(
        &self,
        _tool: &RoutedToolDefinition,
        _args: Value,
        _tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        Err(ProviderSessionError::Tool("unexpected delegated tool".into()))
    }
}

#[test]
fn computer_runtime_parser_rejects_present_empty_or_non_array_followups() {
    let executor: Arc<dyn ComputerToolExecutor> = Arc::new(
        ProductionComputerToolExecutor::new(
            Arc::new(ComputerTransportPort::default()) as Arc<dyn RunnerBoxResourcePort>,
        ),
    );
    let bridge = SandComputerToolBridge::new(Arc::new(EmptyToolBridge), executor);
    let tool = bridge
        .list_tools()
        .expect("tools")
        .into_iter()
        .find(|tool| tool.name == "Computer")
        .expect("Computer tool");

    let empty = bridge
        .call_tool(
            &tool,
            serde_json::json!({"action":"move","x":1,"y":2,"then":[]}),
            "empty-followups",
        )
        .expect_err("explicit empty then must fail frozen min(1)");
    assert!(empty.to_string().contains("at least one follow-up action"));

    let wrong_type = bridge
        .call_tool(
            &tool,
            serde_json::json!({"action":"move","x":1,"y":2,"then":{}}),
            "invalid-followups",
        )
        .expect_err("non-array then must fail");
    assert!(wrong_type.to_string().contains("then must be an array"));
}

#[test]
fn computer_tool_exposure_matches_frozen_runner_roles() {
    let executor: Arc<dyn ComputerToolExecutor> =
        Arc::new(ProductionComputerToolExecutor::new(
            Arc::new(ComputerTransportPort::default()) as Arc<dyn RunnerBoxResourcePort>,
        ));

    let full = SandComputerToolBridge::new(Arc::new(EmptyToolBridge), Arc::clone(&executor))
        .with_exposure(ComputerToolExposure::Full)
        .list_tools()
        .expect("full tools");
    assert!(full.iter().any(|tool| tool.name == "Computer"));
    assert!(full.iter().any(|tool| tool.name == "Screenshot"));

    let screenshot_only =
        SandComputerToolBridge::new(Arc::new(EmptyToolBridge), Arc::clone(&executor))
            .with_exposure(ComputerToolExposure::ScreenshotOnly);
    let screenshot_tools = screenshot_only.list_tools().expect("screenshot tools");
    assert!(!screenshot_tools.iter().any(|tool| tool.name == "Computer"));
    assert!(screenshot_tools.iter().any(|tool| tool.name == "Screenshot"));

    let disabled = SandComputerToolBridge::new(Arc::new(EmptyToolBridge), executor)
        .with_exposure(ComputerToolExposure::Disabled)
        .list_tools()
        .expect("disabled tools");
    assert!(disabled.is_empty());
}

#[test]
fn production_computer_executor_checks_live_takeover_before_input_but_allows_screenshot() {
    let port = Arc::new(ComputerTransportPort::default());
    let takeover = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let takeover_check = Arc::clone(&takeover);
    let executor = ProductionComputerToolExecutor::new(
        Arc::clone(&port) as Arc<dyn RunnerBoxResourcePort>,
    )
    .with_availability_check(Arc::new(move |args| {
        if takeover_check.load(std::sync::atomic::Ordering::SeqCst)
            && args.action != ComputerActionName::Screenshot
        {
            return Err(ProviderSessionError::Tool("human takeover".into()));
        }
        Ok(())
    }));

    let mut click = ComputerActionArgs::simple(ComputerActionName::Click);
    click.x = Some(1);
    click.y = Some(2);
    let blocked = executor.execute(&click, "blocked").expect_err("takeover blocks input");
    assert!(blocked.to_string().contains("human takeover"));
    assert!(port.requests.lock().expect("requests").is_empty());

    executor
        .execute(
            &ComputerActionArgs::simple(ComputerActionName::Screenshot),
            "screenshot",
        )
        .expect("screenshot remains read only");
    assert_eq!(port.requests.lock().expect("requests").len(), 1);

    takeover.store(false, std::sync::atomic::Ordering::SeqCst);
    executor.execute(&click, "released").expect("input after hand back");
    assert_eq!(port.requests.lock().expect("requests").len(), 2);
}


#[test]
fn remote_computer_snapshot_generation_is_primary_only_and_exactly_projected() {
    let parsed = parse_remote_computer_action(&serde_json::json!({
        "action": "click",
        "x": 10,
        "y": 20,
        "snapshotGeneration": 7
    }))
    .expect("parse generation-fenced action");
    assert_eq!(parsed.snapshot_generation, Some(7));
    assert_eq!(to_exact_action_value(&parsed)["snapshotGeneration"], 7);

    let nested = parse_remote_computer_action(&serde_json::json!({
        "action": "move",
        "x": 1,
        "y": 2,
        "then": [{
            "action": "wait",
            "durationMs": 1,
            "snapshotGeneration": 7
        }]
    }))
    .expect_err("nested generation must not override the primary observation fence");
    assert!(nested.to_string().contains("only allowed on the primary action"));

    let zero = parse_remote_computer_action(&serde_json::json!({
        "action": "screenshot",
        "snapshotGeneration": 0
    }))
    .expect_err("generation zero is not a successful observation");
    assert!(zero.to_string().contains("positive integer"));
}
