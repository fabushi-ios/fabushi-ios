use std::fs;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::teach_recording::extension::{
    CAP_SLACK_MS, TEACH_QUEUE_KEY_FILENAME, TEACH_RECORDING_DEPENDENCIES,
    TeachRecordingService, TeachRecordingServiceDeps, TeachRecordingServiceFactory,
    create_teach_recording_extension, load_teach_queue_key_with, parse_teach_queue_key,
    recover_teach_recording_extension, start_teach_recording_extension,
};
use mahayana_host_runtime::extensions::teach_recording::teach_recording_service::SAND_TEACH_MAX_DURATION_MS;

#[test]
fn queue_key_parser_and_persistence_match_the_frozen_extension_contract() {
    let raw = r#"{"version":1,"keyHex":"000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"}"#;
    let parsed = parse_teach_queue_key(raw).unwrap();
    assert_eq!(parsed[0], 0);
    assert_eq!(parsed[31], 31);
    assert!(parse_teach_queue_key(r#"{"version":2,"keyHex":"00"}"#).is_none());
    assert!(parse_teach_queue_key(r#"{"version":1,"keyHex":"zz"}"#).is_none());

    let root = std::env::temp_dir().join(format!(
        "fabushi-teach-key-{}",
        uuid::Uuid::new_v4()
    ));
    let generated = load_teach_queue_key_with(&root, |buffer| {
        for (index, byte) in buffer.iter_mut().enumerate() {
            *byte = (255 - index) as u8;
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(generated[0], 255);
    assert_eq!(generated[31], 224);

    let second = load_teach_queue_key_with(&root, |_buffer| {
        Err(io::Error::other("must not regenerate a valid persisted key"))
    })
    .unwrap();
    assert_eq!(second, generated);

    let stored = fs::read_to_string(root.join(TEACH_QUEUE_KEY_FILENAME)).unwrap();
    assert_eq!(parse_teach_queue_key(&stored), Some(generated));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(root.join(TEACH_QUEUE_KEY_FILENAME))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    let _ = fs::remove_dir_all(root);
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Api(&'static str);

struct FakeService {
    api: Api,
    recover_calls: AtomicUsize,
    dispose_calls: Arc<AtomicUsize>,
    fail_recovery: bool,
}

impl TeachRecordingService for FakeService {
    type Api = Api;

    fn recover_pending(&self) -> Result<(), String> {
        self.recover_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_recovery {
            Err("recover failed".into())
        } else {
            Ok(())
        }
    }

    fn api(&self) -> Self::Api {
        self.api.clone()
    }

    fn dispose(&self) {
        self.dispose_calls.fetch_add(1, Ordering::SeqCst);
    }
}

struct FakeFactory {
    seen_cap_delay: Arc<Mutex<Option<u64>>>,
    dispose_calls: Arc<AtomicUsize>,
    fail_recovery: bool,
}

impl TeachRecordingServiceFactory for FakeFactory {
    type Service = FakeService;

    fn create(&self, deps: TeachRecordingServiceDeps) -> Self::Service {
        *self.seen_cap_delay.lock().unwrap() = Some(deps.cap_delay_ms);
        assert_eq!(TEACH_RECORDING_DEPENDENCIES, [
            "experiments",
            "forever-box",
            "managed-setup",
            "telemetry",
            "transcript",
        ]);
        assert!((deps.is_enabled)());
        assert_eq!((deps.list_agent_ids)().unwrap(), vec!["agent-1"]);
        assert!((deps.ensure_learning_workflow)().unwrap());
        FakeService {
            api: Api("teach-api"),
            recover_calls: AtomicUsize::new(0),
            dispose_calls: Arc::clone(&self.dispose_calls),
            fail_recovery: self.fail_recovery,
        }
    }
}

fn deps() -> TeachRecordingServiceDeps {
    TeachRecordingServiceDeps {
        is_enabled: Arc::new(|| true),
        cap_delay_ms: SAND_TEACH_MAX_DURATION_MS + CAP_SLACK_MS,
        send_learning_prompt: Arc::new(|_, _, _, _| Ok(())),
        list_agent_ids: Arc::new(|| Ok(vec!["agent-1".into()])),
        queue_signature_key: Arc::new(|| Ok([7; 32])),
        ensure_learning_workflow: Arc::new(|| Ok(true)),
        track_recording_started: Arc::new(|_| {}),
        track_recording_stopped: Arc::new(|_| {}),
        report_cap_stop_failed: Arc::new(|_| {}),
        report_start_failed: Arc::new(|_| {}),
    }
}

#[test]
fn extension_composes_service_recovers_pending_and_disposes_with_host_lifecycle() {
    let cap = Arc::new(Mutex::new(None));
    let dispose_calls = Arc::new(AtomicUsize::new(0));
    let factory = FakeFactory {
        seen_cap_delay: Arc::clone(&cap),
        dispose_calls: Arc::clone(&dispose_calls),
        fail_recovery: false,
    };
    let logs = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = {
        let logs = Arc::clone(&logs);
        Arc::new(move |message: &str| logs.lock().unwrap().push(message.into()))
    };

    let extension = start_teach_recording_extension(&factory, deps(), log);
    assert_eq!(extension.api(), Api("teach-api"));
    assert_eq!(*cap.lock().unwrap(), Some(SAND_TEACH_MAX_DURATION_MS + CAP_SLACK_MS));
    assert!(logs.lock().unwrap().is_empty());
    drop(extension);
    assert_eq!(dispose_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn production_composition_can_publish_api_before_pending_recovery() {
    let cap = Arc::new(Mutex::new(None));
    let dispose_calls = Arc::new(AtomicUsize::new(0));
    let factory = FakeFactory {
        seen_cap_delay: Arc::clone(&cap),
        dispose_calls: Arc::clone(&dispose_calls),
        fail_recovery: false,
    };
    let extension = create_teach_recording_extension(&factory, deps());

    assert_eq!(extension.api(), Api("teach-api"));
    assert_eq!(
        extension.service().recover_calls.load(Ordering::SeqCst),
        0,
        "shipping Host must be able to publish the API before recovery can re-enter sendPrompt"
    );

    recover_teach_recording_extension(&extension, Arc::new(|_| {}));
    assert_eq!(
        extension.service().recover_calls.load(Ordering::SeqCst),
        1
    );
    drop(extension);
    assert_eq!(dispose_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn pending_recovery_failure_is_best_effort_and_logged() {
    let cap = Arc::new(Mutex::new(None));
    let factory = FakeFactory {
        seen_cap_delay: cap,
        dispose_calls: Arc::new(AtomicUsize::new(0)),
        fail_recovery: true,
    };
    let logs = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = {
        let logs = Arc::clone(&logs);
        Arc::new(move |message: &str| logs.lock().unwrap().push(message.into()))
    };

    let _extension = start_teach_recording_extension(&factory, deps(), log);
    assert_eq!(
        logs.lock().unwrap().as_slice(),
        ["teach-recording: pending delivery recovery failed"]
    );
}
