use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use mahayana_host_runtime::extensions::browser_ua::extension::StopSubscription;
use mahayana_host_runtime::extensions::notify_bus::extension::{
    NotifyBusExperimentsApi, NotifyBusExtensionOptions,
    SAFETY_POLL_DEFAULT_BEFORE_GATE_RESOLVES, start_notify_bus_extension_with_options,
};
use mahayana_host_runtime::extensions::notify_bus::notify_bus_client::{
    NotifyBusTiming, SandNotifyTopic,
};

#[derive(Default)]
struct TestExperiments {
    gates: Mutex<BTreeMap<String, bool>>,
    listeners: Mutex<Vec<Arc<dyn Fn() + Send + Sync>>>,
}

impl TestExperiments {
    fn set(&self, name: &str, value: bool) {
        self.gates.lock().unwrap().insert(name.into(), value);
        for listener in self.listeners.lock().unwrap().clone() {
            listener();
        }
    }
}

impl NotifyBusExperimentsApi for TestExperiments {
    fn check_feature_gate(&self, name: &str) -> bool {
        self.gates.lock().unwrap().get(name).copied().unwrap_or(false)
    }

    fn subscribe(&self, listener: Arc<dyn Fn() + Send + Sync>) -> StopSubscription {
        self.listeners.lock().unwrap().push(listener);
        Box::new(|| {})
    }
}

#[test]
fn background_ready_gates_start_connected_wakes_all_topics_and_safety_gate_updates() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let accepted = Arc::new(AtomicBool::new(false));
    let accepted_for_server = Arc::clone(&accepted);
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        accepted_for_server.store(true, Ordering::SeqCst);
        let mut request = [0u8; 4096];
        let _ = stream.read(&mut request).unwrap();
        let body = b"data: {\"kind\":\"connected\"}\n\ndata: {\"kind\":\"notify\",\"topic\":\"automation-fires\"}\n\n";
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
        stream.flush().unwrap();
    });

    let experiments = Arc::new(TestExperiments::default());
    experiments.set("sand_notify_bus", true);
    experiments.set("sand_notify_safety_poll", false);

    let backend = format!("http://{addr}/");
    let extension = start_notify_bus_extension_with_options(
        experiments.clone(),
        NotifyBusExtensionOptions {
            get_backend_url: Arc::new(move || Ok(backend.clone())),
            get_access_token: Arc::new(|_| Ok("token".into())),
            now_ms: Arc::new(|| 1_000),
            timing: NotifyBusTiming {
                healthy_connection_min_lifetime_ms: 30_000,
                reconnect_initial_delay_ms: 5_000,
                reconnect_max_delay_ms: 5_000,
                stall_ms: 1_000,
            },
            log: Arc::new(|_| {}),
        },
    )
    .unwrap();

    assert_eq!(
        extension.is_safety_poll_enabled(),
        SAFETY_POLL_DEFAULT_BEFORE_GATE_RESOLVES
    );
    thread::sleep(Duration::from_millis(50));
    assert!(!accepted.load(Ordering::SeqCst));

    let automation = Arc::new(AtomicUsize::new(0));
    let listener_events = Arc::new(AtomicUsize::new(0));
    let xuser = Arc::new(AtomicUsize::new(0));
    let a = Arc::clone(&automation);
    let l = Arc::clone(&listener_events);
    let x = Arc::clone(&xuser);
    let _a_stop = extension.on_notify(
        SandNotifyTopic::AutomationFires,
        Arc::new(move || {
            a.fetch_add(1, Ordering::SeqCst);
        }),
    );
    let _l_stop = extension.on_notify(
        SandNotifyTopic::ListenerEvents,
        Arc::new(move || {
            l.fetch_add(1, Ordering::SeqCst);
        }),
    );
    let _x_stop = extension.on_notify(
        SandNotifyTopic::XuserEvents,
        Arc::new(move || {
            x.fetch_add(1, Ordering::SeqCst);
        }),
    );

    extension.mark_background_work_ready();
    assert!(!extension.is_safety_poll_enabled());

    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline
        && (automation.load(Ordering::SeqCst) < 2
            || listener_events.load(Ordering::SeqCst) < 1
            || xuser.load(Ordering::SeqCst) < 1)
    {
        thread::sleep(Duration::from_millis(10));
    }

    extension.stop();
    server.join().unwrap();
    assert!(accepted.load(Ordering::SeqCst));
    assert_eq!(automation.load(Ordering::SeqCst), 2);
    assert_eq!(listener_events.load(Ordering::SeqCst), 1);
    assert_eq!(xuser.load(Ordering::SeqCst), 1);
}

#[test]
fn gate_changes_after_ready_start_and_stop_client_and_handler_unsubscribe_is_exact() {
    let experiments = Arc::new(TestExperiments::default());
    let extension = start_notify_bus_extension_with_options(
        experiments.clone(),
        NotifyBusExtensionOptions {
            get_backend_url: Arc::new(|| Ok("http://127.0.0.1:9/".into())),
            get_access_token: Arc::new(|_| Ok("token".into())),
            now_ms: Arc::new(|| 1_000),
            timing: NotifyBusTiming {
                healthy_connection_min_lifetime_ms: 30_000,
                reconnect_initial_delay_ms: 50,
                reconnect_max_delay_ms: 50,
                stall_ms: 50,
            },
            log: Arc::new(|_| {}),
        },
    )
    .unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_handler = Arc::clone(&calls);
    let stop = extension.on_notify(
        SandNotifyTopic::AutomationFires,
        Arc::new(move || {
            calls_for_handler.fetch_add(1, Ordering::SeqCst);
        }),
    );
    stop();

    extension.mark_background_work_ready();
    experiments.set("sand_notify_bus", true);
    thread::sleep(Duration::from_millis(30));
    experiments.set("sand_notify_bus", false);
    extension.stop();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
