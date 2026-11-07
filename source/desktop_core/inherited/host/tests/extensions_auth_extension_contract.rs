use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use mahayana_host_runtime::extensions::auth::auth_service::{
    HostAuthEnvironment, HostAuthServiceOptions,
};
use mahayana_host_runtime::extensions::auth::credential_renewer::{
    CredentialRenewalBackend, InferenceCredential, SAND_INFERENCE_RENEWAL_CREDENTIAL_ENV,
    SandCredentialRenewalError,
};
use mahayana_host_runtime::extensions::auth::extension::{
    AUTH_DEPENDENCIES, auth_extension_id, start_host_auth_extension_with_options,
};
use mahayana_host_runtime::extensions::auth::user_full_name_service::{
    display_name_from, non_empty, principal_from_access_token,
};
use mahayana_host_runtime::extensions::browser_ua::extension::BrowserUaAuthApi;
use mahayana_host_runtime::extensions::extension_ids_generated::HostExtensionId;

fn token(sub: &str) -> String {
    let payload = URL_SAFE_NO_PAD.encode(format!(r#"{{"sub":"{sub}"}}"#));
    format!("header.{payload}.signature")
}

struct StaticBackend {
    access_token: String,
}

impl CredentialRenewalBackend for StaticBackend {
    fn renew(&self, credential: &str) -> Result<InferenceCredential, SandCredentialRenewalError> {
        assert_eq!(credential, "renewal-secret");
        thread::sleep(Duration::from_millis(10));
        Ok(InferenceCredential {
            access_token: self.access_token.clone(),
            expires_at_ms: 900_000,
        })
    }
}

#[test]
fn user_full_name_helpers_preserve_reference_normalization() {
    assert_eq!(non_empty(Some("  Gloria  ")).as_deref(), Some("Gloria"));
    assert_eq!(non_empty(Some("   ")), None);
    assert_eq!(
        display_name_from(Some(" Gloria "), Some(" Chan ")).as_deref(),
        Some("Gloria Chan"),
    );
    assert_eq!(display_name_from(None, Some("Chan")).as_deref(), Some("Chan"));
    assert_eq!(
        principal_from_access_token(&token("account-42")).as_deref(),
        Some("account-42"),
    );
}

#[test]
fn auth_extension_composes_service_identity_refresh_and_browser_ua_api() {
    assert_eq!(auth_extension_id(), HostExtensionId::Auth);
    assert!(AUTH_DEPENDENCIES.is_empty());

    let logs = Arc::new(Mutex::new(Vec::<String>::new()));
    let logs_sink = Arc::clone(&logs);
    let fetched = Arc::new(Mutex::new(Vec::<String>::new()));
    let fetched_sink = Arc::clone(&fetched);
    let access_token = token("account-42");

    let extension = start_host_auth_extension_with_options(
        HostAuthServiceOptions {
            environment: HostAuthEnvironment::from_pairs(BTreeMap::from([(
                SAND_INFERENCE_RENEWAL_CREDENTIAL_ENV.to_string(),
                "renewal-secret".to_string(),
            )])),
            backend_url: Some("http://127.0.0.1:9".into()),
            backend: Some(Arc::new(StaticBackend {
                access_token: access_token.clone(),
            })),
            machine_id: Arc::new(|| Ok("machine-42".into())),
            log: Arc::new(move |message| {
                logs_sink.lock().unwrap().push(message.to_string());
            }),
            now_ms: Arc::new(|| 100_000),
        },
        Arc::new(move |token| {
            fetched_sink.lock().unwrap().push(token.to_string());
            Ok(Some("  Gloria Chan  ".into()))
        }),
    )
    .expect("auth extension");

    for _ in 0..100 {
        if extension.get_user_full_name().is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }

    assert_eq!(extension.peek_access_token().as_deref(), Some(access_token.as_str()));
    assert_eq!(extension.get_access_token().unwrap(), access_token);
    assert_eq!(extension.get_machine_id().unwrap(), "machine-42");
    assert_eq!(extension.get_user_full_name().as_deref(), Some("Gloria Chan"));
    assert_eq!(fetched.lock().unwrap().len(), 1);

    let observed = Arc::new(Mutex::new(Vec::<String>::new()));
    let observed_sink = Arc::clone(&observed);
    let stop = BrowserUaAuthApi::subscribe_to_renewal(
        &extension,
        Arc::new(move |event| observed_sink.lock().unwrap().push(event.outcome)),
    );
    stop();

    assert!(
        logs.lock()
            .unwrap()
            .iter()
            .any(|message| message.contains("sole inference-credential source"))
    );
    extension.stop();
}
