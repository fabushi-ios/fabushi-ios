use std::collections::VecDeque;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use mahayana_host_runtime::extensions::automations::connect_unary::SandConnectError;
use mahayana_host_runtime::extensions::automations::listener_integrations::PlatformConnectionReader;
use mahayana_host_runtime::extensions::automations::listener_connect_watcher_production::ProductionListenerConnectWatcher;

struct MockConnectionReader {
    states: Mutex<VecDeque<bool>>,
}

impl MockConnectionReader {
    fn new(states: impl IntoIterator<Item = bool>) -> Arc<Self> {
        Arc::new(Self {
            states: Mutex::new(states.into_iter().collect()),
        })
    }
}

impl PlatformConnectionReader for MockConnectionReader {
    fn is_platform_connected(&self, _platform: &str) -> Result<bool, SandConnectError> {
        Ok(self
            .states
            .lock()
            .expect("states mutex")
            .pop_front()
            .unwrap_or(true))
    }
}

#[test]
fn production_watcher_requires_disconnected_then_connected_edge() {
    let reader = MockConnectionReader::new([false, true]);
    let (tx, rx) = mpsc::channel();
    let watcher = ProductionListenerConnectWatcher::start_with_interval(
        reader,
        Arc::new(move |agent_id, platform| {
            tx.send((agent_id.to_string(), platform.to_string()))
                .expect("callback send");
        }),
        Duration::from_millis(10),
    );
    watcher.watch("agent-1", "slack");

    let connected = rx
        .recv_timeout(Duration::from_millis(500))
        .expect("armed watcher should fire");
    assert_eq!(connected, ("agent-1".into(), "slack".into()));
    assert_eq!(watcher.pending_len(), 0);
    watcher.stop();
}

#[test]
fn production_watcher_does_not_resume_when_already_connected() {
    let reader = MockConnectionReader::new([true]);
    let (tx, rx) = mpsc::channel();
    let watcher = ProductionListenerConnectWatcher::start_with_interval(
        reader,
        Arc::new(move |agent_id, platform| {
            let _ = tx.send((agent_id.to_string(), platform.to_string()));
        }),
        Duration::from_millis(10),
    );
    watcher.watch("agent-1", "github");

    assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    assert_eq!(watcher.pending_len(), 0);
    watcher.stop();
}


#[test]
fn production_watcher_suspension_parks_polling_owner() {
    let source = include_str!("../src/extensions/automations/listener_connect_watcher_production.rs");
    assert!(
        source.contains("!watcher.is_suspended() && !watcher.pending().is_empty()"),
        "suspended listener watches must park the production polling owner"
    );
}
