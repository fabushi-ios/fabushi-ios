use std::fs;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::box_store_sync::chrome_session_watcher::ChromeSessionWatcher;

fn root(label: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-chrome-session-watcher-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn wait_for_count(count: &AtomicUsize, expected: usize, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if count.load(Ordering::Acquire) == expected {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        count.load(Ordering::Acquire),
        expected,
        "callback count did not reach the expected value before timeout"
    );
}

#[test]
fn filters_frozen_session_db_prefix_family() {
    let watcher = ChromeSessionWatcher::new(
        root("filter"),
        ["Cookies", "Login Data", "Login Data For Account", "Web Data"],
        0,
        Arc::new(|| {}),
    );

    assert!(watcher.is_session_db_file("Cookies"));
    assert!(watcher.is_session_db_file("Cookies-journal"));
    assert!(watcher.is_session_db_file("Login Data-wal"));
    assert!(watcher.is_session_db_file("Login Data For Account-shm"));
    assert!(watcher.is_session_db_file("Web Data"));
    assert!(!watcher.is_session_db_file("History"));
    assert!(!watcher.is_session_db_file("Local State"));
}

#[test]
fn null_filename_only_triggers_after_a_session_db_mtime_change() {
    let root = root("mtime");
    fs::create_dir_all(&root).expect("root");
    fs::write(root.join("Cookies"), b"one").expect("seed");

    let count = Arc::new(AtomicUsize::new(0));
    let count_for_callback = Arc::clone(&count);
    let watcher = ChromeSessionWatcher::new(
        &root,
        ["Cookies", "Login Data"],
        0,
        Arc::new(move || {
            count_for_callback.fetch_add(1, Ordering::AcqRel);
        }),
    );
    watcher.refresh_session_db_mtimes();

    watcher.handle_fs_event(None);
    thread::sleep(Duration::from_millis(20));
    assert_eq!(count.load(Ordering::Acquire), 0);

    thread::sleep(Duration::from_millis(20));
    fs::write(root.join("Cookies"), b"two-two").expect("mutate");
    watcher.handle_fs_event(None);
    wait_for_count(&count, 1, Duration::from_secs(2));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn matching_events_debounce_and_stop_disposes_pending_delivery() {
    let root = root("debounce");
    fs::create_dir_all(&root).expect("root");

    let count = Arc::new(AtomicUsize::new(0));
    let count_for_callback = Arc::clone(&count);
    let watcher = ChromeSessionWatcher::new(
        &root,
        ["Cookies"],
        40,
        Arc::new(move || {
            count_for_callback.fetch_add(1, Ordering::AcqRel);
        }),
    );

    watcher.handle_fs_event(Some("Cookies"));
    watcher.handle_fs_event(Some("Cookies-wal"));
    watcher.handle_fs_event(Some("History"));
    wait_for_count(&count, 1, Duration::from_secs(2));
    thread::sleep(Duration::from_millis(80));
    assert_eq!(count.load(Ordering::Acquire), 1, "debounced events must coalesce");

    watcher.handle_fs_event(Some("Cookies-shm"));
    watcher.stop();
    thread::sleep(Duration::from_millis(80));
    assert_eq!(count.load(Ordering::Acquire), 1, "stop must cancel pending delivery");
    assert!(watcher.is_stopped());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn start_arms_a_real_non_recursive_watch_when_directory_exists() {
    let root = root("real-watch");
    fs::create_dir_all(&root).expect("root");

    let count = Arc::new(AtomicUsize::new(0));
    let count_for_callback = Arc::clone(&count);
    let watcher = ChromeSessionWatcher::new(
        &root,
        ["Cookies"],
        20,
        Arc::new(move || {
            count_for_callback.fetch_add(1, Ordering::AcqRel);
        }),
    );
    watcher.start();
    fs::write(root.join("Cookies"), b"event").expect("write");

    wait_for_count(&count, 1, Duration::from_secs(2));

    watcher.stop();
    let _ = fs::remove_dir_all(root);
}
