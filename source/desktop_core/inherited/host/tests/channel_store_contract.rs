use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::session::channel_store::{
    CHANNEL_CHANGE_DEBOUNCE_MS, FileChannelStore, get_agent_channels_dir, label_for,
};
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-channel-store-{label}-{}-{suffix}",
        std::process::id()
    ))
}

fn wait_for_changes(changes: &AtomicUsize, at_least: usize) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while changes.load(Ordering::SeqCst) < at_least && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        changes.load(Ordering::SeqCst) >= at_least,
        "timed out waiting for channel-store change {at_least}"
    );
}

fn wait_for_quiet_changes(changes: &AtomicUsize) -> usize {
    let deadline = Instant::now() + Duration::from_secs(2);
    let quiet_for = Duration::from_millis(CHANNEL_CHANGE_DEBOUNCE_MS * 3 + 20);
    let mut observed = changes.load(Ordering::SeqCst);
    let mut unchanged_since = Instant::now();
    while Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
        let current = changes.load(Ordering::SeqCst);
        if current != observed {
            observed = current;
            unchanged_since = Instant::now();
            continue;
        }
        if unchanged_since.elapsed() >= quiet_for {
            return observed;
        }
    }
    panic!("channel-store watcher did not become quiet");
}

#[test]
fn labels_match_frozen_grok_clamping_and_connector_defaults() {
    assert_eq!(label_for("slack", None), "Slack");
    assert_eq!(label_for("discord", Some("  ")), "Discord");
    assert_eq!(label_for("matrix", None), "matrix");
    assert_eq!(
        label_for("slack", Some("  Team   Operations  ")),
        "Team Operations"
    );
    assert_eq!(
        label_for("slack", Some(&"x".repeat(100))).chars().count(),
        80
    );
}

#[test]
fn file_channel_store_debounces_internal_mutations_and_keeps_frozen_file_contract() {
    let root = temp_root("file");
    let store = FileChannelStore::new(root.join("channels"));
    let changes = Arc::new(AtomicUsize::new(0));
    store.set_on_change(Some({
        let changes = Arc::clone(&changes);
        Arc::new(move || {
            changes.fetch_add(1, Ordering::SeqCst);
        })
    }));

    assert!(!store.write_metadata("../escape", "bad").expect("invalid platform"));
    assert!(store.write_metadata("slack", "").expect("write slack"));
    assert!(store.write_metadata("discord", "  Community   Bot ").expect("write discord"));
    assert_eq!(store.read_label("slack").as_deref(), Some("Slack"));
    assert_eq!(store.read_label("discord").as_deref(), Some("Community Bot"));
    assert_eq!(store.list_platforms(), vec!["discord".to_string(), "slack".to_string()]);
    let connections = store.list_connections();
    assert_eq!(connections.len(), 2);
    assert_eq!(connections[0].status, "configured");

    wait_for_changes(&changes, 1);
    thread::sleep(Duration::from_millis(CHANNEL_CHANGE_DEBOUNCE_MS + 40));
    assert_eq!(
        changes.load(Ordering::SeqCst),
        1,
        "two writes in one debounce window must collapse"
    );

    assert!(store.remove("slack").expect("remove slack"));
    assert!(!store.remove("slack").expect("remove absent slack"));
    assert_eq!(store.list_platforms(), vec!["discord".to_string()]);
    wait_for_changes(&changes, 2);

    store.set_on_change(None);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn file_channel_store_observes_external_filesystem_changes_with_debounce() {
    let root = temp_root("watch");
    let channels = root.join("channels");
    let store = FileChannelStore::new(&channels);
    let changes = Arc::new(AtomicUsize::new(0));
    store.set_on_change(Some({
        let changes = Arc::clone(&changes);
        Arc::new(move || {
            changes.fetch_add(1, Ordering::SeqCst);
        })
    }));

    let slack_dir = channels.join("slack");
    fs::create_dir_all(&slack_dir).expect("external platform dir");
    fs::write(
        slack_dir.join("connection.json"),
        "{\n  \"label\": \"External Slack\"\n}\n",
    )
    .expect("external config");
    wait_for_changes(&changes, 1);
    assert_eq!(store.read_label("slack").as_deref(), Some("External Slack"));

    // notify on macOS can deliver trailing directory-create/write events after
    // the first debounced callback. Establish a quiet boundary before testing
    // a distinct burst; the production contract remains a 50 ms trailing-edge
    // debounce within each burst, matching frozen WatchedDirectory behavior.
    let before = wait_for_quiet_changes(&changes);
    for label in ["One", "Two", "Three"] {
        fs::write(
            slack_dir.join("connection.json"),
            format!("{{\n  \"label\": \"{label}\"\n}}\n"),
        )
        .expect("external burst");
        thread::sleep(Duration::from_millis(10));
    }
    wait_for_changes(&changes, before + 1);
    let after_burst = wait_for_quiet_changes(&changes);
    assert_eq!(
        after_burst,
        before + 1,
        "external burst must collapse to one debounced callback"
    );
    assert_eq!(store.read_label("slack").as_deref(), Some("Three"));

    store.set_on_change(None);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn production_connector_credentials_materialize_channel_metadata_and_filter_on_token() {
    let root = temp_root("production");
    let agents = root.join("agents");
    let runtime = ProductionSessionWorkers::with_agents_root(&agents, 500);
    let record = runtime
        .materialize_new_session(None, "user", None)
        .expect("materialize session");

    assert!(runtime
        .store_connector_credential(&record.id, "slack", "token", "secret-token")
        .expect("store token"));
    assert_eq!(
        fs::read_to_string(
            get_agent_channels_dir(&agents.join(&record.id))
                .join("slack")
                .join("connection.json")
        )
        .expect("channel metadata"),
        "{\n  \"label\": \"Slack\"\n}\n"
    );
    let channels = runtime.list_agent_channels(&record.id).expect("list channels");
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].platform, "slack");
    assert_eq!(channels[0].label, "Slack");

    assert!(runtime
        .store_connector_credential(&record.id, "discord", "clientId", "id-only")
        .expect("store non-token secret"));
    assert_eq!(
        runtime.list_agent_channels(&record.id).expect("filtered channels").len(),
        1
    );

    assert!(runtime.disconnect_channel(&record.id, "slack").expect("disconnect"));
    assert!(runtime.list_agent_channels(&record.id).expect("after disconnect").is_empty());
    assert_eq!(
        runtime.get_connector_secret(&record.id, "slack", "token").expect("secret read"),
        None
    );

    runtime.shutdown();
    let _ = fs::remove_dir_all(root);
}
