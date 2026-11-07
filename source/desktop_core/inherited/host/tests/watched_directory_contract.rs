use std::fs;
use std::sync::{Arc, mpsc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::watched_directory::WatchedDirectory;

fn root(label: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-watched-directory-{label}-{}-{nonce}",
        std::process::id()
    ))
}

#[test]
fn directory_lists_only_subdirectories_in_sorted_order() {
    let root = root("list");
    fs::create_dir_all(root.join("zeta")).unwrap();
    fs::create_dir_all(root.join("alpha")).unwrap();
    fs::write(root.join("file.txt"), b"ignored").unwrap();

    let watched = WatchedDirectory::new(&root, 10);
    assert_eq!(
        watched.list_subdirectory_names(),
        vec!["alpha".to_string(), "zeta".to_string()]
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn atomic_and_external_writes_notify_through_the_same_debounced_owner() {
    let root = root("notify");
    let watched = WatchedDirectory::new(&root, 25);
    let (tx, rx) = mpsc::channel();
    watched
        .set_on_change(Some(Arc::new(move || {
            let _ = tx.send(());
        })))
        .expect("watch");

    watched
        .write_file_atomic(root.join("nested").join("state.txt"), b"one")
        .expect("atomic write");
    rx.recv_timeout(Duration::from_secs(3))
        .expect("atomic write notification");

    while rx.try_recv().is_ok() {}
    fs::write(root.join("nested").join("state.txt"), b"two").expect("external write");
    rx.recv_timeout(Duration::from_secs(3))
        .expect("external change notification");

    watched.set_on_change(None).expect("stop watch");
    let _ = fs::remove_dir_all(root);
}
