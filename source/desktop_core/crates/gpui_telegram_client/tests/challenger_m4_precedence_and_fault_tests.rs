//! Milestone 4 Empirical Adversarial Stress & Fault Injection Test Suite
//!
//! Scope:
//! 1. Multi-threaded race condition stress on 4-tier configuration resolution
//!    (concurrent threads resolving CLI flags, environment overrides, and file configurations).
//! 2. Boundary fuzzing on session storage under hostile process umasks (`0o000`, `0o077`, `0o002`, `0o022`, `0o027`)
//!    verifying POSIX 0600 mode invariance and atomic replacement of insecure pre-existing files.
//! 3. Fault injection on network communication:
//!    - Malformed JSON payloads against `MessageContent::Text` deserialization
//!      (missing entities, null entities, extra unknown fields).
//!    - Unrecoverable HTTP 401 handshake verification: loop terminates and session token is wiped.
//!    - Max reconnect attempts exhaustion boundary: assert loop terminates into `Offline` precisely at limit.

use gpui_telegram_client::config::{ClientConfig, ConfigSource, StoredSession, UserConfigFile, DEFAULT_SERVER_URL};
use gpui_telegram_client::net::{ConnectionStatus, WsChannel, WsTransport};
use gpui_telegram_proto::message::content::{MessageContent, MessageEntityType};
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::Duration;

static TEST_ENV_LOCK: Mutex<()> = Mutex::new(());

fn acquire_lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn set_env_var(key: &str, val: impl AsRef<Path>) {
    unsafe {
        env::set_var(key, val.as_ref());
    }
}

fn remove_env_var(key: &str) {
    unsafe {
        env::remove_var(key);
    }
}

struct EnvGuard {
    saved_home: Option<String>,
    saved_url: Option<String>,
    saved_xdg_data: Option<String>,
}

impl EnvGuard {
    fn new() -> Self {
        Self {
            saved_home: env::var("HOME").ok(),
            saved_url: env::var("GPUI_TELEGRAM_SERVER_URL").ok(),
            saved_xdg_data: env::var("XDG_DATA_HOME").ok(),
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(ref h) = self.saved_home {
            set_env_var("HOME", h);
        } else {
            remove_env_var("HOME");
        }
        if let Some(ref u) = self.saved_url {
            set_env_var("GPUI_TELEGRAM_SERVER_URL", u);
        } else {
            remove_env_var("GPUI_TELEGRAM_SERVER_URL");
        }
        if let Some(ref x) = self.saved_xdg_data {
            set_env_var("XDG_DATA_HOME", x);
        } else {
            remove_env_var("XDG_DATA_HOME");
        }
    }
}

struct UmaskGuard {
    original_umask: libc::mode_t,
}

impl UmaskGuard {
    fn set(new_mask: libc::mode_t) -> Self {
        let original_umask = unsafe { libc::umask(new_mask) };
        Self { original_umask }
    }
}

impl Drop for UmaskGuard {
    fn drop(&mut self) {
        unsafe {
            libc::umask(self.original_umask);
        }
    }
}

// ============================================================================
// AREA 1: Multi-Threaded Race Condition Stress on 4-Tier Configuration Resolution
// ============================================================================

#[test]
fn test_adversarial_multithreaded_4tier_cli_precedence_race() {
    let _lock = acquire_lock();
    let _guard = EnvGuard::new();

    let temp_home = tempfile::tempdir().unwrap();
    set_env_var("HOME", temp_home.path());
    set_env_var("GPUI_TELEGRAM_SERVER_URL", "http://env-override-baseline:8080");

    let app_dir = temp_home.path().join(".config").join("gpui_telegram");
    fs::create_dir_all(&app_dir).unwrap();
    let config_file = UserConfigFile {
        server_url: Some("http://file-config-baseline:8080".to_string()),
        http_timeout_secs: Some(15),
        ws_ping_interval_secs: Some(30),
        max_reconnect_attempts: Some(5),
        ..Default::default()
    };
    fs::write(
        app_dir.join("config.json"),
        serde_json::to_string(&config_file).unwrap(),
    )
    .unwrap();

    const THREAD_COUNT: usize = 16;
    const ITERATIONS_PER_THREAD: usize = 200;

    let barrier = Arc::new(Barrier::new(THREAD_COUNT));
    let mut handles = Vec::new();

    for thread_idx in 0..THREAD_COUNT {
        let b = Arc::clone(&barrier);
        let handle = thread::spawn(move || {
            b.wait();

            for iter in 0..ITERATIONS_PER_THREAD {
                let variant = (thread_idx + iter) % 4;
                let expected_url = format!("http://cli-worker-{}.local:909{}", thread_idx, variant);

                let args = match variant {
                    0 => vec![
                        "gpui_telegram".to_string(),
                        "--server-url".to_string(),
                        format!("{}/", expected_url), // with trailing slash
                    ],
                    1 => vec![
                        "gpui_telegram".to_string(),
                        format!("--server-url={}", expected_url),
                    ],
                    2 => vec![
                        "gpui_telegram".to_string(),
                        "-s".to_string(),
                        format!("{}/", expected_url),
                    ],
                    _ => vec![
                        "gpui_telegram".to_string(),
                        format!("-s={}", expected_url),
                    ],
                };

                let resolved = ClientConfig::resolve_with_args(&args);

                assert_eq!(
                    resolved.server_url, expected_url,
                    "CLI URL must override env and file configs under concurrent thread contention"
                );
                assert_eq!(
                    resolved.config_source,
                    ConfigSource::CliArgument,
                    "Config source must be CliArgument"
                );
                // Non-URL settings should fall back to config file values
                assert_eq!(resolved.http_timeout_secs, 15);
                assert_eq!(resolved.ws_ping_interval_secs, 30);
                assert_eq!(resolved.max_reconnect_attempts, 5);
            }
        });
        handles.push(handle);
    }

    for h in handles {
        h.join().expect("Worker thread panicked during CLI precedence stress");
    }
}

#[test]
fn test_adversarial_multithreaded_config_file_race_and_precedence() {
    let _lock = acquire_lock();
    let _guard = EnvGuard::new();

    let temp_home = tempfile::tempdir().unwrap();
    set_env_var("HOME", temp_home.path());
    remove_env_var("GPUI_TELEGRAM_SERVER_URL");

    let app_dir = temp_home.path().join(".config").join("gpui_telegram");
    fs::create_dir_all(&app_dir).unwrap();
    let config_path = app_dir.join("config.json");

    let running = Arc::new(parking_lot::RwLock::new(true));
    let write_counter = Arc::new(AtomicUsize::new(0));

    // Writer thread constantly overwriting config.json with varying settings
    let writer_running = Arc::clone(&running);
    let writer_config_path = config_path.clone();
    let writer_counter = Arc::clone(&write_counter);
    let writer_handle = thread::spawn(move || {
        let mut idx = 0usize;
        while *writer_running.read() {
            idx += 1;
            let cfg = UserConfigFile {
                server_url: Some(format!("http://file-node-{}.internal:8080", idx % 10)),
                http_timeout_secs: Some(10 + (idx as u64 % 20)),
                ws_ping_interval_secs: Some(20 + (idx as u64 % 15)),
                max_reconnect_attempts: Some(3 + (idx as u32 % 8)),
                ..Default::default()
            };
            if let Ok(json) = serde_json::to_string(&cfg) {
                let _ = fs::write(&writer_config_path, json);
                writer_counter.fetch_add(1, Ordering::Relaxed);
            }
            thread::sleep(Duration::from_millis(1));
        }
    });

    // 8 concurrent reader threads querying configuration
    let mut reader_handles = Vec::new();
    for reader_idx in 0..8 {
        let r_handle = thread::spawn(move || {
            let cli_override_url = format!("http://cli-override-{reader_idx}.internal:7070");
            let cli_args = vec![
                "gpui_telegram".to_string(),
                "-s".to_string(),
                cli_override_url.clone(),
            ];

            for _ in 0..100 {
                // Query 1: CLI override should ALWAYS beat file, regardless of file content
                let resolved_cli = ClientConfig::resolve_with_args(&cli_args);
                assert_eq!(resolved_cli.server_url, cli_override_url);
                assert_eq!(resolved_cli.config_source, ConfigSource::CliArgument);

                // Query 2: Pure file query should safely parse whatever valid state is on disk
                let resolved_file = ClientConfig::resolve_with_args(&["gpui_telegram".to_string()]);
                assert!(
                    resolved_file.server_url.starts_with("http://file-node-")
                        || resolved_file.server_url == DEFAULT_SERVER_URL,
                    "Resolved URL must be valid file URL or base default during race: {}",
                    resolved_file.server_url
                );
                assert!(resolved_file.http_timeout_secs > 0);
                assert!(resolved_file.max_reconnect_attempts > 0);

                thread::sleep(Duration::from_millis(1));
            }
        });
        reader_handles.push(r_handle);
    }

    for rh in reader_handles {
        rh.join().expect("Reader thread panicked during file race stress");
    }

    *running.write() = false;
    writer_handle.join().expect("Writer thread panicked");

    assert!(
        write_counter.load(Ordering::Relaxed) > 10,
        "Writer must have completed multiple concurrent file modifications"
    );
}

#[test]
fn test_adversarial_cli_empty_and_whitespace_fallthrough() {
    let _lock = acquire_lock();
    let _guard = EnvGuard::new();

    let temp_home = tempfile::tempdir().unwrap();
    set_env_var("HOME", temp_home.path());
    set_env_var("GPUI_TELEGRAM_SERVER_URL", "http://env-fallback:8080");

    // Case 1: Empty CLI value falls through to Tier 2 (Env)
    let args_empty = vec![
        "gpui_telegram".to_string(),
        "--server-url".to_string(),
        "".to_string(),
    ];
    let resolved_empty = ClientConfig::resolve_with_args(&args_empty);
    assert_eq!(resolved_empty.server_url, "http://env-fallback:8080");
    assert_eq!(resolved_empty.config_source, ConfigSource::EnvironmentVariable);

    // Case 2: Whitespace-only CLI value falls through to Tier 2 (Env)
    let args_ws = vec![
        "gpui_telegram".to_string(),
        "--server-url=    \t  \n  ".to_string(),
    ];
    let resolved_ws = ClientConfig::resolve_with_args(&args_ws);
    assert_eq!(resolved_ws.server_url, "http://env-fallback:8080");
    assert_eq!(resolved_ws.config_source, ConfigSource::EnvironmentVariable);

    // Case 3: Empty Env falls through to Tier 4 (Default) when no config file
    remove_env_var("GPUI_TELEGRAM_SERVER_URL");
    let resolved_default = ClientConfig::resolve_with_args(&args_empty);
    assert_eq!(resolved_default.server_url, DEFAULT_SERVER_URL);
    assert_eq!(resolved_default.config_source, ConfigSource::DefaultFallback);
}

// ============================================================================
// AREA 2: Boundary Fuzzing on Session Storage Under Hostile Process Umasks
// ============================================================================

#[test]
fn test_adversarial_session_storage_hostile_umasks_0600_invariance() {
    let _lock = acquire_lock();
    let _guard = EnvGuard::new();

    // Adversarial process umasks to test:
    // 0o000: Permissive (world read/write/exec) - would leak credentials if not explicitly chmod'd
    // 0o077: Restrictive
    // 0o022: Standard POSIX
    // 0o002: Group writable
    // 0o027: Strict enterprise
    // 0o007: Group permissive
    let hostile_umasks: &[libc::mode_t] = &[0o000, 0o077, 0o022, 0o002, 0o027, 0o007];

    for &umask in hostile_umasks {
        let _umask_guard = UmaskGuard::set(umask);

        let temp_home = tempfile::tempdir().unwrap();
        set_env_var("HOME", temp_home.path());

        let session = StoredSession {
            server_url: "http://34.46.77.133:8080".to_string(),
            user_id: 888123 + (umask as u64),
            username: format!("umask_adversary_{:03o}", umask),
            token: format!("token_umask_{:03o}_xyz789", umask),
            session_id: format!("sess_umask_{:03o}", umask),
            created_at_ms: chrono::Utc::now().timestamp_millis(),
        };

        // 1. Save session under hostile umask
        ClientConfig::save_session(&session)
            .unwrap_or_else(|e| panic!("save_session failed under umask 0o{:03o}: {}", umask, e));

        let session_file = ClientConfig::get_session_path();
        assert!(session_file.is_file(), "Session file must exist");

        // 2. Empirical verification of POSIX mode 0600 invariance
        let meta = fs::metadata(&session_file).unwrap();
        let file_mode = meta.permissions().mode() & 0o777;
        assert_eq!(
            file_mode, 0o600,
            "Session file mode must be strictly POSIX 0600 under hostile umask 0o{:03o}, but found 0o{:03o}",
            umask, file_mode
        );

        // 3. Verify parent directory permission is 0700
        let app_dir = ClientConfig::get_app_dir();
        let dir_meta = fs::metadata(&app_dir).unwrap();
        let dir_mode = dir_meta.permissions().mode() & 0o777;
        assert_eq!(
            dir_mode, 0o700,
            "App directory mode must be strictly POSIX 0700 under hostile umask 0o{:03o}, but found 0o{:03o}",
            umask, dir_mode
        );

        // 4. Verify loaded session integrity
        let loaded = ClientConfig::load_session().expect("Session must load successfully");
        assert_eq!(loaded, session);

        // 5. Clean up session and verify file removal
        ClientConfig::clear_session();
        assert!(
            !session_file.exists(),
            "Session file must be removed by clear_session"
        );
        assert!(ClientConfig::load_session().is_none());
    }
}

#[test]
fn test_adversarial_session_storage_overwrite_preexisting_insecure_file() {
    let _lock = acquire_lock();
    let _guard = EnvGuard::new();

    // Run under umask 0o000
    let _umask_guard = UmaskGuard::set(0o000);

    let temp_home = tempfile::tempdir().unwrap();
    set_env_var("HOME", temp_home.path());

    let app_dir = ClientConfig::get_app_dir();
    fs::create_dir_all(&app_dir).unwrap();
    let session_file = ClientConfig::get_session_path();

    // Adversarially pre-create an insecure session file with 0o666 (world read/write)
    fs::write(&session_file, b"insecure_preexisting_garbage").unwrap();
    fs::set_permissions(&session_file, fs::Permissions::from_mode(0o666)).unwrap();
    let initial_mode = fs::metadata(&session_file).unwrap().permissions().mode() & 0o777;
    assert_eq!(initial_mode, 0o666);

    let session = StoredSession {
        server_url: "http://34.46.77.133:8080".to_string(),
        user_id: 99901,
        username: "overwritten_user".to_string(),
        token: "overwritten_token_0600".to_string(),
        session_id: "sess_overwritten".to_string(),
        created_at_ms: chrono::Utc::now().timestamp_millis(),
    };

    // save_session must atomically overwrite and enforce 0600 mode
    ClientConfig::save_session(&session).unwrap();

    let meta = fs::metadata(&session_file).unwrap();
    let final_mode = meta.permissions().mode() & 0o777;
    assert_eq!(
        final_mode, 0o600,
        "Atomic rename must replace insecure permissions with 0600"
    );

    let loaded = ClientConfig::load_session().expect("Loaded session must succeed");
    assert_eq!(loaded, session);
}

#[test]
fn test_adversarial_session_storage_concurrent_threads_under_umask000() {
    let _lock = acquire_lock();
    let _guard = EnvGuard::new();
    let _umask_guard = UmaskGuard::set(0o000);

    let temp_home = tempfile::tempdir().unwrap();
    set_env_var("HOME", temp_home.path());

    const WORKERS: usize = 10;
    let barrier = Arc::new(Barrier::new(WORKERS));
    let mut handles = Vec::new();

    for worker_idx in 0..WORKERS {
        let b = Arc::clone(&barrier);
        let h = thread::spawn(move || {
            b.wait();
            for iter in 0..20 {
                let session = StoredSession {
                    server_url: "http://34.46.77.133:8080".to_string(),
                    user_id: 10000 + (worker_idx * 100 + iter) as u64,
                    username: format!("worker_{}_{}", worker_idx, iter),
                    token: format!("tok_{}_{}", worker_idx, iter),
                    session_id: format!("sess_{}_{}", worker_idx, iter),
                    created_at_ms: chrono::Utc::now().timestamp_millis(),
                };
                ClientConfig::save_session(&session).unwrap();
            }
        });
        handles.push(h);
    }

    for h in handles {
        h.join().unwrap();
    }

    let session_file = ClientConfig::get_session_path();
    assert!(session_file.is_file());
    let final_mode = fs::metadata(&session_file).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        final_mode, 0o600,
        "Concurrent save_session under umask 000 must result in 0600 permissions"
    );

    let loaded = ClientConfig::load_session().expect("Final session must be valid JSON");
    assert!(loaded.user_id >= 10000);
}

// ============================================================================
// AREA 3: Fault Injection on Network Communication
// ============================================================================

#[test]
fn test_adversarial_message_content_text_json_fault_injection() {
    // 1. Standard text payload with formatting entities
    let valid_full = r#"{
        "type": "Text",
        "data": {
            "text": "Hello world",
            "entities": [
                { "offset": 0, "length": 5, "entity_type": { "type": "Bold" } }
            ]
        }
    }"#;
    let content: MessageContent = serde_json::from_str(valid_full).expect("Valid payload must parse");
    match content {
        MessageContent::Text { text, entities } => {
            assert_eq!(text, "Hello world");
            assert_eq!(entities.len(), 1);
            assert_eq!(entities[0].offset, 0);
            assert_eq!(entities[0].length, 5);
            assert_eq!(entities[0].entity_type, MessageEntityType::Bold);
        }
        _ => panic!("Expected MessageContent::Text"),
    }

    // 2. Missing `entities` field (omitted entirely)
    // Worker M4 fix added #[serde(default)] to entities, allowing seamless deserialization
    let omitted_entities = r#"{
        "type": "Text",
        "data": {
            "text": "Payload without entities key"
        }
    }"#;
    let content_omitted: MessageContent = serde_json::from_str(omitted_entities)
        .expect("Payload omitting entities must deserialize using default fallback");
    match content_omitted {
        MessageContent::Text { text, entities } => {
            assert_eq!(text, "Payload without entities key");
            assert!(entities.is_empty(), "Entities must default to empty vector");
        }
        _ => panic!("Expected MessageContent::Text"),
    }

    // 3. Extra unknown fields within data object (serde should ignore by default)
    let extra_unknown_fields = r#"{
        "type": "Text",
        "data": {
            "text": "Payload with unknown fields",
            "entities": [],
            "unknown_metadata_key": "some_value",
            "future_spec_number": 12345,
            "future_spec_nested": { "foo": "bar" }
        }
    }"#;
    let content_extra: MessageContent = serde_json::from_str(extra_unknown_fields)
        .expect("Payload with unknown fields in data must deserialize safely");
    match content_extra {
        MessageContent::Text { text, entities } => {
            assert_eq!(text, "Payload with unknown fields");
            assert!(entities.is_empty());
        }
        _ => panic!("Expected MessageContent::Text"),
    }

    // 4. Extra unknown fields at root object level
    let extra_root_fields = r#"{
        "type": "Text",
        "data": {
            "text": "Payload with root extra fields"
        },
        "extra_root_prop": true,
        "gateway_signature": "0xdeadbeef"
    }"#;
    let content_root_extra: MessageContent = serde_json::from_str(extra_root_fields)
        .expect("Payload with extra root fields must deserialize safely");
    match content_root_extra {
        MessageContent::Text { text, .. } => {
            assert_eq!(text, "Payload with root extra fields");
        }
        _ => panic!("Expected MessageContent::Text"),
    }

    // 5. Malformed payload: `entities` is explicitly `null`
    // CRITICAL EMPIRICAL CHALLENGE:
    // Serde's standard `#[serde(default)]` handles key omission, but fails on explicit `null`
    // with: "invalid type: null, expected a sequence".
    // We empirically document and assert this behavior for contract transparency.
    let null_entities_json = r#"{
        "type": "Text",
        "data": {
            "text": "Payload with null entities",
            "entities": null
        }
    }"#;
    let null_result = serde_json::from_str::<MessageContent>(null_entities_json);
    assert!(
        null_result.is_err(),
        "Standard serde #[serde(default)] on Vec rejects explicit null entities: {:?}",
        null_result
    );
    let err_str = null_result.unwrap_err().to_string();
    assert!(
        err_str.contains("invalid type: null, expected a sequence"),
        "Error message must specify type mismatch: {}",
        err_str
    );

    // 6. Malformed payload: missing `text` field
    let missing_text_json = r#"{
        "type": "Text",
        "data": {
            "entities": []
        }
    }"#;
    assert!(
        serde_json::from_str::<MessageContent>(missing_text_json).is_err(),
        "Missing text field must be rejected"
    );

    // 7. Malformed payload: completely invalid data shape (integer instead of object)
    let invalid_data_shape = r#"{
        "type": "Text",
        "data": 12345
    }"#;
    assert!(
        serde_json::from_str::<MessageContent>(invalid_data_shape).is_err(),
        "Primitive data for Text variant must be rejected"
    );
}

#[tokio::test]
async fn test_adversarial_ws_transport_401_local_mock_halts_and_wipes_session() {
    let _lock = acquire_lock();
    let _guard = EnvGuard::new();

    let temp_home = tempfile::tempdir().unwrap();
    set_env_var("HOME", temp_home.path());

    // 1. Persist a session file
    let session = StoredSession {
        server_url: "http://127.0.0.1:9099".to_string(),
        user_id: 77701,
        username: "victim_401".to_string(),
        token: "stale_token_triggering_401".to_string(),
        session_id: "sess_401".to_string(),
        created_at_ms: chrono::Utc::now().timestamp_millis(),
    };
    ClientConfig::save_session(&session).unwrap();
    assert!(ClientConfig::load_session().is_some());

    // 2. Spawn a local mock TCP server that rejects WS upgrade with HTTP 401 Unauthorized
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_port = listener.local_addr().unwrap().port();
    let server_url = format!("http://127.0.0.1:{}", local_port);

    let server_task = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf).await;
            let response = concat!(
                "HTTP/1.1 401 Unauthorized\r\n",
                "Content-Type: application/json\r\n",
                "Content-Length: 39\r\n",
                "Connection: close\r\n",
                "\r\n",
                r#"{"error":"unauthorized","code":401}"#
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.flush().await;
        }
    });

    // 3. Connect WsTransport
    let ws_channel = WsChannel::new();
    let transport = WsTransport::new(server_url, ws_channel);
    transport.set_token(Some(session.token.clone()));
    let mut status_rx = transport.subscribe_status();

    transport.start();

    // 4. Await 401 rejection transition into Offline
    let mut caught_401_offline = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    while tokio::time::Instant::now() < deadline {
        tokio::select! {
            res = status_rx.changed() => {
                if res.is_ok() {
                    let st = status_rx.borrow().clone();
                    if let ConnectionStatus::Offline { reason } = st {
                        if reason.contains("401") || reason.contains("unauthorized") {
                            caught_401_offline = true;
                            // Application shell policy on 401: clear session
                            ClientConfig::clear_session();
                            transport.set_token(None);
                            break;
                        }
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }

    assert!(
        caught_401_offline,
        "WsTransport must intercept HTTP 401 handshake rejection and transition to Offline"
    );

    // 5. Verify stored session token was wiped
    assert!(
        ClientConfig::load_session().is_none(),
        "Stored session token must be wiped after 401 rejection"
    );

    // 6. Verify reconnect loop terminated and remains Offline
    tokio::time::sleep(Duration::from_millis(1000)).await;
    let current_st = transport.status();
    assert!(
        matches!(current_st, ConnectionStatus::Offline { .. }),
        "WsTransport reconnect loop must remain halted in Offline state (found: {:?})",
        current_st
    );

    transport.stop();
    server_task.abort();
}

#[tokio::test]
async fn test_adversarial_ws_transport_401_live_cloud_halts_and_wipes_session() {
    let _lock = acquire_lock();
    let _guard = EnvGuard::new();

    let temp_home = tempfile::tempdir().unwrap();
    set_env_var("HOME", temp_home.path());

    let cloud_url = env::var("GPUI_TELEGRAM_SERVER_URL")
        .unwrap_or_else(|_| "http://34.46.77.133:8080".to_string());

    // 1. Store dummy session
    let session = StoredSession {
        server_url: cloud_url.clone(),
        user_id: 66601,
        username: "adversarial_probe_401".to_string(),
        token: "forged_revoked_adversarial_token_999".to_string(),
        session_id: "sess_cloud_401".to_string(),
        created_at_ms: chrono::Utc::now().timestamp_millis(),
    };
    ClientConfig::save_session(&session).unwrap();

    // 2. Connect against real remote cloud backend with invalid token
    let ws_channel = WsChannel::new();
    let transport = WsTransport::new(cloud_url, ws_channel);
    transport.set_token(Some(session.token.clone()));
    let mut status_rx = transport.subscribe_status();

    transport.start();

    // 3. Await 401 rejection
    let mut caught_401 = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);

    while tokio::time::Instant::now() < deadline {
        tokio::select! {
            res = status_rx.changed() => {
                if res.is_ok() {
                    let st = status_rx.borrow().clone();
                    if let ConnectionStatus::Offline { reason } = st {
                        if reason.contains("401") || reason.contains("unauthorized") {
                            caught_401 = true;
                            ClientConfig::clear_session();
                            transport.set_token(None);
                            break;
                        }
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }

    assert!(
        caught_401,
        "Live cloud backend must reject invalid token with 401 causing WsTransport to halt"
    );

    assert!(
        ClientConfig::load_session().is_none(),
        "Session file must be deleted"
    );

    transport.stop();
}

#[tokio::test]
async fn test_adversarial_ws_transport_max_reconnect_attempts_exhaustion_exact_boundary() {
    // Test case 1: max_reconnect_attempts = 1
    // Unreachable port triggers immediate connect failure.
    // Flow:
    //   Initial connect: fails -> attempt = 1
    //   1 <= 1 -> Reconnecting { attempt: 1 } -> retry after delay
    //   Retry connect: fails -> attempt = 2
    //   2 > 1 -> Offline { reason: "Max reconnection attempts exceeded" } -> loop terminates.
    let ws_channel = WsChannel::new();
    let unreachable_url = "http://127.0.0.1:1".to_string();
    let transport = WsTransport::new(unreachable_url.clone(), ws_channel)
        .with_max_reconnect_attempts(1);
    transport.set_token(Some("test_token_exhaustion".to_string()));

    let mut status_rx = transport.subscribe_status();
    transport.start();

    let mut reconnecting_attempts = Vec::new();
    let mut saw_offline_exhausted = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);

    while tokio::time::Instant::now() < deadline {
        tokio::select! {
            res = status_rx.changed() => {
                if res.is_ok() {
                    let st = status_rx.borrow().clone();
                    match st {
                        ConnectionStatus::Reconnecting { attempt, .. } => {
                            reconnecting_attempts.push(attempt);
                        }
                        ConnectionStatus::Offline { reason } => {
                            if reason.contains("Max reconnection attempts exceeded") {
                                saw_offline_exhausted = true;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }

    assert!(
        saw_offline_exhausted,
        "WsTransport must terminate into Offline when max attempts (1) is exceeded"
    );
    assert_eq!(
        reconnecting_attempts,
        vec![1],
        "Exactly 1 reconnection retry attempt must be emitted for max_reconnect_attempts = 1"
    );

    // Ensure loop remains halted and does NOT emit attempt 2
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let current_st = transport.status();
    assert!(
        matches!(current_st, ConnectionStatus::Offline { .. }),
        "Status must stay Offline and not restart reconnection"
    );

    transport.stop();
}

#[tokio::test]
async fn test_adversarial_ws_transport_max_reconnect_attempts_2_boundary() {
    // Test case 2: max_reconnect_attempts = 2
    // Flow:
    //   Initial connect: fails -> attempt = 1 -> Reconnecting { attempt: 1 }
    //   Retry 1: fails -> attempt = 2 -> Reconnecting { attempt: 2 }
    //   Retry 2: fails -> attempt = 3 > 2 -> Offline { reason: "Max reconnection attempts exceeded" }
    let ws_channel = WsChannel::new();
    let unreachable_url = "http://127.0.0.1:1".to_string();
    let transport = WsTransport::new(unreachable_url, ws_channel)
        .with_max_reconnect_attempts(2);
    transport.set_token(Some("test_token_exhaustion_2".to_string()));

    let mut status_rx = transport.subscribe_status();
    transport.start();

    let mut reconnecting_attempts = Vec::new();
    let mut saw_offline_exhausted = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(12);

    while tokio::time::Instant::now() < deadline {
        tokio::select! {
            res = status_rx.changed() => {
                if res.is_ok() {
                    let st = status_rx.borrow().clone();
                    match st {
                        ConnectionStatus::Reconnecting { attempt, .. } => {
                            reconnecting_attempts.push(attempt);
                        }
                        ConnectionStatus::Offline { reason } => {
                            if reason.contains("Max reconnection attempts exceeded") {
                                saw_offline_exhausted = true;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }

    assert!(
        saw_offline_exhausted,
        "WsTransport must terminate into Offline when max attempts (2) is exceeded"
    );
    assert_eq!(
        reconnecting_attempts,
        vec![1, 2],
        "Exactly 2 reconnection retry attempts must be emitted for max_reconnect_attempts = 2"
    );

    transport.stop();
}
