use std::collections::BTreeMap;
use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::telemetry::bot_block_telemetry::{
    BOT_BLOCK_DETAIL_EVENT, BOT_BLOCK_EVENT, BotBlockReport,
};
use mahayana_host_runtime::extensions::telemetry::host_telemetry_service::{
    HostTelemetryService, PersistedHostTelemetryRecord,
};
use mahayana_host_runtime::runner::bot_block_detection::{
    with_bot_block_detection_sink,
};
use mahayana_host_runtime::runner::sand_action_audit::{
    ActionAuditRecord, ActionAuditSink,
};
use serde_json::json;

fn temp_root(label: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "fabushi-bot-block-{label}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&root).expect("root");
    root
}

fn persisted(service: &HostTelemetryService) -> Vec<PersistedHostTelemetryRecord> {
    fs::read_to_string(service.records_path())
        .expect("jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).expect("record"))
        .collect()
}

#[test]
fn real_action_audit_decorator_records_base_before_block_hit_and_writes_host_logs() {
    let root = temp_root("producer");
    let service = Arc::new(
        HostTelemetryService::open_with_identity_tags(
            root.join("events.jsonl"),
            BTreeMap::from([("host_owner".into(), "row-86".into())]),
        )
        .expect("telemetry"),
    );
    let order = Arc::new(Mutex::new(Vec::<String>::new()));
    let base_records = Arc::new(Mutex::new(Vec::<ActionAuditRecord>::new()));

    let base: Arc<dyn ActionAuditSink> = Arc::new({
        let order = Arc::clone(&order);
        let base_records = Arc::clone(&base_records);
        move |record: ActionAuditRecord| {
            order.lock().expect("order").push("base".into());
            base_records.lock().expect("records").push(record);
        }
    });
    let sink = with_bot_block_detection_sink(
        base,
        Arc::new({
            let order = Arc::clone(&order);
            let service = Arc::clone(&service);
            move |hit, record| {
                order.lock().expect("order").push("hit".into());
                service
                    .logs
                    .report_bot_block(&BotBlockReport {
                        conversation_id: record.agent_id.clone(),
                        family: hit.family.to_string(),
                        confidence: hit.confidence.as_str().to_string(),
                        blocked_host: hit.blocked_host.clone(),
                        blocked_url: hit.blocked_url.clone(),
                    })
                    .expect("bot block telemetry");
            }
        }),
    );

    sink.record(ActionAuditRecord {
        agent_id: "agent-86".into(),
        turn_id: Some("turn-86".into()),
        occurred_at_ms: 1_900_000_000_000,
        action: json!({
            "kind": "browserNavigation",
            "url": "https://www.google.com/sorry/index?secret=redacted",
            "pageTitle": "Automated queries"
        }),
    });

    assert_eq!(*order.lock().expect("order"), vec!["base", "hit"]);
    assert_eq!(base_records.lock().expect("records").len(), 1);

    let records = persisted(&service);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].event, BOT_BLOCK_EVENT);
    assert_eq!(records[1].event, BOT_BLOCK_DETAIL_EVENT);
    assert_eq!(records[0].payload["metadata"]["family"], "google_sorry");
    assert_eq!(records[0].payload["metadata"]["confidence"], "high");
    assert_eq!(records[1].payload["metadata"]["blocked_host"], "google.com");
    assert_eq!(
        records[1].payload["metadata"]["blocked_url"],
        "https://www.google.com/sorry/index"
    );
    assert_eq!(records[0].payload["metadata"]["host_owner"], "row-86");
    assert_eq!(records[1].payload["metadata"]["host_owner"], "row-86");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn harmless_and_non_navigation_actions_only_reach_base_auditor() {
    let order = Arc::new(Mutex::new(Vec::<String>::new()));
    let hits = Arc::new(Mutex::new(0usize));
    let base: Arc<dyn ActionAuditSink> = Arc::new({
        let order = Arc::clone(&order);
        move |_record: ActionAuditRecord| {
            order.lock().expect("order").push("base".into());
        }
    });
    let sink = with_bot_block_detection_sink(
        base,
        Arc::new({
            let hits = Arc::clone(&hits);
            move |_hit, _record| {
                *hits.lock().expect("hits") += 1;
            }
        }),
    );

    sink.record(ActionAuditRecord {
        agent_id: "agent-86".into(),
        turn_id: None,
        occurred_at_ms: 1,
        action: json!({"kind":"browserNavigation","url":"https://example.com/docs","pageTitle":"Docs"}),
    });
    sink.record(ActionAuditRecord {
        agent_id: "agent-86".into(),
        turn_id: None,
        occurred_at_ms: 2,
        action: json!({"kind":"computerAction","type":"click"}),
    });

    assert_eq!(*order.lock().expect("order"), vec!["base", "base"]);
    assert_eq!(*hits.lock().expect("hits"), 0);
}

#[test]
fn shipping_composition_uses_decorator_and_unique_host_structured_log_owner() {
    let app = include_str!("../app/src/main.rs");
    assert!(app.contains("with_bot_block_detection_sink("));
    assert!(app.contains("bot_block_logs.report_bot_block(&report)"));

    let owner = include_str!("../src/extensions/telemetry/host_telemetry_service.rs");
    assert!(owner.contains("pub fn report_bot_block(&self, report: &BotBlockReport)"));
    assert!(owner.contains("self.report_projection(&bot_block_telemetry(report))?;"));
    assert!(owner.contains("self.report_projection(&bot_block_detail_telemetry(report))"));

    let mapper = include_str!("../src/extensions/telemetry/bot_block_telemetry.rs");
    assert_eq!(mapper.matches("sand.bot_block\"").count(), 1);
    assert_eq!(mapper.matches("sand.bot_block_detail").count(), 1);

    let coordinator = include_str!("../../node-agent-coordinator/src/main.rs");
    assert!(!coordinator.contains("reportBotBlock"));
    assert!(!coordinator.contains("report_bot_block"));
    assert!(!coordinator.contains("sand.bot_block_detail"));

    let electron = include_str!("../../electron-main/telemetry/desktop-structured-log-telemetry.ts");
    assert!(!electron.contains("reportBotBlock"));
    assert!(!electron.contains("sand.bot_block_detail"));
}
