use base64::Engine as _;
use mahayana_host_runtime::extensions::local_exec::native_file_sandbox::handle_native_file_request;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

fn sha256_hex(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

fn fixture(label: &str) -> (PathBuf, PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!(
        "fabushi-native-file-{label}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let root = base.join("root");
    let outside = base.join("outside");
    fs::create_dir_all(&root).expect("create root");
    fs::create_dir_all(&outside).expect("create outside");
    (base, root, outside)
}

fn call(root: &Path, request: Value) -> Result<Value, String> {
    handle_native_file_request(root, &request)
}

fn version(root: &Path, path: &str) -> Option<String> {
    call(root, json!({ "op": "version", "path": path, "maxBytes": 1024 * 1024 }))
        .expect("version request")
        .get("version")
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[cfg(unix)]
fn make_directory_redirect(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).expect("create directory symlink");
}

#[cfg(windows)]
fn make_directory_redirect(target: &Path, link: &Path) {
    let status = std::process::Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .status()
        .expect("spawn mklink /J");
    assert!(status.success(), "mklink /J must create a real reparse-point junction");
}

#[test]
fn native_file_owner_rejects_directory_traversal() {
    let (base, root, outside) = fixture("traversal");
    fs::write(outside.join("secret.txt"), b"secret").expect("write outside secret");
    let error = call(
        &root,
        json!({ "op": "read", "path": "../outside/secret.txt", "maxBytes": 1024 }),
    )
    .expect_err("parent traversal must fail closed");
    assert!(error.contains("traversal") || error.contains("relative"));
    let _ = fs::remove_dir_all(base);
}

#[cfg(any(unix, windows))]
#[test]
fn native_file_owner_rejects_symlink_or_reparse_swap_between_chunks_and_commit() {
    let (base, root, outside) = fixture("swap");
    let live = root.join("slot");
    fs::create_dir_all(&live).expect("create live directory");
    let payload = b"owned-by-authorized-root";
    let transfer_id = "swap-transfer";
    let expected = sha256_hex(payload);

    let started = call(
        &root,
        json!({
            "op": "upload-start",
            "path": "slot/result.bin",
            "transferId": transfer_id,
            "totalBytes": payload.len(),
            "maxBytes": 1024 * 1024,
            "headroomBytes": 0,
            "overwrite": true
        }),
    )
    .expect("start upload");
    assert!(started.get("startingVersion").is_some());

    call(
        &root,
        json!({
            "op": "upload-chunk",
            "path": "slot/result.bin",
            "transferId": transfer_id,
            "offset": 0,
            "bytesBase64": base64::engine::general_purpose::STANDARD.encode(payload)
        }),
    )
    .expect("write chunk");

    fs::rename(&live, root.join("slot-original")).expect("rename authorized parent");
    make_directory_redirect(&outside, &live);

    let error = call(
        &root,
        json!({
            "op": "upload-commit",
            "path": "slot/result.bin",
            "transferId": transfer_id,
            "totalBytes": payload.len(),
            "maxBytes": 1024 * 1024,
            "sha256": expected,
            "overwrite": true
        }),
    )
    .expect_err("swapped parent must fail closed");
    assert!(
        error.contains("symlink")
            || error.contains("reparse")
            || error.contains("path")
            || error.contains("missing")
            || error.contains("Not a directory")
    );
    assert!(!outside.join("result.bin").exists());
    let _ = fs::remove_dir_all(base);
}

#[test]
fn native_file_owner_detects_version_conflict_at_chunk_commit() {
    use base64::Engine as _;
    let (base, root, _) = fixture("version");
    fs::create_dir_all(root.join("files")).expect("create files");
    fs::write(root.join("files/item.txt"), b"old").expect("write initial");
    let starting = version(&root, "files/item.txt").expect("starting version");
    let payload = b"replacement";
    let transfer_id = "version-transfer";

    let started = call(
        &root,
        json!({
            "op": "upload-start",
            "path": "files/item.txt",
            "transferId": transfer_id,
            "totalBytes": payload.len(),
            "maxBytes": 1024 * 1024,
            "headroomBytes": 0,
            "expectedVersion": starting,
            "overwrite": true
        }),
    )
    .expect("start versioned upload");
    let starting_version = started
        .get("startingVersion")
        .and_then(Value::as_str)
        .expect("starting version returned")
        .to_string();

    call(
        &root,
        json!({
            "op": "upload-chunk",
            "path": "files/item.txt",
            "transferId": transfer_id,
            "offset": 0,
            "bytesBase64": base64::engine::general_purpose::STANDARD.encode(payload)
        }),
    )
    .expect("write versioned chunk");

    fs::write(root.join("files/item.txt"), b"changed-concurrently").expect("mutate target");

    let error = call(
        &root,
        json!({
            "op": "upload-commit",
            "path": "files/item.txt",
            "transferId": transfer_id,
            "totalBytes": payload.len(),
            "maxBytes": 1024 * 1024,
            "sha256": sha256_hex(payload),
            "startingVersion": starting_version,
            "overwrite": true
        }),
    )
    .expect_err("version change must reject commit");
    assert!(error.contains("version changed"));
    assert_eq!(fs::read(root.join("files/item.txt")).unwrap(), b"changed-concurrently");
    let _ = fs::remove_dir_all(base);
}

#[test]
fn native_file_owner_rejects_chunk_hash_mismatch() {
    use base64::Engine as _;
    let (base, root, _) = fixture("hash");
    fs::create_dir_all(root.join("files")).expect("create files");
    let payload = b"payload";
    let transfer_id = "hash-transfer";

    call(
        &root,
        json!({
            "op": "upload-start",
            "path": "files/item.txt",
            "transferId": transfer_id,
            "totalBytes": payload.len(),
            "maxBytes": 1024 * 1024,
            "headroomBytes": 0,
            "overwrite": true
        }),
    )
    .expect("start hash upload");
    call(
        &root,
        json!({
            "op": "upload-chunk",
            "path": "files/item.txt",
            "transferId": transfer_id,
            "offset": 0,
            "bytesBase64": base64::engine::general_purpose::STANDARD.encode(payload)
        }),
    )
    .expect("write hash chunk");
    let error = call(
        &root,
        json!({
            "op": "upload-commit",
            "path": "files/item.txt",
            "transferId": transfer_id,
            "totalBytes": payload.len(),
            "maxBytes": 1024 * 1024,
            "sha256": "00".repeat(32),
            "overwrite": true
        }),
    )
    .expect_err("wrong digest must fail");
    assert!(error.contains("SHA-256"));
    assert!(!root.join("files/item.txt").exists());
    let _ = call(
        &root,
        json!({ "op": "upload-abort", "path": "files/item.txt", "transferId": transfer_id }),
    );
    let _ = fs::remove_dir_all(base);
}

#[test]
fn native_file_owner_uses_real_free_space_and_rejects_impossible_peak_requirement() {
    let (base, root, _) = fixture("enospc");
    fs::create_dir_all(root.join("files")).expect("create files");
    let error = call(
        &root,
        json!({
            "op": "upload-start",
            "path": "files/huge.bin",
            "transferId": "enospc-transfer",
            "totalBytes": u64::MAX / 2,
            "maxBytes": u64::MAX / 2,
            "headroomBytes": 0,
            "overwrite": true
        }),
    )
    .expect_err("real filesystem cannot satisfy an exabyte-scale atomic write");
    assert!(error.contains("disk space") || error.contains("Storage") || error.contains("space"));
    assert!(!root.join("files/huge.bin").exists());
    let _ = fs::remove_dir_all(base);
}

#[test]
fn native_file_owner_commits_atomically_and_reads_the_committed_hash() {
    use base64::Engine as _;
    let (base, root, _) = fixture("atomic");
    fs::create_dir_all(root.join("files")).expect("create files");
    let payload = b"atomic-result";
    let digest = sha256_hex(payload);

    let write = call(
        &root,
        json!({
            "op": "write-atomic",
            "path": "files/result.bin",
            "bytesBase64": base64::engine::general_purpose::STANDARD.encode(payload),
            "sha256": digest,
            "maxBytes": 1024 * 1024,
            "headroomBytes": 0,
            "overwrite": false
        }),
    )
    .expect("atomic write");
    assert_eq!(write.get("sha256").and_then(Value::as_str), Some(digest.as_str()));
    assert_eq!(fs::read(root.join("files/result.bin")).unwrap(), payload);

    let read = call(
        &root,
        json!({ "op": "read", "path": "files/result.bin", "maxBytes": 1024 * 1024 }),
    )
    .expect("native read");
    assert_eq!(read.get("sha256").and_then(Value::as_str), Some(digest.as_str()));
    assert_eq!(
        read.get("bytesBase64").and_then(Value::as_str),
        Some(base64::engine::general_purpose::STANDARD.encode(payload).as_str())
    );
    let _ = fs::remove_dir_all(base);
}
