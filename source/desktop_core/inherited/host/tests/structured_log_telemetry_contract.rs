use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mahayana_host_runtime::extensions::telemetry::structured_log_telemetry::{
    BufferedStructuredLog, ProductionStructuredLogTransport, STRUCTURED_LOG_MAX_BUFFER_SIZE,
    StructuredLogBackend, StructuredLogLevel, StructuredLogReceipt,
};

#[derive(Default)]
struct FakeBackend {
    attempts: Mutex<Vec<Vec<BufferedStructuredLog>>>,
    failures_remaining: Mutex<usize>,
}

impl FakeBackend {
    fn fail_next(count: usize) -> Self {
        Self {
            attempts: Mutex::new(Vec::new()),
            failures_remaining: Mutex::new(count),
        }
    }

    fn attempts(&self) -> Vec<Vec<BufferedStructuredLog>> {
        self.attempts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl StructuredLogBackend for FakeBackend {
    fn submit(&self, logs: &[BufferedStructuredLog]) -> Result<StructuredLogReceipt, String> {
        self.attempts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(logs.to_vec());
        let mut failures = self
            .failures_remaining
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *failures > 0 {
            *failures -= 1;
            return Err("synthetic submit failure".into());
        }
        Ok(StructuredLogReceipt {
            logs_processed: logs.len(),
            logs_dropped: 0,
        })
    }
}

#[test]
fn identity_hold_delays_shipping_until_host_bundle_identity_arrives() {
    let backend = Arc::new(FakeBackend::default());
    let transport = ProductionStructuredLogTransport::start_with_policy(
        backend.clone(),
        true,
        Duration::from_millis(20),
        Duration::from_secs(2),
    );

    assert!(transport.enqueue(
        StructuredLogLevel::Info,
        "sand.identity.contract",
        BTreeMap::from([
            ("auth_id".into(), "event-auth".into()),
            ("detail".into(), "ok".into()),
        ]),
    ));
    std::thread::sleep(Duration::from_millis(60));
    assert!(
        backend.attempts().is_empty(),
        "identity hold must block polling flush"
    );

    assert!(transport.set_identity_tags(BTreeMap::from([
        ("auth_id".into(), "box-auth".into()),
        ("box_store_id".into(), "store-a".into()),
        ("empty".into(), String::new()),
    ])));
    assert!(transport.flush());

    let attempts = backend.attempts();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].len(), 1);
    let entry = &attempts[0][0];
    assert_eq!(entry.message, "sand.identity.contract");
    assert_eq!(
        entry.metadata.get("auth_id").map(String::as_str),
        Some("event-auth")
    );
    assert_eq!(
        entry.metadata.get("box_store_id").map(String::as_str),
        Some("store-a")
    );
    assert_eq!(entry.metadata.get("detail").map(String::as_str), Some("ok"));
    assert!(!entry.metadata.contains_key("empty"));

    assert!(transport.dispose());
    assert!(transport.dispose(), "dispose must be idempotent");
}

#[test]
fn failed_batch_is_retried_and_dispose_releases_identity_hold() {
    let backend = Arc::new(FakeBackend::fail_next(1));
    let transport = ProductionStructuredLogTransport::start_with_policy(
        backend.clone(),
        true,
        Duration::from_secs(60),
        Duration::from_secs(60),
    );

    assert!(transport.enqueue(
        StructuredLogLevel::Warn,
        "sand.retry.contract",
        BTreeMap::from([("attempt".into(), "1".into())]),
    ));

    assert!(transport.set_identity_tags(BTreeMap::from([(
        "host_bundle_version".into(),
        "0.18-test".into(),
    )])));
    assert!(
        transport.flush(),
        "second submit should deliver the requeued batch"
    );

    let attempts = backend.attempts();
    assert_eq!(
        attempts.len(),
        2,
        "failed batch must be preserved for retry"
    );
    assert_eq!(attempts[0][0].message, "sand.retry.contract");
    assert_eq!(attempts[1][0].message, "sand.retry.contract");

    assert!(transport.dispose());

    let backend = Arc::new(FakeBackend::default());
    let transport = ProductionStructuredLogTransport::start_with_policy(
        backend.clone(),
        true,
        Duration::from_secs(60),
        Duration::from_secs(60),
    );
    assert!(transport.enqueue(
        StructuredLogLevel::Error,
        "sand.dispose.contract",
        BTreeMap::new(),
    ));
    assert!(transport.dispose(), "dispose must release hold and drain");
    let attempts = backend.attempts();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0][0].message, "sand.dispose.contract");
}

#[test]
fn buffer_is_bounded_and_confirmed_ship_bypasses_background_queue() {
    let backend = Arc::new(FakeBackend::default());
    let transport = ProductionStructuredLogTransport::start_with_policy(
        backend.clone(),
        true,
        Duration::from_secs(60),
        Duration::from_secs(60),
    );

    for index in 0..(STRUCTURED_LOG_MAX_BUFFER_SIZE + 25) {
        assert!(transport.enqueue(
            StructuredLogLevel::Info,
            format!("sand.buffer.{index}"),
            BTreeMap::new(),
        ));
    }

    assert!(transport.set_identity_tags(BTreeMap::new()));
    assert!(transport.flush());

    let submitted = backend.attempts().into_iter().flatten().collect::<Vec<_>>();
    assert_eq!(submitted.len(), STRUCTURED_LOG_MAX_BUFFER_SIZE);
    assert_eq!(submitted[0].message, "sand.buffer.25");

    assert!(transport.ship_confirmed(
        backend.as_ref(),
        StructuredLogLevel::Error,
        "sand.confirmed.contract",
        BTreeMap::from([("kind".into(), "fatal".into())]),
    ));
    let attempts = backend.attempts();
    assert_eq!(
        attempts.last().unwrap()[0].message,
        "sand.confirmed.contract"
    );

    assert!(transport.dispose());
}
