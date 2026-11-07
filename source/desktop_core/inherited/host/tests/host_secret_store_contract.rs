use std::fs;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};

static TEST_LOCK: Mutex<()> = Mutex::new(());

use mahayana_host_runtime::host_secret_store::{
    clear_host_machine_id_cache_for_tests, get_or_create_host_machine_id, read_machine_id,
    clear_host_node_token, load_or_create_host_node_identity, read_host_node_token,
    write_host_node_token, write_machine_id,
};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("fabushi-{label}-{suffix}"))
}

#[test]
fn host_secret_store_reads_writes_and_process_caches_machine_id() {
    let _guard = TEST_LOCK.lock().expect("test lock");
    clear_host_machine_id_cache_for_tests();
    let root = temp_root("host-secret-store");
    let path = root.join("nested").join("host-secrets.json");

    assert_eq!(read_machine_id(&path), None);
    write_machine_id(&path, "machine-existing").expect("write existing id");
    assert_eq!(read_machine_id(&path).as_deref(), Some("machine-existing"));

    let first = get_or_create_host_machine_id(Some(&path)).expect("read existing id");
    assert_eq!(first, "machine-existing");

    write_machine_id(&path, "machine-changed-on-disk").expect("replace id on disk");
    let cached = get_or_create_host_machine_id(Some(&path)).expect("cached id");
    assert_eq!(cached, "machine-existing");

    clear_host_machine_id_cache_for_tests();
    let reloaded = get_or_create_host_machine_id(Some(&path)).expect("reload id");
    assert_eq!(reloaded, "machine-changed-on-disk");

    fs::remove_dir_all(root).expect("remove test root");
    clear_host_machine_id_cache_for_tests();
}

#[test]
fn host_secret_store_creates_uuid_and_ignores_invalid_json() {
    let _guard = TEST_LOCK.lock().expect("test lock");
    clear_host_machine_id_cache_for_tests();
    let root = temp_root("host-secret-create");
    fs::create_dir_all(&root).expect("create root");
    let path = root.join("host-secrets.json");
    fs::write(&path, "{not-json").expect("write invalid file");
    assert_eq!(read_machine_id(&path), None);

    let created = get_or_create_host_machine_id(Some(&path)).expect("create machine id");
    assert!(!created.is_empty());
    assert_eq!(read_machine_id(&path).as_deref(), Some(created.as_str()));

    fs::remove_dir_all(root).expect("remove test root");
    clear_host_machine_id_cache_for_tests();
}

#[test]
fn node_identity_is_persisted_and_signs_without_exposing_private_bytes() {
    let _guard = TEST_LOCK.lock().expect("test lock");
    let root = temp_root("node-identity");
    let path = root.join("node-identity.json");

    let first = load_or_create_host_node_identity(Some(&path)).expect("create node identity");
    let second = load_or_create_host_node_identity(Some(&path)).expect("reload node identity");
    assert_eq!(first.device_id(), second.device_id());
    assert_eq!(
        first.public_key_raw_base64_url(),
        second.public_key_raw_base64_url()
    );
    let public_key_bytes: [u8; 32] = URL_SAFE_NO_PAD
        .decode(first.public_key_raw_base64_url())
        .expect("decode public key")
        .try_into()
        .expect("public key length");
    let public_key = VerifyingKey::from_bytes(&public_key_bytes).expect("public key");
    let signature_bytes: [u8; 64] = URL_SAFE_NO_PAD
        .decode(first.sign_payload_base64_url("node-challenge"))
        .expect("decode signature")
        .try_into()
        .expect("signature length");
    public_key
        .verify(
            b"node-challenge",
            &Signature::from_bytes(&signature_bytes),
        )
        .expect("signature verifies");
    assert!(!fs::read_to_string(&path)
        .expect("read persisted identity")
        .contains("signingKey"));

    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn node_tokens_are_origin_scoped_and_rotation_fenced() {
    let _guard = TEST_LOCK.lock().expect("test lock");
    let root = temp_root("node-auth");
    let path = root.join("node-auth.json");
    let scopes = vec!["node.invoke".to_string(), "node.read".to_string()];

    assert!(write_host_node_token(
        Some(&path),
        "device-1",
        " node ",
        Some("wss://gateway.example:443"),
        "token-a",
        &scopes,
        None,
    )
    .expect("write token"));
    let stored = read_host_node_token(
        Some(&path),
        "device-1",
        "node",
        Some("wss://gateway.example:443"),
    )
    .expect("read token")
    .expect("stored token");
    assert_eq!(stored.secret(), "token-a");
    assert_eq!(
        stored.scopes().iter().map(String::as_str).collect::<Vec<_>>(),
        vec!["node.invoke", "node.read"]
    );
    assert!(read_host_node_token(Some(&path), "device-1", "node", None)
        .expect("read unscoped token")
        .is_none());
    assert!(!write_host_node_token(
        Some(&path),
        "device-1",
        "node",
        Some("wss://gateway.example:443"),
        "token-b",
        &[],
        Some("stale-token"),
    )
    .expect("reject stale rotation"));
    assert!(write_host_node_token(
        Some(&path),
        "device-1",
        "node",
        Some("wss://gateway.example:443"),
        "token-b",
        &[],
        Some("token-a"),
    )
    .expect("rotate token"));
    assert!(!clear_host_node_token(
        Some(&path),
        "device-1",
        "node",
        Some("wss://gateway.example:443"),
        Some("token-a"),
    )
    .expect("reject stale clear"));
    assert!(clear_host_node_token(
        Some(&path),
        "device-1",
        "node",
        Some("wss://gateway.example:443"),
        Some("token-b"),
    )
    .expect("clear token"));

    fs::remove_dir_all(root).expect("remove test root");
}
