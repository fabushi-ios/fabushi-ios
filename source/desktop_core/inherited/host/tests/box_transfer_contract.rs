use std::collections::HashMap;
use std::io;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::thread;
use std::time::Duration;

use mahayana_host_runtime::r#box::box_transfer::{
    BoxFileUnreadableError, TransferBox, TransferEndpoint, download_box_files,
    for_each_bounded, for_each_wave_pipelined, is_box_root_path, is_source_missing_error,
    resolve_box_workspace_path, transfer_file_between_boxes, upload_box_files,
};

#[derive(Default)]
struct MemoryBox {
    files: Mutex<HashMap<String, Vec<u8>>>,
    fail_missing_with_unreadable: bool,
}

impl TransferBox<String> for MemoryBox {
    type Error = io::Error;

    fn download_file(
        &self,
        _ctx: &String,
        _agent_id: &str,
        path: &str,
    ) -> Result<Vec<u8>, Self::Error> {
        self.files
            .lock()
            .expect("memory box files")
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "ENOENT"))
    }

    fn upload_file(
        &self,
        _ctx: &String,
        _agent_id: &str,
        path: &str,
        data: &[u8],
    ) -> Result<(), Self::Error> {
        let _ = self.fail_missing_with_unreadable;
        self.files
            .lock()
            .expect("memory box files")
            .insert(path.into(), data.to_vec());
        Ok(())
    }
}

#[test]
fn box_transfer_paths_and_source_missing_classification_match_grok() {
    assert!(is_box_root_path("/workspace"));
    assert!(is_box_root_path("/workspace/a/b"));
    assert!(is_box_root_path("/home/user"));
    assert!(!is_box_root_path("/workspace-other"));
    assert_eq!(resolve_box_workspace_path("a/../b.txt"), "/workspace/b.txt");
    assert_eq!(resolve_box_workspace_path("/root/./a/../b"), "/root/b");

    let unreadable = BoxFileUnreadableError("missing".into());
    assert!(is_source_missing_error(&unreadable));
    let enoent = io::Error::new(io::ErrorKind::NotFound, "not found");
    assert!(is_source_missing_error(&enoent));
    let message = io::Error::other("No such file or directory");
    assert!(is_source_missing_error(&message));
}

#[test]
fn box_transfer_copies_bytes_enforces_limits_and_deduplicates_batches() {
    let source = MemoryBox::default();
    source
        .files
        .lock()
        .expect("source files")
        .insert("/workspace/a.txt".into(), b"hello".to_vec());
    source
        .files
        .lock()
        .expect("source files")
        .insert("/workspace/b.txt".into(), b"world".to_vec());
    let dest = MemoryBox::default();

    let bytes = transfer_file_between_boxes(
        &"ctx".into(),
        TransferEndpoint {
            box_: &source,
            agent_id: "source-agent",
            path: "/workspace/a.txt",
            label: "source",
        },
        TransferEndpoint {
            box_: &dest,
            agent_id: "dest-agent",
            path: "/workspace/copied.txt",
            label: "dest",
        },
        Some(5),
    )
    .expect("copy five bytes");
    assert_eq!(bytes, 5);
    assert_eq!(
        dest.files
            .lock()
            .expect("dest files")
            .get("/workspace/copied.txt")
            .cloned(),
        Some(b"hello".to_vec())
    );

    let too_large = transfer_file_between_boxes(
        &"ctx".into(),
        TransferEndpoint {
            box_: &source,
            agent_id: "source-agent",
            path: "/workspace/a.txt",
            label: "source",
        },
        TransferEndpoint {
            box_: &dest,
            agent_id: "dest-agent",
            path: "/workspace/too-large.txt",
            label: "dest",
        },
        Some(4),
    )
    .expect_err("transfer must enforce max bytes");
    assert!(too_large.0.contains("over the 4-byte limit"));

    let downloaded = download_box_files(
        &"ctx".into(),
        &source,
        "agent",
        &[
            "/workspace/a.txt".into(),
            "/workspace/a.txt".into(),
            "/workspace/b.txt".into(),
        ],
        Some(2),
    )
    .expect("download unique paths");
    assert_eq!(
        downloaded.iter().map(|(path, _)| path.as_str()).collect::<Vec<_>>(),
        vec!["/workspace/a.txt", "/workspace/b.txt"]
    );

    let uploaded = upload_box_files(
        &"ctx".into(),
        &dest,
        "agent",
        &[
            ("/workspace/u.txt".into(), b"first".to_vec()),
            ("/workspace/u.txt".into(), b"second".to_vec()),
            ("/workspace/v.txt".into(), b"third".to_vec()),
        ],
        Some(2),
    )
    .expect("upload unique paths");
    assert_eq!(uploaded, vec!["/workspace/u.txt", "/workspace/v.txt"]);
    assert_eq!(
        dest.files
            .lock()
            .expect("dest uploads")
            .get("/workspace/u.txt")
            .cloned(),
        Some(b"first".to_vec())
    );
}

#[test]
fn bounded_and_wave_pipelined_helpers_bound_work_and_fail_closed() {
    let active = AtomicUsize::new(0);
    let max_active = AtomicUsize::new(0);
    for_each_bounded(&[1, 2, 3, 4, 5, 6], 2, |_| {
        let current = active.fetch_add(1, Ordering::AcqRel) + 1;
        max_active.fetch_max(current, Ordering::AcqRel);
        thread::sleep(Duration::from_millis(2));
        active.fetch_sub(1, Ordering::AcqRel);
        Ok::<(), &'static str>(())
    })
    .expect("bounded work");
    assert!(max_active.load(Ordering::Acquire) <= 2);
    assert!(max_active.load(Ordering::Acquire) >= 1);

    let prepared = Arc::new(Mutex::new(Vec::<Vec<i32>>::new()));
    let processed = Arc::new(Mutex::new(Vec::<i32>::new()));
    let prepared_sink = Arc::clone(&prepared);
    let processed_sink = Arc::clone(&processed);
    for_each_wave_pipelined(
        &[1, 2, 3, 4, 5],
        2,
        2,
        move |wave| {
            prepared_sink
                .lock()
                .expect("prepared waves")
                .push(wave.to_vec());
            Ok::<Vec<i32>, &'static str>(wave.iter().map(|value| value * 10).collect())
        },
        move |work| {
            processed_sink.lock().expect("processed work").push(work);
            Ok::<(), &'static str>(())
        },
    )
    .expect("wave pipeline");
    assert_eq!(
        prepared.lock().expect("prepared waves").as_slice(),
        &[vec![1, 2], vec![3, 4], vec![5]]
    );
    let mut values = processed.lock().expect("processed work").clone();
    values.sort_unstable();
    assert_eq!(values, vec![10, 20, 30, 40, 50]);

    let error = for_each_bounded(&[1, 2, 3], 2, |value| {
        if *value == 2 {
            Err("stop")
        } else {
            Ok(())
        }
    })
    .expect_err("bounded worker should propagate first error");
    assert_eq!(error, "stop");
}
