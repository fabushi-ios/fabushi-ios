use std::collections::BTreeMap;
use std::fs;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use mahayana_host_runtime::extensions::auth::auth_service::{
    EXPIRY_LEEWAY_MS, FABUSHI_CORE_CREDENTIAL_WAITING_MESSAGE,
    FABUSHI_HOST_ACCESS_CREDENTIAL_FILE_ENV, HostAuthEnvironment,
    HostAuthService, HostAuthServiceOptions, InferenceCredentialStore,
    SAND_SHORTLIVED_CREDS_WAITING_MESSAGE,
};
use mahayana_host_runtime::extensions::auth::credential_renewer::{
    CredentialRenewalBackend, InferenceCredential, SandCredentialRenewalError,
    SAND_INFERENCE_RENEWAL_CREDENTIAL_ENV,
};

struct StaticBackend {
    token: String,
    expires_at_ms: u64,
}

impl CredentialRenewalBackend for StaticBackend {
    fn renew(&self, credential: &str) -> Result<InferenceCredential, SandCredentialRenewalError> {
        assert_eq!(credential, "renewal-secret");
        // Grok's renewal backend is async: even an immediately-resolved Promise
        // yields before onResult fires, so callers can subscribe after service
        // construction without racing the first renewal event. Preserve that
        // scheduling boundary in this synchronous Rust test double.
        thread::sleep(Duration::from_millis(5));
        Ok(InferenceCredential {
            access_token: self.token.clone(),
            expires_at_ms: self.expires_at_ms,
        })
    }
}

fn options(
    env: BTreeMap<String, String>,
    now: Arc<Mutex<u64>>,
    logs: Arc<Mutex<Vec<String>>>,
    backend: Arc<dyn CredentialRenewalBackend>,
) -> HostAuthServiceOptions {
    let now_for_clock = Arc::clone(&now);
    let logs_for_sink = Arc::clone(&logs);
    HostAuthServiceOptions {
        environment: HostAuthEnvironment::from_pairs(env),
        backend_url: Some("http://127.0.0.1:9".into()),
        backend: Some(backend),
        machine_id: Arc::new(|| Ok("machine-test".into())),
        log: Arc::new(move |message| {
            logs_for_sink.lock().unwrap().push(message.to_string());
        }),
        now_ms: Arc::new(move || *now_for_clock.lock().unwrap()),
    }
}

#[test]
fn credential_store_honors_the_grok_expiry_leeway() {
    let store = InferenceCredentialStore::default();
    store.set_credential(InferenceCredential {
        access_token: "token".into(),
        expires_at_ms: 100_000,
    });
    assert_eq!(
        store.get_valid_access_token(100_000 - EXPIRY_LEEWAY_MS - 1),
        Some("token".into()),
    );
    assert_eq!(
        store.get_valid_access_token(100_000 - EXPIRY_LEEWAY_MS),
        None,
    );
    store.clear();
    assert!(!store.has_valid_credential(0));
}

#[test]
fn auth_service_fails_closed_without_a_renewal_source() {
    let now = Arc::new(Mutex::new(10_000));
    let logs = Arc::new(Mutex::new(Vec::new()));
    let service = HostAuthService::new(options(
        BTreeMap::new(),
        now,
        Arc::clone(&logs),
        Arc::new(StaticBackend {
            token: "unused".into(),
            expires_at_ms: 100_000,
        }),
    ))
    .unwrap();

    let error = service.get_access_token().unwrap_err();
    assert_eq!(error.to_string(), SAND_SHORTLIVED_CREDS_WAITING_MESSAGE);
    assert!(!service.has_renewal_credential());
    assert!(logs.lock().unwrap()[0].contains("no renewal credential"));
    service.dispose();
}

#[test]
fn fabushi_account_credential_never_becomes_cursor_host_auth() {
    let now = Arc::new(Mutex::new(10_000));
    let logs = Arc::new(Mutex::new(Vec::new()));
    let service = HostAuthService::new(options(
        BTreeMap::from([(
            FABUSHI_HOST_ACCESS_CREDENTIAL_FILE_ENV.into(),
            "/private/fabushi-host-access-credential-v1.json".into(),
        )]),
        now,
        Arc::clone(&logs),
        Arc::new(StaticBackend {
            token: "must-not-be-used".into(),
            expires_at_ms: 100_000,
        }),
    ))
    .unwrap();

    let error = service.get_access_token().unwrap_err();
    assert_eq!(error.to_string(), SAND_SHORTLIVED_CREDS_WAITING_MESSAGE);
    assert!(!service.has_renewal_credential());
    assert!(service.peek_access_token().is_none());
    assert!(logs.lock().unwrap()[0].contains("reserved for first-party Fabushi"));
    service.dispose();
}

#[test]
fn fabushi_core_credential_is_first_party_redacted_and_tracks_account_switch() {
    let root = std::env::temp_dir().join(format!(
        "fabushi-core-credential-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&root).expect("create credential root");
    let credential_path = root.join("fabushi-host-access-credential-v1.json");
    let token_a = "fabushi-product-token-account-a";
    fs::write(
        &credential_path,
        serde_json::json!({
            "version": 1,
            "issuer": "https://api.ombhrum.com",
            "accessToken": token_a,
            "expiresAtMs": 600_000,
            "sessionId": "session-a",
            "deviceId": "device-a",
            "userId": "account-a"
        })
        .to_string(),
    )
    .expect("write first-party credential");

    let now = Arc::new(Mutex::new(10_000));
    let logs = Arc::new(Mutex::new(Vec::new()));
    let service = HostAuthService::new(options(
        BTreeMap::from([(
            FABUSHI_HOST_ACCESS_CREDENTIAL_FILE_ENV.into(),
            credential_path.to_string_lossy().into_owned(),
        )]),
        Arc::clone(&now),
        Arc::clone(&logs),
        Arc::new(StaticBackend {
            token: "must-not-be-used".into(),
            expires_at_ms: 100_000,
        }),
    ))
    .unwrap();

    let first = service
        .get_fabushi_core_credential()
        .expect("read Fabushi Core credential");
    assert_eq!(first.access_token(), token_a);
    assert_eq!(first.issuer(), "https://api.ombhrum.com");
    assert!(first.authorize_origin("https://api.ombhrum.com").is_ok());
    assert!(first.authorize_origin("https://fabushi-ci-isolated-auth.bhrumom.workers.dev").is_err());
    assert!(first.authorize_origin("https://api2.cursor.sh").is_err());
    assert!(first.authorize_origin("http://127.0.0.1:4200").is_err());
    assert_eq!(first.session_id, "session-a");
    assert_eq!(first.device_id, "device-a");
    assert_eq!(first.user_id, "account-a");
    let rendered = format!("{first:?}");
    assert!(!rendered.contains(token_a));
    assert!(rendered.contains("<redacted>"));
    assert!(service.peek_access_token().is_none());
    assert!(!service.has_renewal_credential());

    fs::write(
        &credential_path,
        serde_json::json!({
            "version": 1,
            "accessToken": "fabushi-product-token-account-b",
            "expiresAtMs": 700_000,
            "sessionId": "session-b",
            "deviceId": "device-b",
            "userId": 42
        })
        .to_string(),
    )
    .expect("replace account credential");
    let second = service
        .get_fabushi_core_credential()
        .expect("read replacement credential");
    assert_eq!(second.session_id, "session-b");
    assert_eq!(second.device_id, "device-b");
    assert_eq!(second.user_id, "42");
    assert_eq!(second.access_token(), "fabushi-product-token-account-b");
    fs::write(&credential_path, serde_json::json!({
        "version": 1,
        "issuer": "https://fabushi-ci-isolated-auth.bhrumom.workers.dev",
        "accessToken": "isolated-fixture-token",
        "expiresAtMs": 700_000,
        "sessionId": "isolated-session", "deviceId": "device-a", "userId": "account-a"
    }).to_string()).unwrap();
    let isolated = service.get_fabushi_core_credential().unwrap();
    assert!(isolated.authorize_origin("https://fabushi-ci-isolated-auth.bhrumom.workers.dev").is_ok());
    assert!(isolated.authorize_origin("https://api.ombhrum.com").is_err());
    fs::write(&credential_path, serde_json::json!({
        "version": 1, "issuer": "https://api2.cursor.sh", "accessToken": "foreign-fixture-token",
        "expiresAtMs": 700_000,
        "sessionId": "foreign-session", "deviceId": "device-a", "userId": "account-a"
    }).to_string()).unwrap();
    assert!(service.get_fabushi_core_credential().is_err());


    let log_text = logs.lock().unwrap().join("\n");
    assert!(!log_text.contains(token_a));
    assert!(!log_text.contains("fabushi-product-token-account-b"));
    assert!(!log_text.contains(&credential_path.to_string_lossy().to_string()));
    service.dispose();
    fs::remove_dir_all(root).expect("remove credential root");
}

#[test]
fn fabushi_core_credential_fails_closed_for_expiry_or_malformed_content() {
    let root = std::env::temp_dir().join(format!(
        "fabushi-core-credential-invalid-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&root).expect("create credential root");
    let credential_path = root.join("credential.json");
    let now = Arc::new(Mutex::new(100_000));
    let service = HostAuthService::new(options(
        BTreeMap::from([(
            FABUSHI_HOST_ACCESS_CREDENTIAL_FILE_ENV.into(),
            credential_path.to_string_lossy().into_owned(),
        )]),
        Arc::clone(&now),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(StaticBackend {
            token: "unused".into(),
            expires_at_ms: 500_000,
        }),
    ))
    .unwrap();

    fs::write(
        &credential_path,
        serde_json::json!({
            "version": 1,
            "accessToken": "expired-secret",
            "expiresAtMs": 100_000 + EXPIRY_LEEWAY_MS,
            "sessionId": "session-expired",
            "deviceId": "device-expired",
            "userId": "account-expired"
        })
        .to_string(),
    )
    .expect("write expired credential");
    let expired = service.get_fabushi_core_credential().unwrap_err();
    assert_eq!(expired.to_string(), FABUSHI_CORE_CREDENTIAL_WAITING_MESSAGE);
    assert!(!expired.to_string().contains("expired-secret"));

    fs::write(&credential_path, "{not-json").expect("write malformed credential");
    let malformed = service.get_fabushi_core_credential().unwrap_err();
    assert_eq!(malformed.to_string(), FABUSHI_CORE_CREDENTIAL_WAITING_MESSAGE);
    assert!(!malformed.to_string().contains(&credential_path.to_string_lossy().to_string()));

    service.dispose();
    fs::remove_dir_all(root).expect("remove credential root");
}

#[test]
fn auth_service_renews_stores_emits_first_credential_and_machine_id() {
    let now = Arc::new(Mutex::new(10_000));
    let logs = Arc::new(Mutex::new(Vec::new()));
    let service = HostAuthService::new(options(
        BTreeMap::from([(
            SAND_INFERENCE_RENEWAL_CREDENTIAL_ENV.into(),
            "renewal-secret".into(),
        )]),
        Arc::clone(&now),
        Arc::clone(&logs),
        Arc::new(StaticBackend {
            token: "short-lived-token".into(),
            expires_at_ms: 600_000,
        }),
    ))
    .unwrap();

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_sink = Arc::clone(&events);
    let subscription = service.subscribe_to_renewal(Arc::new(move |event| {
        events_sink.lock().unwrap().push(event);
    }));

    for _ in 0..100 {
        if service.peek_access_token().is_some()
            && service.get_last_renewal_event().is_some()
            && events.lock().unwrap().len() == 1
        {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }

    assert_eq!(service.get_access_token().unwrap(), "short-lived-token");
    assert_eq!(service.get_machine_id().unwrap(), "machine-test");
    let last = service.get_last_renewal_event().expect("renewal event");
    assert!(last.is_first_credential);
    assert_eq!(
        events.lock().unwrap().len(),
        1,
        "the subscribed first-renewal listener must observe exactly one event"
    );
    assert!(service.unsubscribe_from_renewal(subscription));
    assert!(logs.lock().unwrap()[0].contains("sole inference-credential source"));

    *now.lock().unwrap() = 600_000 - EXPIRY_LEEWAY_MS;
    assert!(service.peek_access_token().is_none());
    service.dispose();
}
