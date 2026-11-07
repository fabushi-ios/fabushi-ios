use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, AtomicUsize, Ordering},
    mpsc,
};
use std::thread;
use std::time::Duration;

use mahayana_host_runtime::extensions::extension_ids_generated::HostExtensionId;
use mahayana_host_runtime::extensions::telemetry::webauthn_proxy_telemetry::{
    WebAuthnFailureCause, WebAuthnProxyReport,
};
use mahayana_host_runtime::extensions::webauthn_proxy::extension::{
    WEBAUTHN_PROXY_DEPENDENCIES, start_webauthn_proxy_extension, webauthn_proxy_extension_id,
};
use mahayana_host_runtime::extensions::webauthn_proxy::webauthn_proxy_bridge::{
    SAND_NO_WEBAUTHN_MACHINE_MESSAGE, SAND_WEBAUTHN_CEREMONY_TIMEOUT_MESSAGE,
    SAND_WEBAUTHN_MACHINE_UNAVAILABLE_MESSAGE, SandWebAuthnBridge,
    SandWebAuthnBridgeOptions, sand_webauthn_origin_class,
};
use serde_json::{Value, json};

fn report_sink() -> (
    Arc<Mutex<Vec<WebAuthnProxyReport>>>,
    Arc<dyn Fn(WebAuthnProxyReport) + Send + Sync>,
) {
    let reports = Arc::new(Mutex::new(Vec::new()));
    let sink_reports = Arc::clone(&reports);
    let sink: Arc<dyn Fn(WebAuthnProxyReport) + Send + Sync> =
        Arc::new(move |report| sink_reports.lock().unwrap().push(report));
    (reports, sink)
}

#[test]
fn webauthn_proxy_extension_owns_gateway_provider_lifecycle_and_settlement() {
    assert_eq!(webauthn_proxy_extension_id(), HostExtensionId::WebauthnProxy);
    assert_eq!(WEBAUTHN_PROXY_DEPENDENCIES, &[HostExtensionId::Telemetry]);
    assert_eq!(sand_webauthn_origin_class("https://cursor.com"), "cursor_com");
    assert_eq!(
        sand_webauthn_origin_class("https://agent.cursor.com"),
        "subdomain"
    );
    assert_eq!(sand_webauthn_origin_class("https://example.com"), "external");

    let (reports, sink) = report_sink();
    let extension = Arc::new(start_webauthn_proxy_extension(sink));
    let gateway = extension.gateway_bridge();
    let provider = gateway.subscribe_requests();

    let welcome = provider
        .recv_timeout(Duration::from_secs(1))
        .expect("provider welcome");
    let provider_id = welcome["providerId"]
        .as_str()
        .expect("provider id")
        .to_string();
    assert_eq!(welcome["kind"], "welcome");
    assert_eq!(extension.provider_counts(), (1, 1));

    let request_extension = Arc::clone(&extension);
    let request = thread::spawn(move || {
        request_extension
            .request_ceremony(json!({
                "kind": "get",
                "origin": "https://login.cursor.com",
                "publicKey": { "challenge": "abc" }
            }))
            .expect("ceremony request")
    });

    let ceremony = provider
        .recv_timeout(Duration::from_secs(1))
        .expect("ceremony frame");
    assert_eq!(ceremony["kind"], "ceremony");
    assert_eq!(ceremony["ceremony"]["origin"], "https://login.cursor.com");
    let request_id = ceremony["requestId"].as_str().expect("request id").to_string();

    gateway.submit_responses(json!({
        "providerId": provider_id,
        "frames": [
            { "kind": "hello", "computerId": "computer-1", "label": "Desktop" },
            { "kind": "ping" },
            { "kind": "stage", "requestId": request_id, "stage": "grant", "outcome": "ok" },
            { "kind": "stage", "requestId": request_id, "stage": "sign", "outcome": "ok" },
            {
                "kind": "result",
                "requestId": request_id,
                "credentialJson": { "id": "credential-1" }
            }
        ]
    }));

    let result = request.join().expect("request thread");
    assert_eq!(
        result,
        json!({ "ok": true, "credentialJson": { "id": "credential-1" } })
    );
    let reports = reports.lock().unwrap();
    assert!(reports.iter().any(|report| report.stage == "request" && report.outcome == "ok"));
    assert!(reports.iter().any(|report| report.stage == "grant" && report.outcome == "ok"));
    assert!(reports.iter().any(|report| report.stage == "sign" && report.outcome == "ok"));
    assert!(reports.iter().any(|report| report.stage == "complete" && report.outcome == "ok"));
    drop(reports);

    drop(provider);
    assert_eq!(extension.provider_counts(), (0, 0));
}

#[test]
fn webauthn_proxy_distinguishes_no_provider_and_stale_provider() {
    let now = Arc::new(AtomicU64::new(1_000));
    let next_id = Arc::new(AtomicUsize::new(0));
    let (reports, sink) = report_sink();
    let bridge = SandWebAuthnBridge::new(SandWebAuthnBridgeOptions {
        ceremony_timeout: Duration::from_secs(1),
        liveness_window: Duration::from_millis(30_000),
        now_ms: {
            let now = Arc::clone(&now);
            Arc::new(move || now.load(Ordering::SeqCst))
        },
        create_id: {
            let next_id = Arc::clone(&next_id);
            Arc::new(move || format!("id-{}", next_id.fetch_add(1, Ordering::SeqCst)))
        },
        report: sink,
    });

    let no_provider = bridge
        .request_ceremony(json!({"kind":"get","origin":"https://example.com"}))
        .expect("no-provider result");
    assert_eq!(no_provider["ok"], false);
    assert_eq!(
        no_provider["error"]["message"],
        SAND_NO_WEBAUTHN_MACHINE_MESSAGE
    );

    let (send, receive) = mpsc::channel::<Value>();
    let _registration = bridge.register_provider(send);
    let welcome = receive
        .recv_timeout(Duration::from_secs(1))
        .expect("provider welcome");
    let provider_id = welcome["providerId"].as_str().expect("provider id");
    bridge.submit_responses(json!({
        "providerId": provider_id,
        "frames": [{ "kind": "ping" }]
    }));
    now.store(31_001, Ordering::SeqCst);

    let stale = bridge
        .request_ceremony(json!({"kind":"get","origin":"https://example.com"}))
        .expect("stale-provider result");
    assert_eq!(stale["ok"], false);
    assert_eq!(
        stale["error"]["message"],
        SAND_WEBAUTHN_MACHINE_UNAVAILABLE_MESSAGE
    );

    let reports = reports.lock().unwrap();
    assert!(reports.iter().any(|report| report.cause == Some(WebAuthnFailureCause::NoProvider)));
    assert!(reports.iter().any(|report| report.cause == Some(WebAuthnFailureCause::ProviderStale)));
}

#[test]
fn webauthn_proxy_deadline_sends_cancel_and_returns_not_allowed() {
    let (reports, sink) = report_sink();
    let next_id = Arc::new(AtomicUsize::new(0));
    let bridge = Arc::new(SandWebAuthnBridge::new(SandWebAuthnBridgeOptions {
        ceremony_timeout: Duration::from_millis(25),
        liveness_window: Duration::from_secs(30),
        now_ms: Arc::new(|| 1_000),
        create_id: {
            let next_id = Arc::clone(&next_id);
            Arc::new(move || format!("deadline-{}", next_id.fetch_add(1, Ordering::SeqCst)))
        },
        report: sink,
    }));
    let (send, receive) = mpsc::channel::<Value>();
    let _registration = bridge.register_provider(send);
    receive.recv_timeout(Duration::from_secs(1)).expect("welcome");

    let request_bridge = Arc::clone(&bridge);
    let request = thread::spawn(move || {
        request_bridge
            .request_ceremony(json!({"kind":"create","origin":"https://cursor.com"}))
            .expect("timeout result")
    });
    let ceremony = receive
        .recv_timeout(Duration::from_secs(1))
        .expect("ceremony");
    let request_id = ceremony["requestId"].as_str().expect("request id").to_string();

    let result = request.join().expect("timeout thread");
    assert_eq!(result["ok"], false);
    assert_eq!(
        result["error"]["message"],
        SAND_WEBAUTHN_CEREMONY_TIMEOUT_MESSAGE
    );
    let cancel = receive
        .recv_timeout(Duration::from_secs(1))
        .expect("cancel frame");
    assert_eq!(cancel, json!({ "kind": "cancel", "requestId": request_id }));

    let reports = reports.lock().unwrap();
    assert!(reports.iter().any(|report| {
        report.stage == "complete"
            && report.outcome == "timeout"
            && report.cause == Some(WebAuthnFailureCause::Timeout)
    }));
}
