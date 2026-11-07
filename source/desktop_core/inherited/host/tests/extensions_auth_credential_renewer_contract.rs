use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use mahayana_host_runtime::extensions::auth::credential_renewer::{
    CREDENTIAL_RETRY_BASE_DELAY_MS, CREDENTIAL_RETRY_MAX_DELAY_MS, DEFAULT_TTL_MS,
    InferenceCredential, MAX_REFRESH_INTERVAL_MS, MIN_REFRESH_INTERVAL_MS,
    REFRESH_LEEWAY_MS, RenewalOutcome, SandInferenceCredentialRenewer,
    CredentialRenewalBackend, CredentialRenewerHooks, SandCredentialRenewalError,
    get_access_token_expiry_ms, read_dev_inference_credential_file,
    redact_renewal_error_for_report, refresh_delay_ms, renew_sand_box_inference_credential,
    retry_delay_ms,
};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[test]
fn credential_payload_uses_exp_claim_then_default_ttl() {
    let payload = URL_SAFE_NO_PAD.encode(br#"{"exp":2000000000}"#);
    let token = format!("header.{payload}.signature");
    assert_eq!(get_access_token_expiry_ms(&token), Some(2_000_000_000_000));

    let path = PathBuf::from(format!(
        "{}/fabushi-auth-token-{}-{}.json",
        std::env::temp_dir().display(),
        std::process::id(),
        now_ms(),
    ));
    std::fs::write(&path, format!(r#"{{"accessToken":"{token}"}}"#)).unwrap();
    let parsed = read_dev_inference_credential_file(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(parsed.access_token, token);
    assert_eq!(parsed.expires_at_ms, 2_000_000_000_000);

    assert_eq!(DEFAULT_TTL_MS, 10 * 60 * 1_000);
}

#[test]
fn refresh_and_retry_schedules_match_grok_bounds() {
    let now = 1_000_000_u64;
    assert_eq!(
        refresh_delay_ms(now + REFRESH_LEEWAY_MS + 1, now),
        MIN_REFRESH_INTERVAL_MS,
    );
    assert_eq!(
        refresh_delay_ms(now + REFRESH_LEEWAY_MS + MAX_REFRESH_INTERVAL_MS * 2, now),
        MAX_REFRESH_INTERVAL_MS,
    );
    assert_eq!(retry_delay_ms(1), CREDENTIAL_RETRY_BASE_DELAY_MS);
    assert_eq!(retry_delay_ms(2), CREDENTIAL_RETRY_BASE_DELAY_MS * 2);
    assert_eq!(retry_delay_ms(100), CREDENTIAL_RETRY_MAX_DELAY_MS);
}

#[test]
fn renewal_error_report_redacts_urls_paths_and_long_ids() {
    let redacted = redact_renewal_error_for_report(
        "POST https://secret.example/api failed /Users/name/token ABCDEFGHIJKLMNOPQRSTUVWXYZ012345",
    );
    assert_eq!(redacted, "POST <url> failed <path> <id>");
}

#[test]
fn production_renewer_posts_backend_metadata_and_reads_credential() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut received = Vec::new();
        let mut buffer = [0_u8; 2048];
        let header_end;
        loop {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0);
            received.extend_from_slice(&buffer[..count]);
            if let Some(index) = received.windows(4).position(|w| w == b"\r\n\r\n") {
                header_end = index;
                break;
            }
        }
        let headers = std::str::from_utf8(&received[..header_end]).unwrap();
        let length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        while received.len() < header_end + 4 + length {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0);
            received.extend_from_slice(&buffer[..count]);
        }
        let body = br#"{"accessToken":"short-lived","expiresAtMs":424242}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
        stream.flush().unwrap();
        received
    });

    let credential = renew_sand_box_inference_credential(
        &format!("http://127.0.0.1:{port}/nested/base"),
        "renewal-secret",
    )
    .unwrap();
    assert_eq!(
        credential,
        InferenceCredential {
            access_token: "short-lived".into(),
            expires_at_ms: 424242,
        }
    );

    let request = server.join().unwrap();
    let text = String::from_utf8_lossy(&request);
    assert!(text.starts_with("POST /sand-box/inference-credential HTTP/1.1\r\n"));
    assert!(text.to_ascii_lowercase().contains("x-cursor-client-type: sand\r\n"));
    assert!(text.to_ascii_lowercase().contains("x-cursor-client-version:"));
    assert!(text.to_ascii_lowercase().contains("x-sand-box-namespace:"));
    assert!(text.contains(r#"{"credential":"renewal-secret"}"#));
}

struct SequenceBackend {
    calls: Mutex<u32>,
}

impl CredentialRenewalBackend for SequenceBackend {
    fn renew(&self, _credential: &str) -> Result<InferenceCredential, SandCredentialRenewalError> {
        let mut calls = self.calls.lock().unwrap();
        *calls += 1;
        if *calls == 1 {
            return Err(SandCredentialRenewalError::Transport(
                "https://secret.example/ ABCDEFGHIJKLMNOPQRSTUVWXYZ".into(),
            ));
        }
        Ok(InferenceCredential {
            access_token: format!("token-{calls}"),
            expires_at_ms: now_ms() + 600_000,
        })
    }
}

#[test]
fn background_renewer_reports_failure_then_supports_immediate_retry() {
    let backend = Arc::new(SequenceBackend {
        calls: Mutex::new(0),
    });
    let source = Arc::new(|| Some("renewal-secret".to_string()));
    let stored = Arc::new(Mutex::new(Vec::<InferenceCredential>::new()));
    let stored_sink = Arc::clone(&stored);
    let reports = Arc::new(Mutex::new(Vec::new()));
    let reports_sink = Arc::clone(&reports);

    let mut renewer = SandInferenceCredentialRenewer::new(
        backend,
        CredentialRenewerHooks {
            get_credential: source,
            set_credential: Arc::new(move |credential| {
                stored_sink.lock().unwrap().push(credential);
            }),
            on_result: Some(Arc::new(move |result| {
                reports_sink.lock().unwrap().push(result);
            })),
            now_ms: Arc::new(now_ms),
        },
    );
    renewer.start();

    for _ in 0..100 {
        if reports.lock().unwrap().len() >= 1 {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(reports.lock().unwrap()[0].outcome, RenewalOutcome::Failed);
    assert!(renewer.request_immediate_renewal());

    for _ in 0..100 {
        if !stored.lock().unwrap().is_empty() {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    renewer.close();

    let reports = reports.lock().unwrap();
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[1].outcome, RenewalOutcome::Renewed);
    assert_eq!(stored.lock().unwrap()[0].access_token, "token-2");
}

#[test]
fn product_service_origin_has_no_cursor_default_or_foreign_override() {
    use mahayana_host_runtime::extensions::auth::credential_renewer::configured_product_backend_url;
    assert_eq!(configured_product_backend_url(None, None).unwrap(), "https://api.ombhrum.com/");
    assert_eq!(configured_product_backend_url(Some("http://127.0.0.1:4200"), Some("https://api2.cursor.sh")).unwrap(), "http://127.0.0.1:4200/");
    for origin in ["https://api2.cursor.sh", "https://cursor.com", "https://api3.cursor.sh", "https://example.com", "http://api.ombhrum.com", "https://token@api.ombhrum.com", "https://api.ombhrum.com/?secret=x"] {
        assert!(configured_product_backend_url(Some(origin), None).is_err(), "origin must be refused");
        assert!(configured_product_backend_url(None, Some(origin)).is_err(), "legacy override must be refused");
    }
    assert!(configured_product_backend_url(Some("https://fabushi-ci-isolated-auth.bhrumom.workers.dev"), None).is_ok());
}
