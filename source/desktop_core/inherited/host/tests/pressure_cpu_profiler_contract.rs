use std::collections::BTreeMap;
use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::experiments::{
    HostExperimentsExtension, HostExperimentsOptions,
};
use mahayana_host_runtime::extensions::telemetry::event_loop_telemetry::{
    EventLoopTelemetryConfig, EventLoopTelemetryRuntime, EventLoopTrigger,
};
use mahayana_host_runtime::extensions::telemetry::host_telemetry_service::{
    HostTelemetryService, StructuredLogFlushPolling, route_pressure_cpu_profiler_event,
    tick_pressure_cpu_profiler,
};
use mahayana_host_runtime::extensions::telemetry::pressure_cpu_profiler::{
    CpuProfileArtifact, CpuProfilerBackend, PressureCpuProfiler, PressureCpuProfilerKnobs,
    ProductionCpuProfilerBackend, create_pressure_cpu_profiler_with_backend,
};

#[derive(Default)]
struct Backend {
    starts: AtomicUsize,
    stops: AtomicUsize,
    disposes: AtomicUsize,
    profile: Mutex<String>,
}

impl CpuProfilerBackend for Backend {
    fn start(&self) -> Result<(), String> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn stop(&self) -> Result<CpuProfileArtifact, String> {
        self.stops.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "fabushi-pressure-backend-{}.cpuprofile",
            uuid::Uuid::new_v4()
        ));
        fs::write(&path, self.profile.lock().unwrap().as_bytes())
            .map_err(|error| error.to_string())?;
        Ok(CpuProfileArtifact::new(path, "cpuprofile"))
    }

    fn dispose(&self) {
        self.disposes.fetch_add(1, Ordering::SeqCst);
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn experiments() -> Arc<HostExperimentsExtension> {
    Arc::new(HostExperimentsExtension::new(HostExperimentsOptions {
        is_dev_build: false,
        env_gate_overrides: None,
    }))
}

#[test]
fn sustained_pressure_is_required_and_min_interval_is_enforced() {
    let root = std::env::temp_dir().join(format!(
        "fabushi-pressure-profiler-{}",
        uuid::Uuid::new_v4()
    ));
    let backend = Arc::new(Backend::default());
    *backend.profile.lock().unwrap() = "{\"nodes\":[]}".into();
    let captured_count = Arc::new(AtomicUsize::new(0));
    let captured_for_listener = Arc::clone(&captured_count);
    let mut profiler = PressureCpuProfiler::new(
        &root,
        backend.clone(),
        PressureCpuProfilerKnobs {
            sustained_pressure_window_ms: Some(100),
            profile_duration_ms: Some(50),
            min_interval_ms: Some(1_000),
            max_retained_profiles: Some(2),
        },
    )
    .with_on_captured(Arc::new(move |path| {
        assert!(path.exists());
        captured_for_listener.fetch_add(1, Ordering::SeqCst);
    }));

    profiler.on_pressure(0);
    assert_eq!(backend.starts.load(Ordering::SeqCst), 0);
    profiler.on_pressure(50);
    assert_eq!(backend.starts.load(Ordering::SeqCst), 1);
    assert!(profiler.is_profiling());
    assert!(profiler.on_tick(99).unwrap().is_none());
    let captured = profiler.on_tick(100).unwrap().unwrap();
    assert!(captured.exists());
    assert_eq!(captured.extension().and_then(|value| value.to_str()), Some("cpuprofile"));
    assert_eq!(backend.stops.load(Ordering::SeqCst), 1);
    assert_eq!(captured_count.load(Ordering::SeqCst), 1);

    profiler.on_pressure(200);
    profiler.on_pressure(250);
    assert_eq!(backend.starts.load(Ordering::SeqCst), 1);

    profiler.on_pressure(1_100);
    profiler.on_pressure(1_150);
    assert_eq!(backend.starts.load(Ordering::SeqCst), 2);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn retention_keeps_latest_profiles_and_dispose_is_idempotent() {
    let root =
        std::env::temp_dir().join(format!("fabushi-pressure-retain-{}", uuid::Uuid::new_v4()));
    let backend = Arc::new(Backend::default());
    *backend.profile.lock().unwrap() = "{}".into();
    let mut profiler = PressureCpuProfiler::new(
        &root,
        backend.clone(),
        PressureCpuProfilerKnobs {
            sustained_pressure_window_ms: Some(100),
            profile_duration_ms: Some(0),
            min_interval_ms: Some(0),
            max_retained_profiles: Some(2),
        },
    );
    for base in [100_i64, 300, 500] {
        profiler.on_pressure(base);
        profiler.on_pressure(base + 1);
        profiler.on_tick(base + 1).unwrap();
    }
    let names = fs::read_dir(&root)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(names.len(), 2);
    assert!(!names.iter().any(|name| name.contains("101")));
    profiler.on_pressure(1_000);
    profiler.on_pressure(1_001);
    assert!(profiler.is_profiling());
    profiler.dispose();
    profiler.dispose();
    assert_eq!(backend.disposes.load(Ordering::SeqCst), 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn live_overrides_are_fail_closed_and_max_retention_is_at_least_one() {
    let root = std::env::temp_dir().join(format!(
        "fabushi-pressure-overrides-{}",
        uuid::Uuid::new_v4()
    ));
    let backend = Arc::new(Backend::default());
    *backend.profile.lock().unwrap() = "{}".into();
    let mut profiler = PressureCpuProfiler::new(
        &root,
        backend,
        PressureCpuProfilerKnobs {
            sustained_pressure_window_ms: Some(100),
            profile_duration_ms: Some(0),
            min_interval_ms: Some(0),
            max_retained_profiles: Some(3),
        },
    )
    .with_overrides(Arc::new(|| {
        Some(PressureCpuProfilerKnobs {
            max_retained_profiles: Some(0),
            ..Default::default()
        })
    }));
    for base in [10_i64, 20, 30] {
        profiler.on_pressure(base);
        profiler.on_pressure(base + 1);
        profiler.on_tick(base + 1).unwrap();
    }
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn production_owner_consumes_live_experiment_knobs_functionally() {
    let root = std::env::temp_dir().join(format!(
        "fabushi-pressure-live-knobs-{}",
        uuid::Uuid::new_v4()
    ));
    let experiments = experiments();
    let bootstrap = serde_json::json!({
        "user": { "userID": "pressure-test" },
        "feature_gates": {},
        "dynamic_configs": {
            "sand_pressure_cpu_profiler_config": {
                "value": {
                    "sustainedPressureWindowMs": 100,
                    "profileDurationMs": 0,
                    "minIntervalMs": 0,
                    "maxRetainedProfiles": 1
                }
            }
        }
    })
    .to_string();
    assert!(experiments.hydrate_statsig_bootstrap(&bootstrap, true).unwrap());

    let backend = Arc::new(Backend::default());
    *backend.profile.lock().unwrap() = "{\"nodes\":[]}".into();
    let profiler = create_pressure_cpu_profiler_with_backend(
        Arc::clone(&experiments),
        &root,
        backend.clone(),
    );
    {
        let mut profiler = profiler.lock().unwrap();
        profiler.on_pressure(100);
        profiler.on_pressure(150);
        assert!(profiler.is_profiling());
        profiler.on_tick(150).unwrap();
    }
    assert_eq!(backend.starts.load(Ordering::SeqCst), 1);
    assert_eq!(backend.stops.load(Ordering::SeqCst), 1);
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_pressure_event_and_flush_tick_execute_the_same_profiler_owner() {
    let root = std::env::temp_dir().join(format!(
        "fabushi-pressure-shipping-wiring-{}",
        uuid::Uuid::new_v4()
    ));
    let experiments = experiments();
    experiments.replace_feature_flag_overrides(BTreeMap::from([(
        "sand_enable_pressure_cpu_profiler".into(),
        true,
    )]));

    let backend = Arc::new(Backend::default());
    *backend.profile.lock().unwrap() = "{\"nodes\":[]}".into();
    let profiler = Arc::new(Mutex::new(PressureCpuProfiler::new(
        &root,
        backend.clone(),
        PressureCpuProfilerKnobs {
            sustained_pressure_window_ms: Some(1_000),
            profile_duration_ms: Some(0),
            min_interval_ms: Some(10_000),
            max_retained_profiles: Some(2),
        },
    )));

    let (report_tx, report_rx) = mpsc::channel();
    let pressure_for_reports = Arc::clone(&profiler);
    let experiments_for_reports = Arc::clone(&experiments);
    let event_loop = EventLoopTelemetryRuntime::start_with_config(
        EventLoopTelemetryConfig {
            resolution_ms: 5,
            window_ms: 25,
            pressure_p95_ms: -1.0,
            heartbeat_every_n_windows: 0,
        },
        Arc::new(move |report| {
            route_pressure_cpu_profiler_event(
                &experiments_for_reports,
                Some(&pressure_for_reports),
                report,
                now_ms(),
            );
            let _ = report_tx.send(report.trigger);
        }),
    );

    for _ in 0..2 {
        assert_eq!(
            report_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            EventLoopTrigger::Pressure
        );
    }

    let deadline = SystemTime::now() + Duration::from_secs(2);
    while backend.starts.load(Ordering::SeqCst) == 0 && SystemTime::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(backend.starts.load(Ordering::SeqCst), 1);

    let telemetry =
        HostTelemetryService::open(root.join("host-events.jsonl")).expect("telemetry service");
    let pressure_for_tick = Arc::clone(&profiler);
    let flush = StructuredLogFlushPolling::start_with_interval(
        telemetry.logs.clone(),
        Some(Arc::new(move || {
            let _ = tick_pressure_cpu_profiler(Some(&pressure_for_tick), now_ms());
        })),
        Duration::from_millis(20),
    );

    let deadline = SystemTime::now() + Duration::from_secs(2);
    while backend.stops.load(Ordering::SeqCst) == 0 && SystemTime::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(backend.stops.load(Ordering::SeqCst), 1);
    // Backend stop is an internal capture phase. Reacquiring the shared profiler owner
    // waits for the flush-tick call to finish persisting the final artifact before the
    // publication assertion below, without weakening the shipping ownership contract.
    assert!(
        !profiler.lock().unwrap().is_profiling(),
        "flush tick must settle the shared profiler owner before artifact publication"
    );
    assert_eq!(
        fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("sand-host-pressure-"))
            .count(),
        1
    );

    drop(flush);
    drop(event_loop);
    profiler.lock().unwrap().dispose();
    let _ = fs::remove_dir_all(root);
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
#[test]
fn production_backend_emits_native_consumable_cpu_profile_artifact() {
    let backend = ProductionCpuProfilerBackend::default();
    backend.start().expect("start production CPU profiler");
    assert!(backend.start().is_err(), "duplicate start must fail closed");
    thread::sleep(Duration::from_millis(750));
    let artifact = backend.stop().expect("stop production CPU profiler");

    let metadata = fs::metadata(&artifact.path).expect("captured native artifact");
    assert!(metadata.is_file());
    assert!(metadata.len() > 0);

    #[cfg(target_os = "macos")]
    {
        assert_eq!(artifact.extension, "sample.txt");
        let text = fs::read_to_string(&artifact.path).expect("macOS sample profile");
        assert!(text.contains("Call graph"));
    }

    #[cfg(target_os = "linux")]
    {
        assert_eq!(artifact.extension, "perf.data");
        let status = Command::new("perf")
            .args(["report", "--stdio", "--header-only", "-i"])
            .arg(&artifact.path)
            .status()
            .expect("validate perf artifact");
        assert!(status.success(), "perf must consume its captured artifact");
    }

    #[cfg(target_os = "windows")]
    {
        assert_eq!(artifact.extension, "etl");
    }

    fs::remove_file(&artifact.path).expect("remove native artifact");
    assert!(backend.stop().is_err(), "stop without start must fail closed");
    backend.dispose();
    backend.dispose();
}
