use gpui_telegram_client::config::{ClientConfig, ConfigSource, StoredSession, DEFAULT_SERVER_URL};
use gpui_telegram_client::net::http_client::{HttpClient, HttpClientError};
use std::env;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn acquire_env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn set_env(key: &str, val: impl AsRef<Path>) {
    unsafe {
        env::set_var(key, val.as_ref());
    }
}

fn remove_env(key: &str) {
    unsafe {
        env::remove_var(key);
    }
}

struct EnvGuard {
    saved_home: Option<String>,
    saved_url: Option<String>,
}

impl EnvGuard {
    fn new() -> Self {
        Self {
            saved_home: env::var("HOME").ok(),
            saved_url: env::var("GPUI_TELEGRAM_SERVER_URL").ok(),
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(ref h) = self.saved_home {
            set_env("HOME", h);
        } else {
            remove_env("HOME");
        }
        if let Some(ref u) = self.saved_url {
            set_env("GPUI_TELEGRAM_SERVER_URL", u);
        } else {
            remove_env("GPUI_TELEGRAM_SERVER_URL");
        }
    }
}

// ============================================================================
// Challenge Area 1: 4-Tier Precedence Under Adversarial & Conflicting Conditions
// ============================================================================

#[test]
fn test_adversarial_cli_precedence_over_all_tiers() {
    let _lock = acquire_env_lock();
    let _guard = EnvGuard::new();

    let temp_dir = tempfile::tempdir().unwrap();
    set_env("HOME", temp_dir.path());
    set_env("GPUI_TELEGRAM_SERVER_URL", "http://env-override:8080");

    // Create Tier 3 config.json
    let cfg_dir = temp_dir.path().join(".config").join("gpui_telegram");
    fs::create_dir_all(&cfg_dir).unwrap();
    fs::write(
        cfg_dir.join("config.json"),
        r#"{"server_url": "http://config-file:8080"}"#,
    )
    .unwrap();

    // Adversarial CLI args with trailing slashes
    let args = vec![
        "gpui_telegram".to_string(),
        "--server-url".to_string(),
        "http://cli-override:9999///".to_string(),
    ];

    let cfg = ClientConfig::resolve_with_args(&args);
    assert_eq!(cfg.server_url, "http://cli-override:9999");
    assert_eq!(cfg.config_source, ConfigSource::CliArgument);
}

#[test]
fn test_adversarial_cli_equals_syntax_overrides_env_and_file() {
    let _lock = acquire_env_lock();
    let _guard = EnvGuard::new();

    let temp_dir = tempfile::tempdir().unwrap();
    set_env("HOME", temp_dir.path());
    set_env("GPUI_TELEGRAM_SERVER_URL", "http://env-server:8080");

    let cfg_dir = temp_dir.path().join(".config").join("gpui_telegram");
    fs::create_dir_all(&cfg_dir).unwrap();
    fs::write(
        cfg_dir.join("config.json"),
        r#"{"server_url": "http://config-file:8080"}"#,
    )
    .unwrap();

    let args = vec![
        "gpui_telegram".to_string(),
        "--server-url=https://secure.custom-cloud.io:8443/".to_string(),
    ];

    let cfg = ClientConfig::resolve_with_args(&args);
    assert_eq!(cfg.server_url, "https://secure.custom-cloud.io:8443");
    assert_eq!(cfg.config_source, ConfigSource::CliArgument);
}

#[test]
fn test_adversarial_env_precedence_over_config_file_and_default() {
    let _lock = acquire_env_lock();
    let _guard = EnvGuard::new();

    let temp_dir = tempfile::tempdir().unwrap();
    set_env("HOME", temp_dir.path());
    set_env("GPUI_TELEGRAM_SERVER_URL", "34.170.16.35:8080//");

    let cfg_dir = temp_dir.path().join(".config").join("gpui_telegram");
    fs::create_dir_all(&cfg_dir).unwrap();
    fs::write(
        cfg_dir.join("config.json"),
        r#"{"server_url": "http://config-file:8080"}"#,
    )
    .unwrap();

    let empty_args = vec!["gpui_telegram".to_string()];
    let cfg = ClientConfig::resolve_with_args(&empty_args);
    assert_eq!(cfg.server_url, "http://34.170.16.35:8080");
    assert_eq!(cfg.config_source, ConfigSource::EnvironmentVariable);
}

#[test]
fn test_adversarial_config_file_precedence_over_default() {
    let _lock = acquire_env_lock();
    let _guard = EnvGuard::new();

    let temp_dir = tempfile::tempdir().unwrap();
    set_env("HOME", temp_dir.path());
    remove_env("GPUI_TELEGRAM_SERVER_URL");

    let cfg_dir = temp_dir.path().join(".config").join("gpui_telegram");
    fs::create_dir_all(&cfg_dir).unwrap();
    fs::write(
        cfg_dir.join("config.json"),
        r#"{
            "server_url": "http://10.0.0.5:8080/",
            "http_timeout_secs": 15,
            "ws_ping_interval_secs": 30,
            "max_reconnect_attempts": 5
        }"#,
    )
    .unwrap();

    let empty_args = vec!["gpui_telegram".to_string()];
    let cfg = ClientConfig::resolve_with_args(&empty_args);
    assert_eq!(cfg.server_url, "http://10.0.0.5:8080");
    assert_eq!(cfg.http_timeout_secs, 15);
    assert_eq!(cfg.ws_ping_interval_secs, 30);
    assert_eq!(cfg.max_reconnect_attempts, 5);
    assert_eq!(
        cfg.config_source,
        ConfigSource::ConfigFile(cfg_dir.join("config.json"))
    );
}

#[test]
fn test_adversarial_malformed_and_edge_case_config_files() {
    let _lock = acquire_env_lock();
    let _guard = EnvGuard::new();

    let temp_dir = tempfile::tempdir().unwrap();
    set_env("HOME", temp_dir.path());
    remove_env("GPUI_TELEGRAM_SERVER_URL");

    let cfg_dir = temp_dir.path().join(".config").join("gpui_telegram");
    fs::create_dir_all(&cfg_dir).unwrap();
    let config_path = cfg_dir.join("config.json");
    let empty_args = vec!["gpui_telegram".to_string()];

    // Case 1: Malformed JSON syntax
    fs::write(&config_path, b"{ broken_json: [").unwrap();
    let cfg1 = ClientConfig::resolve_with_args(&empty_args);
    assert_eq!(cfg1.server_url, DEFAULT_SERVER_URL);
    assert_eq!(cfg1.config_source, ConfigSource::DefaultFallback);

    // Case 2: Empty JSON object
    fs::write(&config_path, b"{}").unwrap();
    let cfg2 = ClientConfig::resolve_with_args(&empty_args);
    assert_eq!(cfg2.server_url, DEFAULT_SERVER_URL);
    assert_eq!(cfg2.config_source, ConfigSource::DefaultFallback);

    // Case 3: JSON with unrelated keys
    fs::write(&config_path, br#"{"unrelated": true, "number": 42}"#).unwrap();
    let cfg3 = ClientConfig::resolve_with_args(&empty_args);
    assert_eq!(cfg3.server_url, DEFAULT_SERVER_URL);
    assert_eq!(cfg3.config_source, ConfigSource::DefaultFallback);

    // Case 4: Whitespace-only server_url
    fs::write(&config_path, br#"{"server_url": "   \n\t  "}"#).unwrap();
    let cfg4 = ClientConfig::resolve_with_args(&empty_args);
    assert_eq!(cfg4.server_url, DEFAULT_SERVER_URL);
    assert_eq!(cfg4.config_source, ConfigSource::DefaultFallback);

    // Case 5: Empty file (0 bytes)
    fs::write(&config_path, b"").unwrap();
    let cfg5 = ClientConfig::resolve_with_args(&empty_args);
    assert_eq!(cfg5.server_url, DEFAULT_SERVER_URL);
    assert_eq!(cfg5.config_source, ConfigSource::DefaultFallback);
}

#[test]
fn test_adversarial_cli_dangling_flags() {
    let _lock = acquire_env_lock();
    let _guard = EnvGuard::new();

    let temp_dir = tempfile::tempdir().unwrap();
    set_env("HOME", temp_dir.path());
    remove_env("GPUI_TELEGRAM_SERVER_URL");

    // Dangling --server-url at end of args list
    let args1 = vec!["gpui_telegram".to_string(), "--server-url".to_string()];
    let cfg1 = ClientConfig::resolve_with_args(&args1);
    assert_eq!(cfg1.server_url, DEFAULT_SERVER_URL);
    assert_eq!(cfg1.config_source, ConfigSource::DefaultFallback);

    // Dangling -s at end of args list
    let args2 = vec!["gpui_telegram".to_string(), "-s".to_string()];
    let cfg2 = ClientConfig::resolve_with_args(&args2);
    assert_eq!(cfg2.server_url, DEFAULT_SERVER_URL);
    assert_eq!(cfg2.config_source, ConfigSource::DefaultFallback);
}

#[test]
fn test_adversarial_cli_empty_url_fallback() {
    let _lock = acquire_env_lock();
    let _guard = EnvGuard::new();

    let temp_dir = tempfile::tempdir().unwrap();
    set_env("HOME", temp_dir.path());
    remove_env("GPUI_TELEGRAM_SERVER_URL");

    // Case 1: Empty CLI url --server-url ""
    let args1 = vec![
        "gpui_telegram".to_string(),
        "--server-url".to_string(),
        "".to_string(),
    ];
    let cfg1 = ClientConfig::resolve_with_args(&args1);
    assert_ne!(
        cfg1.server_url, "http://",
        "Empty CLI argument should not resolve to malformed 'http://'"
    );
    assert_eq!(
        cfg1.server_url, DEFAULT_SERVER_URL,
        "Empty CLI argument should fall back to default server URL"
    );

    // Case 2: Whitespace CLI url --server-url "   "
    let args2 = vec![
        "gpui_telegram".to_string(),
        "--server-url=   ".to_string(),
    ];
    let cfg2 = ClientConfig::resolve_with_args(&args2);
    assert_ne!(
        cfg2.server_url, "http://",
        "Whitespace CLI argument should not resolve to malformed 'http://'"
    );
    assert_eq!(
        cfg2.server_url, DEFAULT_SERVER_URL,
        "Whitespace CLI argument should fall back to default server URL"
    );
}

// ============================================================================
// Challenge Area 2: Session Persistence POSIX Permissions (0600)
// ============================================================================

#[test]
fn test_adversarial_session_posix_permissions_0600() {
    let _lock = acquire_env_lock();
    let _guard = EnvGuard::new();

    let temp_dir = tempfile::tempdir().unwrap();
    set_env("HOME", temp_dir.path());

    let session = StoredSession {
        server_url: "http://34.170.16.35:8080".to_string(),
        user_id: 88812,
        username: "perm_test_user".to_string(),
        token: "secret_token_123456789".to_string(),
        session_id: "sess_perm_test".to_string(),
        created_at_ms: chrono::Utc::now().timestamp_millis(),
    };

    let save_res = ClientConfig::save_session(&session);
    assert!(save_res.is_ok(), "save_session failed: {:?}", save_res);

    let session_path = ClientConfig::get_session_path();
    assert!(session_path.exists(), "session.json does not exist");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(&session_path).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "Session file must strictly have POSIX mode 0600, but found {:o}",
            mode
        );
    }

    let loaded = ClientConfig::load_session();
    assert_eq!(loaded, Some(session));
}

// ============================================================================
// Challenge Area 3: Resilience Against Corrupted & Truncated session.json
// ============================================================================

#[test]
fn test_adversarial_session_corruption_and_truncation_resilience() {
    let _lock = acquire_env_lock();
    let _guard = EnvGuard::new();

    let temp_dir = tempfile::tempdir().unwrap();
    set_env("HOME", temp_dir.path());

    let session_dir = ClientConfig::get_app_dir();
    fs::create_dir_all(&session_dir).unwrap();
    let session_path = ClientConfig::get_session_path();

    // Corruption Case 1: 0-byte empty file
    fs::write(&session_path, b"").unwrap();
    assert!(ClientConfig::load_session().is_none());

    // Corruption Case 2: Truncated JSON during simulated power cut
    fs::write(
        &session_path,
        br#"{"server_url":"http://34.170.16.35:8080","user_id":888,"#,
    )
    .unwrap();
    assert!(ClientConfig::load_session().is_none());

    // Corruption Case 3: Random binary junk / garbage bytes
    let random_junk = [0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01, 0xFF, 0xFE];
    fs::write(&session_path, &random_junk).unwrap();
    assert!(ClientConfig::load_session().is_none());

    // Corruption Case 4: Missing required fields
    fs::write(
        &session_path,
        br#"{"server_url":"http://34.170.16.35:8080","user_id":888}"#,
    )
    .unwrap();
    assert!(ClientConfig::load_session().is_none());

    // Corruption Case 5: Type mismatch (e.g., token is a number instead of string)
    fs::write(
        &session_path,
        br#"{
            "server_url": "http://34.170.16.35:8080",
            "user_id": "not_a_number",
            "username": "user",
            "token": "tok",
            "session_id": "sess",
            "created_at_ms": 1234
        }"#,
    )
    .unwrap();
    assert!(ClientConfig::load_session().is_none());

    // Corruption Case 6: session.json is a directory instead of a file
    fs::remove_file(&session_path).unwrap();
    fs::create_dir_all(&session_path).unwrap();
    assert!(ClientConfig::load_session().is_none());

    // Cleanup directory
    fs::remove_dir_all(&session_path).unwrap();
    assert!(ClientConfig::load_session().is_none());
}

// ============================================================================
// Challenge Area 4: Rejected Session Token (401 Unauthorized) & Session Recovery
// ============================================================================

#[tokio::test]
async fn test_adversarial_rejected_session_token_live_cloud_401() {
    let client = HttpClient::new("http://34.170.16.35:8080".to_string());

    // Adversarial probe: forged/expired session token
    let bogus_token = "adversarial_forged_expired_session_token_xyz987";
    let res = client.get_me(bogus_token).await;

    match res {
        Err(HttpClientError::Unauthorized(msg)) => {
            // Expected 401 Unauthorized from backend
            println!(
                "[Adversarial Test] Successfully caught 401 Unauthorized: {}",
                msg
            );
        }
        other => panic!("Expected HttpClientError::Unauthorized, got: {:?}", other),
    }
}

#[test]
fn test_adversarial_rejected_session_clearing_lifecycle() {
    let _lock = acquire_env_lock();
    let _guard = EnvGuard::new();

    let temp_dir = tempfile::tempdir().unwrap();
    set_env("HOME", temp_dir.path());

    // 1. Simulate existing session on disk
    let session = StoredSession {
        server_url: "http://34.170.16.35:8080".to_string(),
        user_id: 99901,
        username: "revoked_user".to_string(),
        token: "revoked_token_123".to_string(),
        session_id: "revoked_session_id".to_string(),
        created_at_ms: chrono::Utc::now().timestamp_millis(),
    };
    ClientConfig::save_session(&session).unwrap();
    assert!(ClientConfig::get_session_path().exists());
    assert!(ClientConfig::load_session().is_some());

    // 2. Clear session on simulated 401 Unauthorized (matching shell.rs behavior)
    ClientConfig::clear_session();
    assert!(!ClientConfig::get_session_path().exists());
    assert!(ClientConfig::load_session().is_none());

    // 3. Clear session when already cleared should not error
    ClientConfig::clear_session();
    assert!(ClientConfig::load_session().is_none());
}

// ============================================================================
// Challenge Area 5: Concurrent Session Operations Stress Test
// ============================================================================

#[test]
fn test_adversarial_concurrent_session_saves_stress() {
    let _lock = acquire_env_lock();
    let _guard = EnvGuard::new();

    let temp_dir = tempfile::tempdir().unwrap();
    set_env("HOME", temp_dir.path());

    let n_threads = 16;
    let iterations_per_thread = 20;

    let mut handles = Vec::new();
    let error_count = Arc::new(Mutex::new(0));

    for thread_idx in 0..n_threads {
        let err_cnt = Arc::clone(&error_count);
        let handle = thread::spawn(move || {
            for iter in 0..iterations_per_thread {
                let session = StoredSession {
                    server_url: "http://34.170.16.35:8080".to_string(),
                    user_id: 1000 + (thread_idx as u64) * 100 + (iter as u64),
                    username: format!("thread_{}_iter_{}", thread_idx, iter),
                    token: format!("tok_{}_{}", thread_idx, iter),
                    session_id: format!("sess_{}_{}", thread_idx, iter),
                    created_at_ms: chrono::Utc::now().timestamp_millis(),
                };
                if let Err(e) = ClientConfig::save_session(&session) {
                    eprintln!(
                        "Error saving session from thread {} iter {}: {:?}",
                        thread_idx, iter, e
                    );
                    let mut lock = err_cnt.lock().unwrap();
                    *lock += 1;
                }
            }
        });
        handles.push(handle);
    }

    for h in handles {
        h.join().unwrap();
    }

    let final_errors = *error_count.lock().unwrap();
    println!(
        "[Adversarial Concurrency Test] Completed {} concurrent saves across {} threads. Errors: {}",
        n_threads * iterations_per_thread,
        n_threads,
        final_errors
    );

    assert_eq!(
        final_errors, 0,
        "Concurrent saves must not fail due to shared temp file collision (found {} errors)",
        final_errors
    );

    // Verify session file is in a valid, loadable state and not corrupted
    let loaded = ClientConfig::load_session();
    assert!(
        loaded.is_some(),
        "Session file must remain valid and uncorrupted after concurrent writes"
    );

    // Verify permissions still 0600
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(ClientConfig::get_session_path()).unwrap();
        assert_eq!(
            metadata.permissions().mode() & 0o777,
            0o600,
            "Permissions must remain 0600 after concurrent writes"
        );
    }
}
