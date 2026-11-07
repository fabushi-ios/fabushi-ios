use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use mahayana_host_runtime::extensions::session::box_handoff_service::{
    BoxHandoffDeps, BoxHandoffService, HandoffDecision, HandoffRequest,
    HandoffStartResult, HandoffTelemetry, HandoffTrigger, PendingHandoff,
    ScreenshotPayload, decide_box_hand_back,
};

#[test]
fn handback_decision_matches_frozen_trigger_defaults() {
    let pending = PendingHandoff {
        request_id: "request-1".into(),
        instruction: "take over".into(),
        snapshot_data_url: None,
    };
    assert_eq!(
        decide_box_hand_back(Some(&pending), &HandoffTrigger::Name("cancel".into())),
        HandoffDecision::End(
            mahayana_host_runtime::extensions::session::box_handoff_service::EndHandoffDecision {
                request_id: "request-1".into(),
                resolution: "cancelled".into(),
                trigger: "cancel".into(),
            }
        )
    );
    assert_eq!(
        decide_box_hand_back(
            Some(&pending),
            &HandoffTrigger::Detailed {
                resolution: None,
                trigger: None,
            }
        ),
        HandoffDecision::End(
            mahayana_host_runtime::extensions::session::box_handoff_service::EndHandoffDecision {
                request_id: "request-1".into(),
                resolution: "completed".into(),
                trigger: "unknown".into(),
            }
        )
    );
    assert_eq!(
        decide_box_hand_back(None, &HandoffTrigger::Name("done".into())),
        HandoffDecision::None
    );
}

#[test]
fn service_deduplicates_pending_requests_captures_snapshot_and_reports() {
    let statuses = Arc::new(Mutex::new(Vec::<String>::new()));
    let reports = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let tracks = Arc::new(Mutex::new(Vec::<(String, serde_json::Value)>::new()));
    let ended = Arc::new(Mutex::new(Vec::new()));

    let service = BoxHandoffService::new(BoxHandoffDeps {
        grab_screenshot: Some(Arc::new(|_| {
            Ok(Some(ScreenshotPayload::Bytes(b"abc".to_vec())))
        })),
        on_status_changed: Some({
            let statuses = Arc::clone(&statuses);
            Arc::new(move |agent_id, _pending| {
                statuses.lock().expect("statuses").push(agent_id.to_string());
            })
        }),
        on_ended: Some({
            let ended = Arc::clone(&ended);
            Arc::new(move |event| {
                ended.lock().expect("ended").push(event);
                Ok(())
            })
        }),
        report: Some({
            let reports = Arc::clone(&reports);
            Arc::new(move |event| reports.lock().expect("reports").push(event))
        }),
        track_event: Some({
            let tracks = Arc::clone(&tracks);
            Arc::new(move |name, properties| {
                tracks
                    .lock()
                    .expect("tracks")
                    .push((name.to_string(), properties));
            })
        }),
        timeout_ms: Some(100),
        ..BoxHandoffDeps::default()
    });

    let request = HandoffRequest {
        agent_id: "agent-a".into(),
        instruction: "Take control".into(),
        telemetry: HandoffTelemetry {
            reason: Some("captcha".into()),
            domain: Some("d".repeat(80)),
            idp_domain: Some("idp.example".into()),
        },
    };
    let first = service.start(request.clone());
    let request_id = match first {
        HandoffStartResult::Started { request_id } => request_id,
        other => panic!("expected started, got {other:?}"),
    };
    assert_eq!(
        service.start(request),
        HandoffStartResult::AlreadyPending {
            request_id: request_id.clone(),
            instruction: "Take control".into(),
        }
    );

    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if service
            .get("agent-a")
            .and_then(|pending| pending.snapshot_data_url)
            .is_some()
        {
            break;
        }
        assert!(Instant::now() < deadline, "snapshot was not captured");
        thread::sleep(Duration::from_millis(5));
    }

    assert_eq!(
        service
            .get("agent-a")
            .and_then(|pending| pending.snapshot_data_url)
            .as_deref(),
        Some("data:image/webp;base64,YWJj")
    );

    let deadline = Instant::now() + Duration::from_secs(1);
    while reports.lock().expect("reports").is_empty() {
        assert!(Instant::now() < deadline, "capture report was not emitted");
        thread::sleep(Duration::from_millis(5));
    }
    let tracked = tracks.lock().expect("tracks");
    assert_eq!(tracked[0].0, "sand.box_help");
    assert_eq!(tracked[0].1["reason"], "captcha");
    assert_eq!(
        tracked[0].1["domain"].as_str().map(str::len),
        Some(64)
    );
    drop(tracked);

    assert!(service
        .end("agent-a", HandoffTrigger::Name("cancel".into()))
        .expect("end"));
    assert!(service.get("agent-a").is_none());
    assert_eq!(
        ended.lock().expect("ended")[0].resolution,
        "cancelled"
    );
    assert!(statuses.lock().expect("statuses").len() >= 3);
}

#[test]
fn stale_snapshot_cannot_overwrite_a_new_handoff() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let reports = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (first_started_tx, first_started_rx) = std::sync::mpsc::sync_channel(1);
    let (release_first_tx, release_first_rx) = std::sync::mpsc::sync_channel(1);
    let release_first_rx = Arc::new(Mutex::new(release_first_rx));

    let service = BoxHandoffService::new(BoxHandoffDeps {
        grab_screenshot: Some({
            let calls = Arc::clone(&calls);
            let release_first_rx = Arc::clone(&release_first_rx);
            Arc::new(move |_| {
                let call = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if call == 0 {
                    first_started_tx.send(()).expect("signal first capture");
                    release_first_rx
                        .lock()
                        .expect("release receiver")
                        .recv_timeout(Duration::from_secs(1))
                        .expect("release first capture");
                    Ok(Some(ScreenshotPayload::Base64("b2xk".into())))
                } else {
                    Ok(None)
                }
            })
        }),
        report: Some({
            let reports = Arc::clone(&reports);
            Arc::new(move |_| {
                reports.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
        }),
        timeout_ms: Some(500),
        ..BoxHandoffDeps::default()
    });

    let request = || HandoffRequest {
        agent_id: "agent-race".into(),
        instruction: "Take control".into(),
        telemetry: HandoffTelemetry::default(),
    };

    assert!(matches!(
        service.start(request()),
        HandoffStartResult::Started { .. }
    ));
    first_started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("first capture entered screenshot callback");

    service.forget("agent-race");
    let second_id = match service.start(request()) {
        HandoffStartResult::Started { request_id } => request_id,
        other => panic!("expected second start, got {other:?}"),
    };
    release_first_tx.send(()).expect("release stale capture");

    let deadline = Instant::now() + Duration::from_secs(1);
    while reports.load(std::sync::atomic::Ordering::SeqCst) < 2 {
        assert!(Instant::now() < deadline, "both capture attempts did not settle");
        thread::sleep(Duration::from_millis(5));
    }

    let pending = service.get("agent-race").expect("second pending");
    assert_eq!(pending.request_id, second_id);
    assert_eq!(pending.snapshot_data_url, None);
}
