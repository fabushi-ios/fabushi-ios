use mahayana_host_runtime::extensions::telemetry::openclaw_logging_core::{
    is_level_enabled, parse_log_level, resolve_env_log_level, ConsoleLineFormatter, LogLevel,
};
use mahayana_host_runtime::extensions::telemetry::openclaw_secret_redactor::{
    mask_token, redact_auth_header, redact_pem_blocks, redact_sensitive_query_params,
};
use mahayana_host_runtime::extensions::telemetry::openclaw_diagnostic_telemetry::{
    evaluate_memory_pressure, DiagnosticRunRegistry, MemoryPressureLevel,
};
use serde_json::Value;

#[test]
fn test_ocrm_logging_levels_and_formatting() {
    assert_eq!(parse_log_level("debug", LogLevel::Info), LogLevel::Debug);
    assert_eq!(parse_log_level("WARNING", LogLevel::Info), LogLevel::Warn);
    assert_eq!(parse_log_level("invalid-level", LogLevel::Info), LogLevel::Info);

    // Verbose flag overrides env level to Debug
    assert_eq!(resolve_env_log_level(Some("info"), true), LogLevel::Debug);
    assert_eq!(resolve_env_log_level(Some("warn"), false), LogLevel::Warn);
    assert_eq!(resolve_env_log_level(None, false), LogLevel::Info);

    // Level enablement
    assert!(is_level_enabled(LogLevel::Info, LogLevel::Error));
    assert!(is_level_enabled(LogLevel::Info, LogLevel::Info));
    assert!(!is_level_enabled(LogLevel::Info, LogLevel::Debug));
    assert!(!is_level_enabled(LogLevel::Silent, LogLevel::Error));

    // Formatter outputs
    let json_line = ConsoleLineFormatter::format_json(
        "2026-10-27T08:00:00Z",
        LogLevel::Warn,
        Some("agent/subsystem"),
        "High latency observed",
    );
    let parsed: Value = serde_json::from_str(&json_line).expect("valid json");
    assert_eq!(parsed["level"], "warn");
    assert_eq!(parsed["subsystem"], "agent/subsystem");
    assert_eq!(parsed["message"], "High latency observed");

    let text_line = ConsoleLineFormatter::format_text(
        "2026-10-27T08:00:00Z",
        LogLevel::Info,
        Some("core"),
        "System ready",
    );
    assert!(text_line.contains("[core] System ready"));
}

#[test]
fn test_ocrm_logging_secret_redaction() {
    // Short token masked completely
    assert_eq!(mask_token("12345"), "***");
    // Long token preserves head and tail
    assert_eq!(mask_token("sk-proj-1234567890abcdef"), "sk-p...cdef");

    // Query params
    let url = "https://api.example.com/v1/query?user=alice&token=secret123&api_key=key456&mode=fast";
    let redacted_url = redact_sensitive_query_params(url);
    assert_eq!(
        redacted_url,
        "https://api.example.com/v1/query?user=alice&token=[REDACTED]&api_key=[REDACTED]&mode=fast"
    );

    // PEM blocks
    let pem_input = "Cert preamble\n-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA0...\n-----END RSA PRIVATE KEY-----\nCert footer";
    let redacted_pem = redact_pem_blocks(pem_input);
    assert_eq!(redacted_pem, "Cert preamble\n[REDACTED_PRIVATE_KEY]\nCert footer");

    // Auth headers
    assert_eq!(
        redact_auth_header("Bearer sk-proj-1234567890abcdef"),
        "Bearer sk-p...cdef"
    );
    assert_eq!(redact_auth_header("Bearer secret"), "Bearer ***");
}

#[test]
fn test_ocrm_logging_diagnostics_and_memory_pressure() {
    // Memory pressure
    assert_eq!(
        evaluate_memory_pressure(500, 1000),
        MemoryPressureLevel::Normal
    );
    assert_eq!(
        evaluate_memory_pressure(780, 1000),
        MemoryPressureLevel::Moderate
    );
    assert_eq!(
        evaluate_memory_pressure(920, 1000),
        MemoryPressureLevel::Critical
    );

    // Diagnostic run registry & heartbeat sweep
    let mut registry = DiagnosticRunRegistry::new();
    registry.record_heartbeat("sess-1", Some("streaming_reply".to_string()), 1000);
    registry.record_heartbeat("sess-2", Some("indexing".to_string()), 500);

    // Sweep at t=2000 with 1000ms timeout: sess-1 is alive (diff 1000), sess-2 timed out (diff 1500 > 1000)
    registry.sweep_liveness(2000, 1000);

    let hb1 = registry.get_session_heartbeat("sess-1").expect("sess-1 exists");
    assert!(hb1.is_alive);
    assert_eq!(hb1.active_operation.as_deref(), Some("streaming_reply"));

    let hb2 = registry.get_session_heartbeat("sess-2").expect("sess-2 exists");
    assert!(!hb2.is_alive);
}
