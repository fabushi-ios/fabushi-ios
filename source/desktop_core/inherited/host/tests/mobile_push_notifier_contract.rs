use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::notifications::mobile_push_notifier::{
    MobilePushInput, NotificationAgent, SAND_MOBILE_PUSH_FOCUS_FRESHNESS_MS,
    SandMobilePushNotifier,
};

fn agent(running: bool, awaiting: Option<&str>, message_id: Option<&str>) -> NotificationAgent {
    NotificationAgent {
        id: "agent-a".into(),
        name: "Alpha".into(),
        is_running: running,
        awaiting_reason: awaiting.map(str::to_string),
        notify_enabled: true,
        is_hidden_from_sidebar: false,
        last_message_id: message_id.map(str::to_string),
        last_message_preview: Some("finished work".into()),
    }
}

#[test]
fn buffers_preseed_delta_then_notifies_on_needs_input_transition() {
    let sent = Arc::new(Mutex::new(Vec::<MobilePushInput>::new()));
    let capture = Arc::clone(&sent);
    let mut notifier = SandMobilePushNotifier::new(Arc::new(move |input| {
        capture.lock().unwrap().push(input);
        Ok(())
    }));

    notifier.handle_agent_upserted(agent(true, Some("approve"), Some("m2")), None, 100);
    assert!(sent.lock().unwrap().is_empty());

    notifier.seed_baseline(&[agent(true, None, Some("m1"))], 100);
    let sent = sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].awaiting_user_response);
    assert_eq!(sent[0].last_message_id, "m2");
}

#[test]
fn finished_turn_requires_a_new_message_and_unfocused_window() {
    let sent = Arc::new(Mutex::new(Vec::<MobilePushInput>::new()));
    let capture = Arc::clone(&sent);
    let mut notifier = SandMobilePushNotifier::new(Arc::new(move |input| {
        capture.lock().unwrap().push(input);
        Ok(())
    }));

    notifier.seed_baseline(&[agent(true, None, Some("m1"))], 1_000);
    notifier.handle_agent_upserted(agent(false, None, Some("m1")), None, 2_000);
    assert!(sent.lock().unwrap().is_empty());

    notifier.handle_agent_upserted(agent(true, None, Some("m1")), None, 8_000);
    notifier.handle_agent_upserted(agent(false, None, Some("m2")), Some(8_000), 8_001);
    assert!(sent.lock().unwrap().is_empty());

    let stale_now_ms = SAND_MOBILE_PUSH_FOCUS_FRESHNESS_MS + 14_000;
    notifier.handle_agent_upserted(agent(true, None, Some("m2")), None, stale_now_ms);
    notifier.handle_agent_upserted(
        agent(false, None, Some("m3")),
        Some(stale_now_ms - SAND_MOBILE_PUSH_FOCUS_FRESHNESS_MS - 1),
        stale_now_ms,
    );
    let sent = sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert!(!sent[0].awaiting_user_response);
    assert_eq!(sent[0].message_preview, "finished work");
}

#[test]
fn forget_resets_baseline_and_notify_failures_are_nonfatal() {
    let mut notifier = SandMobilePushNotifier::new(Arc::new(|_| Err("offline".into())));
    notifier.seed_baseline(&[agent(true, None, Some("m1"))], 0);
    notifier.handle_agent_upserted(agent(false, None, Some("m2")), None, 10_000);
    notifier.forget("agent-a");
    notifier.handle_agent_upserted(agent(false, None, Some("m3")), None, 20_000);
}
