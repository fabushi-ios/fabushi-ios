use std::fs;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::extension_ids_generated::HostExtensionId;
use mahayana_host_runtime::extensions::local_tool_permission::extension::{
    LOCAL_TOOL_PERMISSION_DEPENDENCIES, local_tool_permission_extension_id,
    start_local_tool_permission_extension,
};
use mahayana_host_runtime::extensions::local_tool_permission::local_tool_permission_controller::{
    SAND_LOCAL_TOOLS_ABANDONED_MESSAGE, SAND_LOCAL_TOOLS_ASK_UNAVAILABLE_MESSAGE,
    SAND_LOCAL_TOOLS_DISABLED_MESSAGE, SAND_LOCAL_TOOLS_PREPARATORY_MESSAGE,
    SAND_LOCAL_TOOLS_STALE_TASK_MESSAGE, SandLocalToolControllerEventKind,
    SandLocalToolPermissionController, SandLocalToolRequest,
    SandLocalToolRequestStatus, SandLocalToolScope,
};
use mahayana_host_runtime::extensions::local_tool_permission::local_tool_permission_resolution::{
    LocalToolPermissionAskStore, SandLocalToolResolution,
};
use mahayana_host_runtime::extensions::settings::settings_service::{
    SandLocalToolPermission, SettingsService,
};

fn temp_settings(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-local-tool-permission-{label}-{}-{suffix}.json",
        std::process::id()
    ))
}

#[test]
fn extension_identity_and_standing_modes_match_frozen_contract() {
    assert_eq!(
        local_tool_permission_extension_id(),
        HostExtensionId::LocalToolPermission
    );
    assert_eq!(
        LOCAL_TOOL_PERMISSION_DEPENDENCIES,
        &[
            HostExtensionId::Settings,
            HostExtensionId::Telemetry,
            HostExtensionId::Transcript,
        ]
    );

    let path = temp_settings("standing");
    let settings = Arc::new(SettingsService::new(path.clone()));
    let extension = start_local_tool_permission_extension(Arc::clone(&settings));
    let scope = SandLocalToolScope {
        agent_id: "agent-a".into(),
        session_key: None,
        tool_call_id: Some("tool-a".into()),
        action: Some("read-file".into()),
        direction_epoch: None,
    };
    let request = SandLocalToolRequest::simple("read-file", "/tmp/a");

    settings
        .set_local_tool_permission(SandLocalToolPermission::Never)
        .expect("never");
    let denied = extension.authorize(Some(&scope), &request);
    assert!(!denied.allowed);
    assert_eq!(
        denied.reason.as_deref(),
        Some(SAND_LOCAL_TOOLS_DISABLED_MESSAGE)
    );

    settings
        .set_local_tool_permission(SandLocalToolPermission::Always)
        .expect("always");
    assert!(extension.authorize(Some(&scope), &request).allowed);

    settings
        .set_local_tool_permission(SandLocalToolPermission::Ask)
        .expect("ask");
    let denied = extension.authorize(Some(&scope), &request);
    assert_eq!(
        denied.reason.as_deref(),
        Some(SAND_LOCAL_TOOLS_ASK_UNAVAILABLE_MESSAGE)
    );
    let _ = fs::remove_file(path);
}

#[test]
fn ask_resolution_wakes_waiter_and_retains_one_time_approval_until_scope_completes() {
    let path = temp_settings("ask");
    let settings = Arc::new(SettingsService::new(path.clone()));
    let controller = Arc::new(SandLocalToolPermissionController::with_options(
        Arc::clone(&settings),
        1_000,
        Arc::new(|| 10_000),
        Arc::new(|| "approval-1".to_string()),
    ));
    controller.bind_ask_surfaces(Arc::new(|_| true));
    controller.bind_live_computer_check(Arc::new(|_| true));
    let events = Arc::new(Mutex::new(Vec::new()));
    let events_sink = Arc::clone(&events);
    controller.bind_event_sink(Some(Arc::new(move |event| {
        events_sink.lock().expect("events").push(event);
    })));

    let scope = SandLocalToolScope {
        agent_id: "agent-a".into(),
        session_key: None,
        tool_call_id: Some("tool-a".into()),
        action: Some("read-file".into()),
        direction_epoch: None,
    };
    let request = SandLocalToolRequest::simple("read-file", "/tmp/a");
    let worker_controller = Arc::clone(&controller);
    let worker_scope = scope.clone();
    let worker_request = request.clone();
    let worker = thread::spawn(move || {
        worker_controller.authorize(Some(&worker_scope), &worker_request)
    });

    let pending = (0..100)
        .find_map(|_| {
            let pending = controller.get_pending_request_for_agent("agent-a");
            if pending.is_none() {
                thread::sleep(Duration::from_millis(2));
            }
            pending
        })
        .expect("pending ask");
    assert_eq!(pending.id, "approval-1");
    assert_eq!(pending.status, SandLocalToolRequestStatus::Pending);
    assert!(controller.resolve_request(
        &pending.id,
        SandLocalToolResolution::AllowOnce
    ));
    let decision = worker.join().expect("worker");
    assert!(decision.allowed);
    assert_eq!(decision.approval_id.as_deref(), Some("approval-1"));
    assert!(controller.was_settled("approval-1"));
    assert_eq!(controller.live_approval_ids(), vec!["approval-1"]);

    let covered = controller.authorize(Some(&scope), &request);
    assert!(covered.allowed);
    assert_eq!(covered.approval_id.as_deref(), Some("approval-1"));

    controller.complete_scope(Some(&scope));
    assert!(controller.live_approval_ids().is_empty());
    assert!(
        events
            .lock()
            .expect("events")
            .iter()
            .any(|event| event.status == SandLocalToolRequestStatus::Allowed)
    );
    let _ = fs::remove_file(path);
}


#[test]
fn denied_action_is_remembered_and_forgotten_agent_is_fenced() {
    let path = temp_settings("refusal-memory");
    let settings = Arc::new(SettingsService::new(path.clone()));
    let controller = Arc::new(SandLocalToolPermissionController::with_options(
        Arc::clone(&settings),
        1_000,
        Arc::new(|| 1_000),
        Arc::new(|| "ask-refusal".to_string()),
    ));
    controller.bind_ask_surfaces(Arc::new(|_| true));
    controller.bind_live_computer_check(Arc::new(|_| true));
    let scope = SandLocalToolScope {
        agent_id: "agent-r".into(),
        session_key: None,
        tool_call_id: Some("tool-r".into()),
        action: Some("read-file".into()),
        direction_epoch: Some(0),
    };
    let request = SandLocalToolRequest::simple("read-file", "/tmp/refused");
    let worker_controller = Arc::clone(&controller);
    let worker_scope = scope.clone();
    let worker_request = request.clone();
    let worker = thread::spawn(move || worker_controller.authorize(Some(&worker_scope), &worker_request));
    let pending = (0..100).find_map(|_| {
        let value = controller.get_pending_request_for_agent("agent-r");
        if value.is_none() { thread::sleep(Duration::from_millis(2)); }
        value
    }).expect("pending");
    assert!(controller.resolve_request(&pending.id, SandLocalToolResolution::Deny));
    assert!(!worker.join().expect("worker").allowed);
    assert_eq!(controller.remembered_refusal_count(), 1);
    assert_eq!(
        controller.authorize(Some(&scope), &request).reason.as_deref(),
        Some(SAND_LOCAL_TOOLS_ABANDONED_MESSAGE)
    );
    controller.forget_agent("agent-r");
    assert_eq!(
        controller.authorize(Some(&scope), &request).reason.as_deref(),
        Some(SAND_LOCAL_TOOLS_STALE_TASK_MESSAGE)
    );
    let _ = fs::remove_file(path);
}

#[test]
fn preparatory_action_is_fenced_and_scope_retirement_is_observable() {
    let path = temp_settings("preparatory");
    let settings = Arc::new(SettingsService::new(path.clone()));
    let controller = Arc::new(SandLocalToolPermissionController::with_options(
        Arc::clone(&settings),
        1_000,
        Arc::new(|| 2_000),
        Arc::new(|| "approval-prep".to_string()),
    ));
    controller.bind_ask_surfaces(Arc::new(|_| true));
    controller.bind_live_computer_check(Arc::new(|_| true));
    let scope = SandLocalToolScope {
        agent_id: "agent-p".into(),
        session_key: None,
        tool_call_id: Some("tool-p".into()),
        action: Some("run-command".into()),
        direction_epoch: None,
    };
    let read = SandLocalToolRequest::simple("read-file", "/tmp/a");
    assert_eq!(
        controller.authorize(Some(&scope), &read).reason.as_deref(),
        Some(SAND_LOCAL_TOOLS_PREPARATORY_MESSAGE)
    );

    let retired = Arc::new(Mutex::new(Vec::new()));
    let retired_sink = Arc::clone(&retired);
    controller.bind_approval_retired_sink(Some(Arc::new(move |id| {
        retired_sink.lock().expect("retired").push(id.to_string());
    })));

    let command = SandLocalToolRequest::simple("run-command", "echo hi");
    let worker_controller = Arc::clone(&controller);
    let worker_scope = scope.clone();
    let worker_command = command.clone();
    let worker = thread::spawn(move || worker_controller.authorize(Some(&worker_scope), &worker_command));
    let pending = (0..100).find_map(|_| {
        let value = controller.get_pending_request_for_agent("agent-p");
        if value.is_none() { thread::sleep(Duration::from_millis(2)); }
        value
    }).expect("pending");
    assert!(controller.resolve_request(&pending.id, SandLocalToolResolution::AllowOnce));
    assert!(worker.join().expect("worker").allowed);
    controller.complete_scope(Some(&scope));
    assert_eq!(retired.lock().expect("retired").as_slice(), &["approval-prep"]);
    let _ = fs::remove_file(path);
}


#[test]
fn standing_grant_with_implicit_epoch_does_not_reenter_state_lock() {
    let path = temp_settings("implicit-epoch");
    let settings = Arc::new(SettingsService::new(path.clone()));
    let controller = SandLocalToolPermissionController::with_options(
        Arc::clone(&settings),
        100,
        Arc::new(|| 1),
        Arc::new(|| "unused".to_string()),
    );
    controller.begin_turn("agent-lock");
    settings
        .set_local_tool_permission(SandLocalToolPermission::Always)
        .expect("always");
    controller.note_permission_changed();
    let scope = SandLocalToolScope {
        agent_id: "agent-lock".into(),
        session_key: None,
        tool_call_id: Some("tool".into()),
        action: Some("read-file".into()),
        direction_epoch: None,
    };
    assert!(
        controller
            .authorize(Some(&scope), &SandLocalToolRequest::simple("read-file", "/tmp/a"))
            .allowed
    );
    let _ = fs::remove_file(path);
}


#[test]
fn joined_waiter_cancellation_only_retires_pending_when_last_waiter_leaves() {
    let path = temp_settings("joined-cancel");
    let settings = Arc::new(SettingsService::new(path.clone()));
    let controller = Arc::new(SandLocalToolPermissionController::with_options(
        Arc::clone(&settings),
        2_000,
        Arc::new(|| 10_000),
        Arc::new(|| "joined-ask".to_string()),
    ));
    controller.bind_ask_surfaces(Arc::new(|_| true));
    controller.bind_live_computer_check(Arc::new(|_| true));

    let scope = SandLocalToolScope {
        agent_id: "agent-j".into(),
        session_key: None,
        tool_call_id: Some("tool-j".into()),
        action: Some("read-file".into()),
        direction_epoch: None,
    };
    let request = SandLocalToolRequest::simple("read-file", "/tmp/joined");
    let cancel_first = Arc::new(AtomicBool::new(false));
    let cancel_first_worker = Arc::clone(&cancel_first);
    let c1 = Arc::clone(&controller);
    let s1 = scope.clone();
    let r1 = request.clone();
    let w1 = thread::spawn(move || c1.authorize_with_cancel(Some(&s1), &r1, Some(&cancel_first_worker)));

    let c2 = Arc::clone(&controller);
    let s2 = scope.clone();
    let r2 = request.clone();
    let w2 = thread::spawn(move || c2.authorize(Some(&s2), &r2));

    let pending = (0..100).find_map(|_| {
        let value = controller.get_pending_request_for_agent("agent-j");
        if value.is_none() { thread::sleep(Duration::from_millis(2)); }
        value
    }).expect("pending");

    cancel_first.store(true, Ordering::Release);
    let first = w1.join().expect("cancelled waiter");
    assert_eq!(
        first.reason.as_deref(),
        Some(mahayana_host_runtime::extensions::local_tool_permission::local_tool_permission_controller::SAND_LOCAL_TOOLS_ASK_CANCELLED_MESSAGE)
    );
    assert!(controller.get_pending_request_by_id_full(&pending.id).is_some());

    assert!(controller.resolve_request(&pending.id, SandLocalToolResolution::AllowOnce));
    assert!(w2.join().expect("remaining waiter").allowed);
    let _ = fs::remove_file(path);
}

#[test]
fn controller_subscription_reports_created_and_settled_and_can_unsubscribe() {
    let path = temp_settings("subscribe");
    let settings = Arc::new(SettingsService::new(path.clone()));
    let controller = Arc::new(SandLocalToolPermissionController::with_options(
        Arc::clone(&settings),
        1_000,
        Arc::new(|| 20_000),
        Arc::new(|| "subscribe-ask".to_string()),
    ));
    controller.bind_ask_surfaces(Arc::new(|_| true));
    controller.bind_live_computer_check(Arc::new(|_| true));

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_sink = Arc::clone(&events);
    let subscription = controller.subscribe(Arc::new(move |event| {
        events_sink.lock().expect("events").push(event.kind);
    }));

    let scope = SandLocalToolScope {
        agent_id: "agent-s".into(),
        session_key: None,
        tool_call_id: Some("tool-s".into()),
        action: Some("read-file".into()),
        direction_epoch: None,
    };
    let request = SandLocalToolRequest::simple("read-file", "/tmp/sub");
    let worker_controller = Arc::clone(&controller);
    let worker_scope = scope.clone();
    let worker_request = request.clone();
    let worker = thread::spawn(move || worker_controller.authorize(Some(&worker_scope), &worker_request));
    let pending = (0..100).find_map(|_| {
        let value = controller.get_pending_request_for_agent("agent-s");
        if value.is_none() { thread::sleep(Duration::from_millis(2)); }
        value
    }).expect("pending");
    assert!(controller.resolve_request(&pending.id, SandLocalToolResolution::AllowOnce));
    assert!(worker.join().expect("worker").allowed);
    assert_eq!(
        events.lock().expect("events").as_slice(),
        &[SandLocalToolControllerEventKind::Created, SandLocalToolControllerEventKind::Settled]
    );
    subscription.unsubscribe();
    let _ = fs::remove_file(path);
}

#[test]
fn approved_long_target_precedes_utf16_size_fence_and_unapproved_target_uses_utf16_units() {
    let path = temp_settings("utf16");
    let settings = Arc::new(SettingsService::new(path.clone()));
    settings
        .set_local_tool_permission(SandLocalToolPermission::Always)
        .expect("always");
    let controller = SandLocalToolPermissionController::with_options(
        Arc::clone(&settings),
        100,
        Arc::new(|| 1),
        Arc::new(|| "unused".to_string()),
    );
    let scope = SandLocalToolScope {
        agent_id: "agent-u".into(),
        session_key: None,
        tool_call_id: Some("tool-u".into()),
        action: Some("read-file".into()),
        direction_epoch: None,
    };
    let huge = SandLocalToolRequest::simple("read-file", "😀".repeat(5_001));
    assert!(controller.authorize(Some(&scope), &huge).allowed);

    settings
        .set_local_tool_permission(SandLocalToolPermission::Ask)
        .expect("ask");
    controller.bind_ask_surfaces(Arc::new(|_| true));
    controller.bind_live_computer_check(Arc::new(|_| true));
    let denied = controller.authorize(Some(&scope), &huge);
    assert_eq!(
        denied.reason.as_deref(),
        Some(mahayana_host_runtime::extensions::local_tool_permission::local_tool_permission_controller::SAND_LOCAL_TOOLS_TARGET_TOO_LARGE_MESSAGE)
    );
    let _ = fs::remove_file(path);
}
