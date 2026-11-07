use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::extension_ids_generated::HostExtensionId;
use mahayana_host_runtime::extensions::trays::extension::{
    TRAYS_DEPENDENCIES, HostTraysExtension, trays_extension_id,
};
use mahayana_host_runtime::extensions::trays::trays_service::{
    MAX_TRAYS, PushErrorOptions, TrayEvent, TrayManager,
};

fn error(title: &str, agent_id: &str) -> PushErrorOptions {
    PushErrorOptions {
        agent_id: Some(agent_id.to_string()),
        title: title.to_string(),
        detail: format!("{title} detail"),
        ..PushErrorOptions::default()
    }
}

#[test]
fn trays_extension_preserves_grok_owner_and_dependency_boundary() {
    assert_eq!(trays_extension_id(), HostExtensionId::Trays);
    assert!(TRAYS_DEPENDENCIES.is_empty());
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<HostTraysExtension>();
}

#[test]
fn tray_manager_dedupes_updates_and_emits_frozen_events() {
    let ids = Arc::new(Mutex::new(vec!["tray-2".to_string(), "tray-1".to_string()]));
    let ids_for_factory = Arc::clone(&ids);
    let clock = Arc::new(Mutex::new(vec![200_u64, 100_u64]));
    let clock_for_factory = Arc::clone(&clock);
    let manager = TrayManager::new(
        move || ids_for_factory.lock().expect("ids").pop().expect("id"),
        move || clock_for_factory.lock().expect("clock").pop().expect("now"),
    );
    let observed = Arc::new(Mutex::new(Vec::<TrayEvent>::new()));
    let observed_for_listener = Arc::clone(&observed);
    let _subscription = manager.subscribe(Arc::new(move |event| {
        observed_for_listener.lock().expect("events").push(event);
    }));

    let first = manager.push_error(PushErrorOptions {
        dedupe_key: Some("provider".into()),
        count: None,
        ..error("first", "agent-a")
    });
    assert_eq!(first.id, "tray-1");
    assert_eq!(first.count, Some(1));
    assert_eq!(first.created_at, 100);

    let updated = manager.push_error(PushErrorOptions {
        dedupe_key: Some("provider".into()),
        request_id: Some("request-2".into()),
        error_kind: Some("provider_overloaded".into()),
        ..error("second", "ignored-agent")
    });
    assert_eq!(updated.id, first.id);
    assert_eq!(updated.agent_id, first.agent_id);
    assert_eq!(updated.title, "second");
    assert_eq!(updated.detail, "second detail");
    assert_eq!(updated.request_id.as_deref(), Some("request-2"));
    assert_eq!(updated.error_kind.as_deref(), Some("provider_overloaded"));
    assert_eq!(updated.count, Some(2));
    assert_eq!(updated.created_at, 200);
    assert_eq!(manager.get_trays(), vec![updated.clone()]);

    let events = observed.lock().expect("events");
    assert_eq!(events.len(), 2);
    assert!(matches!(&events[0], TrayEvent::Pushed(tray) if tray == &first));
    assert!(matches!(&events[1], TrayEvent::Pushed(tray) if tray == &updated));
}

#[test]
fn tray_manager_enforces_cap_dismisses_and_clears_per_agent() {
    let next_id = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let ids = Arc::clone(&next_id);
    let now = Arc::new(std::sync::atomic::AtomicU64::new(1_000));
    let clock = Arc::clone(&now);
    let manager = TrayManager::new(
        move || format!(
            "tray-{}",
            ids.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ),
        move || clock.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
    );
    let observed = Arc::new(Mutex::new(Vec::<TrayEvent>::new()));
    let events = Arc::clone(&observed);
    let subscription = manager.subscribe(Arc::new(move |event| {
        events.lock().expect("events").push(event);
    }));

    for index in 0..=MAX_TRAYS {
        manager.push_error(error(
            &format!("error-{index}"),
            if index % 2 == 0 { "agent-a" } else { "agent-b" },
        ));
    }
    let trays = manager.get_trays();
    assert_eq!(trays.len(), MAX_TRAYS);
    assert_eq!(trays.first().expect("first retained tray").id, "tray-1");
    assert!(observed
        .lock()
        .expect("events")
        .iter()
        .any(|event| matches!(event, TrayEvent::Dismissed(id) if id == "tray-0")));

    manager.clear_for_agent("agent-a");
    assert!(manager
        .get_trays()
        .iter()
        .all(|tray| tray.agent_id.as_deref() != Some("agent-a")));

    let remaining_id = manager
        .get_trays()
        .first()
        .expect("remaining agent-b tray")
        .id
        .clone();
    assert!(manager.dismiss(&remaining_id));
    assert!(!manager.dismiss("missing"));

    manager.clear_all();
    assert!(manager.get_trays().is_empty());
    assert!(matches!(
        observed.lock().expect("events").last(),
        Some(TrayEvent::Cleared)
    ));

    drop(subscription);
    let before = observed.lock().expect("events").len();
    manager.push_error(error("after-unsubscribe", "agent-c"));
    assert_eq!(observed.lock().expect("events").len(), before);
}
