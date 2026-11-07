use std::collections::BTreeMap;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::telemetry::bot_block_telemetry::{
    BOT_BLOCK_DETAIL_EVENT, BOT_BLOCK_EVENT, BotBlockReport,
    bot_block_detail_telemetry, bot_block_telemetry,
};
use mahayana_host_runtime::extensions::telemetry::host_telemetry_service::{
    HostTelemetryService, PersistedHostTelemetryRecord,
};

fn temp_root() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "fabushi-bot-block-row86-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&root).expect("root");
    root
}

fn report() -> BotBlockReport {
    BotBlockReport {
        conversation_id: "agent-86".into(),
        family: "google_sorry".into(),
        confidence: "high".into(),
        blocked_host: "google.com".into(),
        blocked_url: "https://www.google.com/sorry/index".into(),
    }
}

#[test]
fn frozen_bot_block_mapper_pins_two_warn_events_and_exact_metadata_split() {
    let summary = bot_block_telemetry(&report());
    assert_eq!(summary.level, Some("warn"));
    assert_eq!(summary.event, Some(BOT_BLOCK_EVENT));
    assert_eq!(
        summary.metadata,
        BTreeMap::from([
            ("confidence".into(), "high".into()),
            ("conversation_id".into(), "agent-86".into()),
            ("family".into(), "google_sorry".into()),
        ])
    );

    let detail = bot_block_detail_telemetry(&report());
    assert_eq!(detail.level, Some("warn"));
    assert_eq!(detail.event, Some(BOT_BLOCK_DETAIL_EVENT));
    assert_eq!(
        detail.metadata,
        BTreeMap::from([
            ("blocked_host".into(), "google.com".into()),
            ("blocked_url".into(), "https://www.google.com/sorry/index".into()),
            ("conversation_id".into(), "agent-86".into()),
            ("family".into(), "google_sorry".into()),
        ])
    );
}

#[test]
fn typed_host_facade_writes_summary_then_detail_jsonl_with_identity() {
    let root = temp_root();
    let service = HostTelemetryService::open_with_identity_tags(
        root.join("events.jsonl"),
        BTreeMap::from([("box_store_id".into(), "store-86".into())]),
    )
    .expect("telemetry");
    service.logs.report_bot_block(&report()).expect("report");

    let raw = fs::read_to_string(service.records_path()).expect("jsonl");
    let records = raw
        .lines()
        .map(|line| serde_json::from_str::<PersistedHostTelemetryRecord>(line).expect("record"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);

    assert_eq!(records[0].channel, "structured_log");
    assert_eq!(records[0].event, BOT_BLOCK_EVENT);
    assert_eq!(records[0].payload["level"], "warn");
    assert_eq!(records[0].payload["metadata"]["conversation_id"], "agent-86");
    assert_eq!(records[0].payload["metadata"]["family"], "google_sorry");
    assert_eq!(records[0].payload["metadata"]["confidence"], "high");
    assert_eq!(records[0].payload["metadata"]["box_store_id"], "store-86");
    assert!(records[0].payload["metadata"].get("blocked_host").is_none());
    assert!(records[0].payload["metadata"].get("blocked_url").is_none());

    assert_eq!(records[1].channel, "structured_log");
    assert_eq!(records[1].event, BOT_BLOCK_DETAIL_EVENT);
    assert_eq!(records[1].payload["level"], "warn");
    assert_eq!(records[1].payload["metadata"]["conversation_id"], "agent-86");
    assert_eq!(records[1].payload["metadata"]["family"], "google_sorry");
    assert_eq!(records[1].payload["metadata"]["blocked_host"], "google.com");
    assert_eq!(
        records[1].payload["metadata"]["blocked_url"],
        "https://www.google.com/sorry/index"
    );
    assert_eq!(records[1].payload["metadata"]["box_store_id"], "store-86");
    assert!(records[1].payload["metadata"].get("confidence").is_none());

    let _ = fs::remove_dir_all(root);
}
