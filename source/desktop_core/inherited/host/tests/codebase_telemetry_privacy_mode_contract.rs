use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use mahayana_host_runtime::cursor_backend::SandPrivacyMode;
use mahayana_host_runtime::extensions::codebase_telemetry::privacy_mode::{
    CodebaseTelemetryPrivacyMode, PrivacyLookupError, TelemetryAuth,
};

fn auth(id: &str) -> TelemetryAuth {
    TelemetryAuth { auth_id: id.into(), auth_token: format!("token-{id}") }
}

fn wait_until(predicate: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if predicate() { return; }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("condition did not become true");
}

#[test]
fn auth_switch_resets_to_unspecified_and_stale_lookup_cannot_win() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_loader = Arc::clone(&calls);
    let privacy = CodebaseTelemetryPrivacyMode::with_policy(
        Arc::new(move |credentials| {
            calls_for_loader.fetch_add(1, Ordering::SeqCst);
            if credentials.auth_id == "old" {
                thread::sleep(Duration::from_millis(80));
                Ok(SandPrivacyMode::UsageCodebaseTrainingAllowed)
            } else {
                Ok(SandPrivacyMode::NoStorage)
            }
        }),
        Duration::from_secs(1),
        Duration::from_millis(1),
        Duration::from_secs(60),
    );
    privacy.set_auth(auth("old"));
    privacy.set_auth(auth("new"));
    wait_until(|| privacy.get() == SandPrivacyMode::NoStorage);
    thread::sleep(Duration::from_millis(100));
    assert_eq!(privacy.get(), SandPrivacyMode::NoStorage);
    assert!(calls.load(Ordering::SeqCst) >= 2);
}

#[test]
fn retryable_lookup_is_retried_once_and_clear_auth_fails_closed() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_loader = Arc::clone(&calls);
    let privacy = CodebaseTelemetryPrivacyMode::with_policy(
        Arc::new(move |_| {
            let n = calls_for_loader.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Err(PrivacyLookupError::new("unavailable", true))
            } else {
                Ok(SandPrivacyMode::NoTraining)
            }
        }),
        Duration::from_secs(1),
        Duration::from_millis(5),
        Duration::from_secs(60),
    );
    privacy.set_auth(auth("user"));
    wait_until(|| privacy.get() == SandPrivacyMode::NoTraining);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    privacy.clear_auth();
    assert_eq!(privacy.get(), SandPrivacyMode::Unspecified);
}

#[test]
fn deadline_is_retryable_and_subscribers_observe_committed_modes() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let privacy = CodebaseTelemetryPrivacyMode::with_policy(
        Arc::new(move |_| {
            thread::sleep(Duration::from_millis(60));
            Ok(SandPrivacyMode::UsageDataTrainingAllowed)
        }),
        Duration::from_millis(10),
        Duration::from_millis(1),
        Duration::from_secs(60),
    );
    let seen_for_listener = Arc::clone(&seen);
    let _subscription = privacy.subscribe(Arc::new(move |mode| {
        seen_for_listener.lock().unwrap().push(mode);
    }));
    privacy.set_auth(auth("slow"));
    thread::sleep(Duration::from_millis(80));
    assert_eq!(privacy.get(), SandPrivacyMode::Unspecified);
    assert!(seen.lock().unwrap().iter().all(|mode| *mode != SandPrivacyMode::UsageDataTrainingAllowed));
}
