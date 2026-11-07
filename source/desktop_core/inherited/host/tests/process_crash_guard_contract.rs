use std::sync::{Arc, Mutex};

use mahayana_host_runtime::process_crash_guard::{
    ProcessCrashGuard, ProcessCrashKind, ProcessCrashReporter, handle_process_crash,
};

#[test]
fn crash_reporter_receives_scope_safe_panic_signal() {
    let seen = Arc::new(Mutex::new(Vec::<(String, ProcessCrashKind)>::new()));
    let capture = Arc::clone(&seen);
    let reporter: ProcessCrashReporter = Arc::new(move |message, kind| {
        capture.lock().expect("capture lock").push((message, kind));
    });

    handle_process_crash(
        "sand-host",
        Some(&reporter),
        ProcessCrashKind::Panic,
        "boom",
    );

    assert_eq!(
        *seen.lock().expect("capture lock"),
        vec![("boom".to_string(), ProcessCrashKind::Panic)]
    );
}

#[test]
fn crash_reporter_failures_are_swallowed_like_the_frozen_host_guard() {
    let reporter: ProcessCrashReporter = Arc::new(|_, _| panic!("reporter failed"));
    handle_process_crash(
        "sand-host",
        Some(&reporter),
        ProcessCrashKind::Panic,
        "original panic",
    );
}

#[test]
fn crash_guard_reporter_can_be_replaced_without_reinstalling_process_hooks() {
    let guard = ProcessCrashGuard::default();
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let capture = Arc::clone(&seen);
    guard.set_reporter(Some(Arc::new(move |message, _| {
        capture.lock().expect("capture lock").push(message);
    })));
    guard.set_reporter(None);
    assert!(seen.lock().expect("capture lock").is_empty());
}
