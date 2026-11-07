use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::telemetry::host_telemetry_service::{
    HostTelemetryService, PersistedHostTelemetryRecord,
};
use mahayana_host_runtime::extensions::telemetry::turn_telemetry_mappers::{
    ClosingSendNudgeFields, ComputerUseUsageFields, TokenUsage, TtftFields, TurnAwaitFields,
    TurnInterruptFields, TurnRetryFields, TurnUsageFields, UserMessageReceivedFields,
    closing_send_nudge_telemetry, computer_use_usage_telemetry, ttft_telemetry,
    turn_await_telemetry, turn_interrupt_telemetry, turn_retry_telemetry, turn_usage_telemetry,
    user_message_received_telemetry, usage_token_tags,
};

fn usage() -> TokenUsage {
    TokenUsage {
        input_tokens: 11,
        output_tokens: 7,
        cache_read_tokens: 3,
        cache_write_tokens: 2,
        reasoning_tokens: Some(5),
    }
}

#[test]
fn usage_tags_match_frozen_schema_and_total_input_rule() {
    let tags = usage_token_tags(Some(usage()));
    assert_eq!(tags.get("input_tokens").map(String::as_str), Some("11"));
    assert_eq!(tags.get("output_tokens").map(String::as_str), Some("7"));
    assert_eq!(tags.get("cache_read_tokens").map(String::as_str), Some("3"));
    assert_eq!(tags.get("cache_write_tokens").map(String::as_str), Some("2"));
    assert_eq!(tags.get("reasoning_tokens").map(String::as_str), Some("5"));
    assert_eq!(tags.get("total_input_tokens").map(String::as_str), Some("11"));
    assert!(usage_token_tags(None).is_empty());
}

#[test]
fn retry_projection_bounds_error_type_and_level() {
    let retried = turn_retry_telemetry(&TurnRetryFields {
        conversation_id: "c".into(),
        outcome: "retried".into(),
        attempt: 2,
        max_attempts: 4,
        error_type: "provider.timeout".into(),
        error_code: "ETIMEDOUT".into(),
        cause: "network".into(),
        delay_ms: Some(1250.6),
        server_paced: Some(true),
    });
    assert_eq!(retried.level, Some("info"));
    assert_eq!(retried.event, Some("sand.turn.retry"));
    assert_eq!(retried.metadata.get("error_type").map(String::as_str), Some("provider.timeout"));
    assert_eq!(retried.metadata.get("delay_ms").map(String::as_str), Some("1251"));
    assert_eq!(retried.metadata.get("server_paced").map(String::as_str), Some("true"));

    let failed = turn_retry_telemetry(&TurnRetryFields {
        error_type: "not allowed whitespace".into(),
        outcome: "failed".into(),
        ..TurnRetryFields {
            conversation_id: "c".into(),
            outcome: String::new(),
            attempt: 1,
            max_attempts: 1,
            error_type: String::new(),
            error_code: "E".into(),
            cause: "x".into(),
            delay_ms: None,
            server_paced: None,
        }
    });
    assert_eq!(failed.level, Some("warn"));
    assert_eq!(failed.metadata.get("error_type").map(String::as_str), Some("error"));
}

#[test]
fn interrupt_await_message_and_closing_nudge_preserve_fields() {
    let interrupt = turn_interrupt_telemetry(&TurnInterruptFields {
        conversation_id: "c1".into(),
        reason: "user".into(),
        had_active_run: true,
        was_in_flight: false,
    });
    assert_eq!(interrupt.event, Some("sand.turn.interrupt"));
    assert_eq!(interrupt.metadata.get("had_active_run").map(String::as_str), Some("true"));

    let await_event = turn_await_telemetry(&TurnAwaitFields {
        conversation_id: "c1".into(),
        block_until_ms: 99,
        outcome: "resumed".into(),
        await_index: 3,
    });
    assert_eq!(await_event.metadata.get("block_until_ms").map(String::as_str), Some("99"));

    let message = user_message_received_telemetry(&UserMessageReceivedFields {
        conversation_id: "c1".into(),
        was_in_flight: true,
    });
    assert_eq!(message.event, Some("sand.user_message.received"));

    let warn = closing_send_nudge_telemetry(&ClosingSendNudgeFields {
        conversation_id: "c1".into(),
        delivered: false,
        sent_message_count: 0,
        aborted: false,
    });
    assert_eq!(warn.level, Some("warn"));
    let aborted = closing_send_nudge_telemetry(&ClosingSendNudgeFields {
        aborted: true,
        ..ClosingSendNudgeFields {
            conversation_id: "c1".into(),
            delivered: false,
            sent_message_count: 0,
            aborted: false,
        }
    });
    assert_eq!(aborted.level, Some("info"));
}

#[test]
fn ttft_turn_usage_and_computer_use_match_frozen_schema() {
    let ttft = ttft_telemetry(&TtftFields {
        conversation_id: "c".into(),
        ttft_ms: Some(42.6),
        skew: false,
        skew_reason: "none".into(),
        chunk_type: "text".into(),
        is_fork: false,
        model_id: "m".into(),
        trace_id: "t".into(),
        span_id: "s".into(),
    });
    assert_eq!(ttft.metadata.get("ttft_ms").map(String::as_str), Some("43"));

    let turn = turn_usage_telemetry(&TurnUsageFields {
        conversation_id: "c".into(),
        source: "runner".into(),
        request_id: Some("req-1".into()),
        request_id_count: 2,
        turn_ended_seq: 8,
        usage: Some(usage()),
    });
    assert_eq!(turn.metadata.get("schema_version").map(String::as_str), Some("2"));
    assert_eq!(turn.metadata.get("has_request_id").map(String::as_str), Some("true"));
    assert_eq!(turn.metadata.get("has_usage").map(String::as_str), Some("true"));

    let computer = computer_use_usage_telemetry(&ComputerUseUsageFields {
        parent_agent_id: "parent".into(),
        subagent_agent_id: "sub".into(),
        subagent_type: "computer".into(),
        subagent_request_id: "req".into(),
        model_id: "model".into(),
        outcome: "error".into(),
        duration_ms: 9.6,
        tool_call_count: 4,
        turn_ended_count: 1,
        usage: None,
    });
    assert_eq!(computer.level, Some("warn"));
    assert_eq!(computer.metadata.get("duration_ms").map(String::as_str), Some("10"));
    assert_eq!(computer.metadata.get("has_usage").map(String::as_str), Some("false"));
}


fn telemetry_root() -> std::path::PathBuf {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-turn-typed-facade-{}-{n}",
        std::process::id()
    ))
}

#[test]
fn typed_host_turn_facade_writes_frozen_closing_ttft_and_usage_jsonl() {
    let root = telemetry_root();
    let service = HostTelemetryService::open(root.join("telemetry.jsonl"))
        .expect("telemetry service");

    service
        .logs
        .report_closing_send_nudge(&ClosingSendNudgeFields {
            conversation_id: "c1".into(),
            delivered: false,
            sent_message_count: 0,
            aborted: false,
        })
        .expect("closing send");
    service
        .logs
        .report_ttft(&TtftFields {
            conversation_id: "c1".into(),
            ttft_ms: Some(12.6),
            skew: false,
            skew_reason: String::new(),
            chunk_type: "text".into(),
            is_fork: false,
            model_id: "model".into(),
            trace_id: "trace".into(),
            span_id: "span".into(),
        })
        .expect("ttft");
    service
        .logs
        .report_turn_usage(&TurnUsageFields {
            conversation_id: "c1".into(),
            source: "turn".into(),
            request_id: Some("req".into()),
            request_id_count: 1,
            turn_ended_seq: 9,
            usage: Some(usage()),
        })
        .expect("usage");

    let records = fs::read_to_string(service.records_path())
        .expect("jsonl")
        .lines()
        .map(|line| serde_json::from_str::<PersistedHostTelemetryRecord>(line).expect("record"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 3);
    assert_eq!(records[0].event, "sand.turn.closing_send_nudge");
    assert_eq!(records[0].payload["level"], "warn");
    assert_eq!(records[0].payload["metadata"]["delivered"], "false");
    assert_eq!(records[0].payload["metadata"]["sent_message_count"], "0");
    assert_eq!(records[1].event, "sand.ttft");
    assert_eq!(records[1].payload["metadata"]["ttft_ms"], "13");
    assert_eq!(records[1].payload["metadata"]["trace_id"], "trace");
    assert_eq!(records[1].payload["metadata"]["span_id"], "span");
    assert_eq!(records[2].event, "sand.turn.usage");
    assert_eq!(records[2].payload["metadata"]["schema_version"], "2");
    assert_eq!(records[2].payload["metadata"]["input_tokens"], "11");

    let _ = fs::remove_dir_all(root);
}
