use std::cell::RefCell;
use std::io;
use std::sync::Arc;

use mahayana_host_runtime::ports::r#box::{
    SAND_BOX_NOT_READY_MESSAGE, SAND_BOX_NO_MONITOR_AVAILABLE_MESSAGE,
    SAND_BOX_NOT_RESPONDING_MESSAGE, SandBoxDaemonUnreachableError,
};
use mahayana_host_runtime::runner::remote_box_resources::{
    RemoteBoxResourceCoordinator, RemoteConnection, RemoteConnectError,
    RemoteResourceKind, RemoteResourceLifecycleEvent, RemoteShellKind,
    execute_remote_resource_lifecycle_plan, should_register_auto_review_classifier,
};
use mahayana_host_runtime::runner::sand_auto_review::SandAutoReviewModes;

fn connection(name: &str, monitor: bool) -> RemoteConnection<String> {
    RemoteConnection {
        terminals_folder: format!("/tmp/{name}/terminals"),
        resource: name.to_string(),
        owns_monitor: monitor,
        window_index: 7,
    }
}

#[test]
fn prepared_and_cached_connections_are_reused_and_publish_terminal_folder() {
    let prepared = connection("prepared", true);
    let mut coordinator =
        RemoteBoxResourceCoordinator::new(true, Some(prepared.clone()));
    let mut ensure_calls = 0usize;

    let first = coordinator
        .connect(false, || {
            ensure_calls += 1;
            Ok(connection("ensured", true))
        })
        .expect("prepared connection");
    assert_eq!(first, prepared);
    assert_eq!(ensure_calls, 0);
    assert_eq!(
        coordinator.terminals_folder(),
        Some("/tmp/prepared/terminals")
    );

    let second = coordinator
        .connect(false, || {
            ensure_calls += 1;
            Ok(connection("second", true))
        })
        .expect("cached connection");
    assert_eq!(second.resource, "prepared");
    assert_eq!(ensure_calls, 0);
    assert!(coordinator.has_cached_connection());
}

#[test]
fn preparing_status_can_delegate_to_the_host_owned_bounded_readiness_wait() {
    let mut coordinator: RemoteBoxResourceCoordinator<String> =
        RemoteBoxResourceCoordinator::new(true, None);
    let mut ensure_calls = 0usize;
    let prepared = coordinator
        .connect(false, || {
            ensure_calls += 1;
            Ok(connection("ready-after-wait", true))
        })
        .expect("Host-owned readiness wait should be allowed to settle a preparing box");
    assert_eq!(prepared.resource, "ready-after-wait");
    assert_eq!(ensure_calls, 1);
    assert!(coordinator.has_cached_connection());

    coordinator.clear_connection();
    let timeout: RemoteConnectError = Arc::new(
        SandBoxDaemonUnreachableError::new("timeout", "deadline"),
    );
    let failed = coordinator
        .connect(false, || Err(timeout))
        .unwrap_err();
    assert_eq!(failed.0, SAND_BOX_NOT_RESPONDING_MESSAGE);
    assert!(!coordinator.has_cached_connection());

    let retried = coordinator
        .connect(false, || Ok(connection("recovered", true)))
        .expect("retry");
    assert_eq!(retried.resource, "recovered");
}

#[test]
fn unknown_connect_failures_use_generic_not_ready_message() {
    let mut coordinator: RemoteBoxResourceCoordinator<String> =
        RemoteBoxResourceCoordinator::new(true, None);
    let error: RemoteConnectError =
        Arc::new(io::Error::new(io::ErrorKind::Other, "unknown"));
    assert_eq!(
        coordinator.connect(false, || Err(error)).unwrap_err().0,
        SAND_BOX_NOT_READY_MESSAGE
    );
}

#[test]
fn shell_side_effect_plan_preserves_frozen_barrier_audit_navigation_order() {
    let coordinator = RemoteBoxResourceCoordinator::new(true, None);
    let connection = connection("box", true);
    let plan = coordinator.shell_stream_plan(
        &connection,
        "agent-1",
        "pwd",
        Some("turn-1"),
        "box-1",
    );
    assert_eq!(
        plan,
        vec![
            RemoteResourceLifecycleEvent::AutoReviewBarrier,
            RemoteResourceLifecycleEvent::AuditShell {
                agent_id: "agent-1".into(),
                kind: RemoteShellKind::Foreground,
                command: "pwd".into(),
                turn_id: Some("turn-1".into()),
                box_id: "box-1".into(),
            },
            RemoteResourceLifecycleEvent::AutoReviewBarrier,
            RemoteResourceLifecycleEvent::CaptureNavigationBaseline {
                window_index: 7,
            },
            RemoteResourceLifecycleEvent::AutoReviewBarrier,
            RemoteResourceLifecycleEvent::Delegate {
                resource: RemoteResourceKind::ShellStream,
            },
            RemoteResourceLifecycleEvent::ProbeNavigation,
        ]
    );

    let background = coordinator.background_shell_plan(
        &connection,
        "agent-1",
        "sleep 1",
        None,
        "box-1",
    );
    assert!(matches!(
        background[1],
        RemoteResourceLifecycleEvent::AuditShell {
            kind: RemoteShellKind::Background,
            ..
        }
    ));
}

#[test]
fn read_shell_and_computer_plans_preserve_distinct_side_effect_guards() {
    let coordinator = RemoteBoxResourceCoordinator::new(true, None);
    assert_eq!(
        coordinator.read_plan(),
        vec![RemoteResourceLifecycleEvent::Delegate {
            resource: RemoteResourceKind::Read
        }]
    );
    assert_eq!(
        coordinator.shell_plan(),
        vec![
            RemoteResourceLifecycleEvent::AutoReviewBarrier,
            RemoteResourceLifecycleEvent::AutoReviewBarrier,
            RemoteResourceLifecycleEvent::Delegate {
                resource: RemoteResourceKind::Shell
            },
        ]
    );

    let plan = coordinator
        .computer_use_plan(&connection("box", true), Some("click"))
        .expect("computer plan");
    assert_eq!(
        plan,
        vec![
            RemoteResourceLifecycleEvent::CaptureNavigationBaseline {
                window_index: 7
            },
            RemoteResourceLifecycleEvent::TouchMonitorBusyLease {
                window_index: 7
            },
            RemoteResourceLifecycleEvent::AutoReviewBarrier,
            RemoteResourceLifecycleEvent::Delegate {
                resource: RemoteResourceKind::ComputerUse
            },
            RemoteResourceLifecycleEvent::RecordComputerAuditIntent {
                action_case: Some("click".into())
            },
            RemoteResourceLifecycleEvent::ProbeNavigation,
        ]
    );
}

#[test]
fn no_monitor_fails_closed_and_connection_can_be_invalidated_for_retry() {
    let mut coordinator =
        RemoteBoxResourceCoordinator::new(true, Some(connection("box", false)));
    let active = coordinator
        .connect(false, || unreachable!())
        .expect("prepared");
    assert_eq!(
        coordinator.computer_use_plan(&active, Some("screenshot"))
            .unwrap_err()
            .0,
        SAND_BOX_NO_MONITOR_AVAILABLE_MESSAGE
    );
    assert!(coordinator.has_cached_connection());
    assert_eq!(
        coordinator.no_monitor_error().0,
        SAND_BOX_NO_MONITOR_AVAILABLE_MESSAGE
    );
    assert!(!coordinator.has_cached_connection());
}

#[test]
fn classifier_registration_requires_executor_and_any_non_off_mode() {
    assert!(!should_register_auto_review_classifier(
        true,
        &SandAutoReviewModes::off()
    ));
    assert!(!should_register_auto_review_classifier(
        false,
        &SandAutoReviewModes::enforce()
    ));
    assert!(should_register_auto_review_classifier(
        true,
        &SandAutoReviewModes::shadow()
    ));
}


#[test]
fn lifecycle_executor_keeps_final_probe_after_delegate_error_but_skips_success_audit() {
    let plan = vec![
        RemoteResourceLifecycleEvent::AutoReviewBarrier,
        RemoteResourceLifecycleEvent::Delegate {
            resource: RemoteResourceKind::ComputerUse,
        },
        RemoteResourceLifecycleEvent::RecordComputerAuditIntent {
            action_case: Some("click".into()),
        },
        RemoteResourceLifecycleEvent::ProbeNavigation,
    ];
    let events = RefCell::new(Vec::<&'static str>::new());
    let result: Result<(), &'static str> = execute_remote_resource_lifecycle_plan(
        &plan,
        |event| {
            events.borrow_mut().push(match event {
                RemoteResourceLifecycleEvent::AutoReviewBarrier => "barrier",
                RemoteResourceLifecycleEvent::RecordComputerAuditIntent { .. } => "audit-intent",
                RemoteResourceLifecycleEvent::ProbeNavigation => "probe",
                _ => "other",
            });
            Ok(())
        },
        |resource| {
            assert_eq!(resource, RemoteResourceKind::ComputerUse);
            events.borrow_mut().push("delegate");
            Err("delegate-failed")
        },
    );
    assert_eq!(result, Err("delegate-failed"));
    assert_eq!(&*events.borrow(), &["barrier", "delegate", "probe"]);
}

#[test]
fn lifecycle_executor_runs_success_post_delegate_intent_before_probe() {
    let plan = vec![
        RemoteResourceLifecycleEvent::Delegate {
            resource: RemoteResourceKind::ComputerUse,
        },
        RemoteResourceLifecycleEvent::RecordComputerAuditIntent {
            action_case: Some("click".into()),
        },
        RemoteResourceLifecycleEvent::ProbeNavigation,
    ];
    let events = RefCell::new(Vec::<&'static str>::new());
    let result = execute_remote_resource_lifecycle_plan(
        &plan,
        |event| {
            events.borrow_mut().push(match event {
                RemoteResourceLifecycleEvent::RecordComputerAuditIntent { .. } => "audit-intent",
                RemoteResourceLifecycleEvent::ProbeNavigation => "probe",
                _ => "other",
            });
            Ok::<(), &'static str>(())
        },
        |resource| {
            assert_eq!(resource, RemoteResourceKind::ComputerUse);
            events.borrow_mut().push("delegate");
            Ok::<u32, &'static str>(7)
        },
    )
    .expect("lifecycle");
    assert_eq!(result, 7);
    assert_eq!(&*events.borrow(), &["delegate", "audit-intent", "probe"]);
}
