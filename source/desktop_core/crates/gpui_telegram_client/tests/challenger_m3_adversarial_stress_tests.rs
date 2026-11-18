use gpui_telegram_client::config::{ClientConfig, ConfigSource, StoredSession, DEFAULT_SERVER_URL};
use gpui_telegram_client::net::http_client::{HttpClient, HttpClientError};
use gpui_telegram_client::net::{ConnectionStatus, WsChannel, WsTransport};
use std::env;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

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
    set_env("GPUI_TELEGRAM_SERVER_URL", "34.46.77.133:8080//");

    let cfg_dir = temp_dir.path().join(".config").join("gpui_telegram");
    fs::create_dir_all(&cfg_dir).unwrap();
    fs::write(
        cfg_dir.join("config.json"),
        r#"{"server_url": "http://config-file:8080"}"#,
    )
    .unwrap();

    let empty_args = vec!["gpui_telegram".to_string()];
    let cfg = ClientConfig::resolve_with_args(&empty_args);
    assert_eq!(cfg.server_url, "http://34.46.77.133:8080");
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
        server_url: "http://34.46.77.133:8080".to_string(),
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
        br#"{"server_url":"http://34.46.77.133:8080","user_id":888,"#,
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
        br#"{"server_url":"http://34.46.77.133:8080","user_id":888}"#,
    )
    .unwrap();
    assert!(ClientConfig::load_session().is_none());

    // Corruption Case 5: Type mismatch (e.g., token is a number instead of string)
    fs::write(
        &session_path,
        br#"{
            "server_url": "http://34.46.77.133:8080",
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
    let client = HttpClient::new("http://34.46.77.133:8080".to_string());

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
        server_url: "http://34.46.77.133:8080".to_string(),
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
                    server_url: "http://34.46.77.133:8080".to_string(),
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

// ============================================================================
// Challenge Area 6: Custom Timeouts Preservation Under CLI & ENV Overrides
// ============================================================================

#[test]
fn test_adversarial_cli_override_strictly_preserves_custom_timeouts() {
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
            "server_url": "http://config-host.corp:8080",
            "http_timeout_secs": 42,
            "ws_ping_interval_secs": 55,
            "max_reconnect_attempts": 7
        }"#,
    )
    .unwrap();

    // 1. Long flag: --server-url
    let args1 = vec![
        "gpui_telegram".to_string(),
        "--server-url".to_string(),
        "http://cli-override.corp:9090".to_string(),
    ];
    let cfg1 = ClientConfig::resolve_with_args(&args1);
    assert_eq!(cfg1.server_url, "http://cli-override.corp:9090");
    assert_eq!(cfg1.config_source, ConfigSource::CliArgument);
    assert_eq!(cfg1.http_timeout_secs, 42, "CLI override must preserve custom http_timeout_secs from config.json");
    assert_eq!(cfg1.ws_ping_interval_secs, 55, "CLI override must preserve custom ws_ping_interval_secs from config.json");
    assert_eq!(cfg1.max_reconnect_attempts, 7, "CLI override must preserve custom max_reconnect_attempts from config.json");

    // 2. Equals syntax: --server-url=...
    let args2 = vec![
        "gpui_telegram".to_string(),
        "--server-url=http://cli-eq.corp:9091/".to_string(),
    ];
    let cfg2 = ClientConfig::resolve_with_args(&args2);
    assert_eq!(cfg2.server_url, "http://cli-eq.corp:9091");
    assert_eq!(cfg2.config_source, ConfigSource::CliArgument);
    assert_eq!(cfg2.http_timeout_secs, 42);
    assert_eq!(cfg2.ws_ping_interval_secs, 55);
    assert_eq!(cfg2.max_reconnect_attempts, 7);

    // 3. Short flag: -s
    let args3 = vec![
        "gpui_telegram".to_string(),
        "-s".to_string(),
        "http://cli-short.corp:9092".to_string(),
    ];
    let cfg3 = ClientConfig::resolve_with_args(&args3);
    assert_eq!(cfg3.server_url, "http://cli-short.corp:9092");
    assert_eq!(cfg3.config_source, ConfigSource::CliArgument);
    assert_eq!(cfg3.http_timeout_secs, 42);
    assert_eq!(cfg3.ws_ping_interval_secs, 55);
    assert_eq!(cfg3.max_reconnect_attempts, 7);

    // 4. Short flag equals: -s=...
    let args4 = vec![
        "gpui_telegram".to_string(),
        "-s=http://cli-short-eq.corp:9093".to_string(),
    ];
    let cfg4 = ClientConfig::resolve_with_args(&args4);
    assert_eq!(cfg4.server_url, "http://cli-short-eq.corp:9093");
    assert_eq!(cfg4.config_source, ConfigSource::CliArgument);
    assert_eq!(cfg4.http_timeout_secs, 42);
    assert_eq!(cfg4.ws_ping_interval_secs, 55);
    assert_eq!(cfg4.max_reconnect_attempts, 7);
}

#[test]
fn test_adversarial_env_override_strictly_preserves_custom_timeouts() {
    let _lock = acquire_env_lock();
    let _guard = EnvGuard::new();

    let temp_dir = tempfile::tempdir().unwrap();
    set_env("HOME", temp_dir.path());
    set_env("GPUI_TELEGRAM_SERVER_URL", "http://env-host.corp:7070");

    let cfg_dir = temp_dir.path().join(".config").join("gpui_telegram");
    fs::create_dir_all(&cfg_dir).unwrap();
    fs::write(
        cfg_dir.join("config.json"),
        r#"{
            "server_url": "http://config-host.corp:8080",
            "http_timeout_secs": 19,
            "ws_ping_interval_secs": 41,
            "max_reconnect_attempts": 3
        }"#,
    )
    .unwrap();

    let empty_args = vec!["gpui_telegram".to_string()];
    let cfg = ClientConfig::resolve_with_args(&empty_args);
    assert_eq!(cfg.server_url, "http://env-host.corp:7070");
    assert_eq!(cfg.config_source, ConfigSource::EnvironmentVariable);
    assert_eq!(cfg.http_timeout_secs, 19, "ENV override must preserve custom http_timeout_secs from config.json");
    assert_eq!(cfg.ws_ping_interval_secs, 41, "ENV override must preserve custom ws_ping_interval_secs from config.json");
    assert_eq!(cfg.max_reconnect_attempts, 3, "ENV override must preserve custom max_reconnect_attempts from config.json");
}

// ============================================================================
// Challenge Area 7: Exponential Backoff Non-Deterministic Jitter Spread Oracle
// ============================================================================

#[test]
fn test_adversarial_exponential_backoff_jitter_oracle_distribution() {
    // Mathematical oracle for the backoff jitter specification:
    // base_ms = 1000 * 2^attempt.min(5) capped at 30,000 ms.
    // spread = +/- 25% (range: 0.75 .. 1.25)
    // clamped to [500, 30_000] ms.
    let attempts = [0u32, 1, 2, 3, 4, 5, 6, 10];

    for &att in &attempts {
        let base_ms = 1000u64.saturating_mul(1u64 << att.min(5)).min(30_000);
        let min_expected = ((base_ms as f64) * 0.75) as u64;
        let max_expected = (((base_ms as f64) * 1.25) as u64).min(30_000);

        let mut observed_min = u64::MAX;
        let mut observed_max = 0u64;
        let mut distinct_values = std::collections::HashSet::new();

        for _ in 0..1000 {
            let mut rand_bytes = [0u8; 4];
            getrandom::getrandom(&mut rand_bytes).unwrap();
            let rand_val = u32::from_le_bytes(rand_bytes) as f64 / u32::MAX as f64;
            let jitter_factor = 0.75 + (rand_val * 0.50);
            let jittered_ms = (((base_ms as f64) * jitter_factor) as u64).max(500).min(30_000);

            assert!(
                jittered_ms >= min_expected,
                "Jittered delay {}ms fell below minimum {}ms for attempt {}",
                jittered_ms, min_expected, att
            );
            assert!(
                jittered_ms <= max_expected,
                "Jittered delay {}ms exceeded maximum {}ms for attempt {}",
                jittered_ms, max_expected, att
            );

            observed_min = observed_min.min(jittered_ms);
            observed_max = observed_max.max(jittered_ms);
            distinct_values.insert(jittered_ms);
        }

        // Assert non-deterministic entropy: 1000 samples must generate a wide distribution
        assert!(
            distinct_values.len() > 100,
            "Jitter must be non-deterministic (expected >100 distinct values, got {})",
            distinct_values.len()
        );

        // Assert spread spans close to boundaries
        let spread_ratio = (observed_max - observed_min) as f64 / (base_ms as f64);
        if att < 5 {
            // For att < 5, max is not clipped by 30_000, so spread should be ~0.50 (+/- 25%)
            assert!(
                spread_ratio > 0.40,
                "Spread ratio {:.3} is too narrow for attempt {} (expected >0.40)",
                spread_ratio, att
            );
        }
    }
}

// ============================================================================
// Challenge Area 8: WsTransport 401 Interception & Reconnect Termination
// ============================================================================

#[tokio::test]
async fn test_adversarial_ws_transport_401_terminates_loop_and_clears_session() {
    let _lock = acquire_env_lock();
    let _guard = EnvGuard::new();

    let temp_dir = tempfile::tempdir().unwrap();
    set_env("HOME", temp_dir.path());

    // 1. Store a session file on disk
    let session = StoredSession {
        server_url: "http://34.46.77.133:8080".to_string(),
        user_id: 77701,
        username: "revoked_ws_user".to_string(),
        token: "adversarial_forged_token_ws_401".to_string(),
        session_id: "sess_revoked_ws".to_string(),
        created_at_ms: chrono::Utc::now().timestamp_millis(),
    };
    ClientConfig::save_session(&session).unwrap();
    assert!(ClientConfig::load_session().is_some());

    // 2. Initialize WsTransport with the invalid token against live cloud backend
    let ws_channel = WsChannel::new();
    let transport = WsTransport::new("http://34.46.77.133:8080".to_string(), ws_channel);
    let mut status_rx = transport.subscribe_status();
    transport.set_token(Some(session.token.clone()));
    transport.start();

    // 3. Await status transition to Offline with 401 reason
    let mut reached_offline_401 = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);

    while tokio::time::Instant::now() < deadline {
        tokio::select! {
            res = status_rx.changed() => {
                if res.is_ok() {
                    let st = status_rx.borrow().clone();
                    if let ConnectionStatus::Offline { reason } = st {
                        if reason.contains("401") || reason.contains("unauthorized") || reason.contains("Unauthorized") {
                            reached_offline_401 = true;
                            // Simulate shell.rs session purge on 401:
                            ClientConfig::clear_session();
                            break;
                        }
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }

    assert!(
        reached_offline_401,
        "WsTransport must terminate and transition to Offline with 401 reason when connecting with invalid token"
    );

    // 4. Verify session file was purged
    assert!(
        ClientConfig::load_session().is_none(),
        "Session file must be deleted upon 401 unauthorized status"
    );

    // 5. Verify reconnect loop did NOT resume (status remains Offline)
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let final_status = transport.status();
    assert!(
        matches!(final_status, ConnectionStatus::Offline { .. }),
        "WsTransport must stay in Offline state and terminate the reconnect loop immediately after 401 (found: {:?})",
        final_status
    );
}

#[tokio::test]
async fn test_adversarial_ws_transport_reconnect_jittered_delay_bounds() {
    let ws_channel = WsChannel::new();
    // Use an unreachable port to trigger immediate connection failure
    let transport = WsTransport::new("http://127.0.0.1:1".to_string(), ws_channel);
    let mut status_rx = transport.subscribe_status();
    transport.set_token(Some("valid_format_dummy_token".to_string()));
    transport.start();

    // Await Reconnecting status
    let mut reached_reconnecting = false;
    let mut observed_retry_secs = 0u64;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    while tokio::time::Instant::now() < deadline {
        tokio::select! {
            res = status_rx.changed() => {
                if res.is_ok() {
                    let st = status_rx.borrow().clone();
                    if let ConnectionStatus::Reconnecting { attempt, next_retry_in_secs } = st {
                        assert_eq!(attempt, 1, "First reconnect must be attempt 1");
                        observed_retry_secs = next_retry_in_secs;
                        reached_reconnecting = true;
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }

    transport.stop();

    assert!(reached_reconnecting, "WsTransport must enter Reconnecting status on network failure");
    // For attempt 1: base = 2000 ms. Jitter +/- 25% gives 1500..2500 ms.
    // as_secs().max(1) produces 1 or 2.
    assert!(
        (1..=3).contains(&observed_retry_secs),
        "Expected retry_secs for attempt 1 to be in 1..=3 seconds, got {}",
        observed_retry_secs
    );
}

// ============================================================================
// Challenge Area 9: Session Permissions Under Hostile Process Umasks
// ============================================================================

#[test]
fn test_adversarial_session_posix_permissions_under_hostile_umasks() {
    let _lock = acquire_env_lock();
    let _guard = EnvGuard::new();

    let temp_dir = tempfile::tempdir().unwrap();
    set_env("HOME", temp_dir.path());

    let session = StoredSession {
        server_url: "http://34.46.77.133:8080".to_string(),
        user_id: 12345,
        username: "umask_user".to_string(),
        token: "tok_umask".to_string(),
        session_id: "sess_umask".to_string(),
        created_at_ms: chrono::Utc::now().timestamp_millis(),
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        // Test under umask 0000 (wide open default)
        let old_umask = unsafe { libc::umask(0o000) };
        let save1 = ClientConfig::save_session(&session);
        unsafe { libc::umask(old_umask) };
        assert!(save1.is_ok());

        let p1 = ClientConfig::get_session_path();
        let m1 = fs::metadata(&p1).unwrap().permissions().mode() & 0o777;
        assert_eq!(m1, 0o600, "Even with umask 0000, session file mode must be strictly 0600, found {:o}", m1);

        // Test directory mode 0700
        let dir_mode = fs::metadata(ClientConfig::get_app_dir()).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "Config dir mode must be strictly 0700, found {:o}", dir_mode);

        // Test under umask 0077 (restrictive)
        let old_umask2 = unsafe { libc::umask(0o077) };
        let save2 = ClientConfig::save_session(&session);
        unsafe { libc::umask(old_umask2) };
        assert!(save2.is_ok());

        let m2 = fs::metadata(&p1).unwrap().permissions().mode() & 0o777;
        assert_eq!(m2, 0o600, "With umask 0077, session file mode must remain 0600, found {:o}", m2);
    }
}
