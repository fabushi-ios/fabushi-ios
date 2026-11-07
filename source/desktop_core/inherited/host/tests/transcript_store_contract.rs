use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mahayana_host_runtime::extensions::transcript::transcript_hub::{
    Clock, RealClock, RUNNER_UNATTACHED_MESSAGE, transcript_entry_id,
    transcript_entry_kind,
};
use mahayana_host_runtime::extensions::transcript::transcript_store::TranscriptStore;
use serde_json::json;

#[test]
fn transcript_store_matches_frozen_copy_append_update_remove_clear_contract() {
    let store = TranscriptStore::new();
    assert!(store.get_transcript().is_empty());

    let original = vec![
        json!({"id":"one","kind":"message","content":"hello"}),
        json!({"id":"two","kind":"notice","text":"world"}),
    ];
    store.set_transcript(&original);
    let snapshot = store.get_transcript();
    assert_eq!(snapshot, original);

    store.append_entry(json!({"id":"three","kind":"message","content":"third"}));
    assert_eq!(store.get_transcript().len(), 3);

    let updated = store
        .update_entry("one", |entry| {
            let mut next = entry.clone();
            next["content"] = json!("changed");
            next
        })
        .expect("updated entry");
    assert_eq!(updated["content"], "changed");
    assert_eq!(store.get_transcript()[0]["content"], "changed");
    assert!(store.update_entry("missing", Clone::clone).is_none());

    assert!(store.remove_entry("two"));
    assert!(!store.remove_entry("missing"));
    assert_eq!(
        store
            .get_transcript()
            .iter()
            .filter_map(transcript_entry_id)
            .collect::<Vec<_>>(),
        vec!["one", "three"]
    );

    store.clear_transcript();
    assert!(store.is_empty());
}

#[test]
fn transcript_hub_exposes_frozen_entry_helpers_and_disposable_clock() {
    assert_eq!(
        RUNNER_UNATTACHED_MESSAGE,
        "Sand agent runner factory is not attached."
    );
    let entry = json!({"id":"entry-1","kind":"send-message"});
    assert_eq!(transcript_entry_id(&entry), Some("entry-1"));
    assert_eq!(transcript_entry_kind(&entry), Some("send-message"));

    let clock = RealClock;
    assert!(clock.now_ms() > 0);

    let fired = Arc::new(AtomicBool::new(false));
    let fired_task = Arc::clone(&fired);
    let disposable = clock.schedule(
        25,
        Box::new(move || fired_task.store(true, Ordering::Release)),
    );
    disposable.dispose();
    std::thread::sleep(Duration::from_millis(50));
    assert!(!fired.load(Ordering::Acquire));
}
