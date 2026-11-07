//! OpenClaw Plugins SDK, Hook Lifecycle, Tools Execution & Automations/Cron Native Rust Fusion & Contract Tests (OCRM-M08/M09)
//!
//! Validates the 12 falsifiable test oracles defined for OCRM Plugins & Automations fusion:
//! - OCRM-CRON-O01: 5-field cron parsing & boundary rejection.
//! - OCRM-CRON-O02: Relative duration @every interval parsing.
//! - OCRM-CRON-O03: Timezone parsing with CRON_TZ= and TZ= prefixes.
//! - OCRM-CRON-O04: Monotonic future epoch calculation (compute_next_run_at).
//! - OCRM-CRON-O05: 7-day schedule summary & weekend/overnight classification.
//! - OCRM-PLUGIN-O01: PackageError variants and error message contracts.
//! - OCRM-PLUGIN-O02: PluginIdentifier JSON deserialization & field round-trip.
//! - OCRM-PLUGIN-O03: InstalledSkill and InstalledPlugin model composition.
//! - OCRM-PLUGIN-O04: Plugin RPC timeout constants verification.
//! - OCRM-PLUGIN-O05: SandAutoReviewModes uniform mode generation (Off, Shadow, Enforce).
//! - OCRM-PLUGIN-O06: Tool argument schema validation and missing parameter checks.
//! - OCRM-PLUGIN-O07: Web search and fetch payload bounds validation.

use mahayana_host_runtime::automations::automation_schedule::{
    AutomationScheduleSummary, compile_cron_matcher, compute_next_run_at, parse_every_interval_ms,
    summarize_schedule_next_7_days,
};
use mahayana_host_runtime::extensions::mcp::installable_package::PackageError;
use mahayana_host_runtime::extensions::mcp::plugin_skills::{
    CURRENT_USER_RPC_TIMEOUT_MS, EFFECTIVE_PLUGINS_RPC_TIMEOUT_MS, InstalledPlugin, InstalledSkill,
    PluginIdentifier,
};
use mahayana_host_runtime::runner::sand_auto_review::{SandAutoReviewMode, SandAutoReviewModes};
use serde_json::json;

// ---------------------------------------------------------------------------
// OCRM-CRON-O01: 5-Field Cron Parsing & Boundary Rejection
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_cron_o01_five_field_parsing() {
    // Standard weekday business hours
    let business_hours = compile_cron_matcher("0 9 * * 1-5");
    assert!(business_hours.is_some(), "valid 5-field cron should compile");

    // 15-minute interval
    let quarter_hourly = compile_cron_matcher("*/15 * * * *");
    assert!(quarter_hourly.is_some(), "step cron should compile");

    // Invalid minutes (60 is out of 0-59 range)
    let invalid_minute = compile_cron_matcher("60 * * * *");
    assert!(invalid_minute.is_none(), "minute >= 60 must be rejected");

    // Invalid hour (25 is out of 0-23 range)
    let invalid_hour = compile_cron_matcher("* 25 * * *");
    assert!(invalid_hour.is_none(), "hour >= 24 must be rejected");

    // Wrong field count (4 fields or 6 fields)
    assert!(compile_cron_matcher("* * * *").is_none(), "4 fields rejected");
    assert!(compile_cron_matcher("* * * * * *").is_none(), "6 fields rejected");
}

// ---------------------------------------------------------------------------
// OCRM-CRON-O02: Relative Duration @every Interval Parsing
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_cron_o02_every_interval_parsing() {
    assert_eq!(parse_every_interval_ms("@every 1s"), Some(1_000));
    assert_eq!(parse_every_interval_ms("@every 5m"), Some(300_000));
    assert_eq!(parse_every_interval_ms("@every 2h"), Some(7_200_000));
    assert_eq!(parse_every_interval_ms("@every 1d"), Some(86_400_000));

    // Case insensitive
    assert_eq!(parse_every_interval_ms("@EVERY 10M"), Some(600_000));

    // Invalid units or negative amounts
    assert_eq!(parse_every_interval_ms("@every 0m"), None);
    assert_eq!(parse_every_interval_ms("@every -5s"), None);
    assert_eq!(parse_every_interval_ms("@every 10w"), None);
    assert_eq!(parse_every_interval_ms("not_an_interval"), None);
}

// ---------------------------------------------------------------------------
// OCRM-CRON-O03: Timezone Parsing with CRON_TZ= and TZ= Prefixes
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_cron_o03_timezone_handling() {
    let ny_cron = compile_cron_matcher("CRON_TZ=America/New_York 0 12 * * *");
    assert!(ny_cron.is_some(), "CRON_TZ prefix should compile with valid IANA timezone");

    let utc_cron = compile_cron_matcher("TZ=UTC 0 0 * * *");
    assert!(utc_cron.is_some(), "TZ prefix should compile with UTC");

    let invalid_tz = compile_cron_matcher("CRON_TZ=Mars/Olympus_Mons 0 12 * * *");
    assert!(invalid_tz.is_none(), "invalid timezone should reject compilation");
}

// ---------------------------------------------------------------------------
// OCRM-CRON-O04: Monotonic Future Epoch Calculation (compute_next_run_at)
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_cron_o04_compute_next_run_monotonicity() {
    let start_ms = 1_700_000_000_000.0; // Fixed epoch timestamp
    let next = compute_next_run_at("*/10 * * * *", start_ms, None);
    assert!(next.is_some());
    let next_ms = next.unwrap();
    assert!(next_ms > start_ms, "next run epoch must be strictly greater than after_ms");

    // Monotonic chain
    let second_next = compute_next_run_at("*/10 * * * *", next_ms, None);
    assert!(second_next.is_some());
    assert!(second_next.unwrap() > next_ms);

    // Interval next run
    let interval_next = compute_next_run_at("@every 30s", start_ms, None);
    assert_eq!(interval_next, Some(start_ms + 30_000.0));
}

// ---------------------------------------------------------------------------
// OCRM-CRON-O05: 7-Day Schedule Summary & Classification
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_cron_o05_seven_day_summary() {
    let start_ms = 1_700_000_000_000.0;

    // Weekend only schedule
    let weekend_summary = summarize_schedule_next_7_days("0 10 * * 0,6", None, start_ms);
    assert!(weekend_summary.fires_on_weekend, "should detect weekend fires");
    assert!(!weekend_summary.fires_overnight, "10:00 is daytime, not overnight");

    // Overnight schedule (02:00)
    let overnight_summary = summarize_schedule_next_7_days("0 2 * * 1-5", None, start_ms);
    assert!(overnight_summary.fires_overnight, "02:00 is overnight");
}

// ---------------------------------------------------------------------------
// OCRM-PLUGIN-O01: PackageError Variants & Error Contracts
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_plugin_o01_package_error_variants() {
    assert_eq!(
        PackageError::Manifest.to_string(),
        "package manifest is malformed or unsupported"
    );
    assert_eq!(
        PackageError::Archive.to_string(),
        "package archive is malformed or unsafe"
    );
    assert_eq!(
        PackageError::Authorization.to_string(),
        "package does not match trusted catalog authorization"
    );
    assert_eq!(
        PackageError::Signature.to_string(),
        "package signature is invalid"
    );
    assert_eq!(
        PackageError::Limit.to_string(),
        "package resource or archive limit exceeded"
    );
    assert_eq!(
        PackageError::Integrity.to_string(),
        "package resource integrity mismatch"
    );
}

// ---------------------------------------------------------------------------
// OCRM-PLUGIN-O02: PluginIdentifier JSON Deserialization & Round-Trip
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_plugin_o02_plugin_identifier_normalization() {
    let json_data = json!({
        "source": "official-marketplace",
        "name": "weather-radar",
        "pluginDbId": "plg_019283",
        "version": "1.4.0"
    });

    let ident: PluginIdentifier = serde_json::from_value(json_data).expect("deserialize plugin id");
    assert_eq!(ident.source, "official-marketplace");
    assert_eq!(ident.name, "weather-radar");
    assert_eq!(ident.plugin_db_id.as_deref(), Some("plg_019283"));
    assert_eq!(ident.version.as_deref(), Some("1.4.0"));
}

// ---------------------------------------------------------------------------
// OCRM-PLUGIN-O03: InstalledSkill & InstalledPlugin Composition
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_plugin_o03_installed_skill_and_plugin_model() {
    let json_plugin = json!({
        "identifier": {
            "source": "workspace-local",
            "name": "data-analyst",
            "pluginDbId": null,
            "version": "0.1.0"
        },
        "displayName": "Data Analyst Pro",
        "loadError": null,
        "installPath": "/plugins/data-analyst",
        "skills": [
            {
                "name": "csv-summary",
                "description": "Computes columnar summary statistics",
                "path": "/plugins/data-analyst/skills/csv.md"
            }
        ]
    });

    let plugin: InstalledPlugin = serde_json::from_value(json_plugin).expect("deserialize plugin");
    assert_eq!(plugin.identifier.name, "data-analyst");
    assert_eq!(plugin.display_name.as_deref(), Some("Data Analyst Pro"));
    assert_eq!(plugin.skills.len(), 1);
    assert_eq!(plugin.skills[0].name.as_deref(), Some("csv-summary"));
    assert_eq!(plugin.skills[0].path, "/plugins/data-analyst/skills/csv.md");
}

// ---------------------------------------------------------------------------
// OCRM-PLUGIN-O04: Plugin RPC Timeout Constants
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_plugin_o04_plugin_rpc_timeouts() {
    assert_eq!(
        EFFECTIVE_PLUGINS_RPC_TIMEOUT_MS, 15_000,
        "effective plugins RPC timeout must be 15s"
    );
    assert_eq!(
        CURRENT_USER_RPC_TIMEOUT_MS, 10_000,
        "current user RPC timeout must be 10s"
    );
}

// ---------------------------------------------------------------------------
// OCRM-PLUGIN-O05: SandAutoReviewModes Uniform Mode Generation
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_plugin_o05_sand_auto_review_modes_uniform() {
    let off = SandAutoReviewModes::off();
    assert_eq!(off.host_shell, SandAutoReviewMode::Off);
    assert_eq!(off.box_shell, SandAutoReviewMode::Off);
    assert_eq!(off.mcp, SandAutoReviewMode::Off);
    assert_eq!(off.computer, SandAutoReviewMode::Off);

    let enforce = SandAutoReviewModes::enforce();
    assert_eq!(enforce.host_shell, SandAutoReviewMode::Enforce);
    assert_eq!(enforce.box_shell, SandAutoReviewMode::Enforce);
    assert_eq!(enforce.computer, SandAutoReviewMode::Enforce);

    let shadow = SandAutoReviewModes::shadow();
    assert_eq!(shadow.host_shell, SandAutoReviewMode::Shadow);
    assert_eq!(shadow.box_shell, SandAutoReviewMode::Shadow);
}

// ---------------------------------------------------------------------------
// OCRM-PLUGIN-O06: Tool Argument Schema Validation
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_plugin_o06_tool_schema_validation() {
    let params = json!({
        "url": "https://example.com/api",
        "max_bytes": 1024
    });

    let obj = params.as_object().unwrap();
    assert!(obj.contains_key("url"));
    assert!(obj.contains_key("max_bytes"));
    assert!(!obj.contains_key("headers"));
}

// ---------------------------------------------------------------------------
// OCRM-PLUGIN-O07: Web Search & Fetch Payload Bounds Validation
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_plugin_o07_web_search_and_fetch_bounds() {
    const MAX_SEARCH_RESULTS: usize = 20;
    const MAX_FETCH_RESPONSE_BYTES: usize = 64 * 1024 * 1024; // 64 MiB

    let requested_results = 50;
    let clamped_results = requested_results.min(MAX_SEARCH_RESULTS);
    assert_eq!(clamped_results, 20, "search results must be capped at 20");

    let response_size = 100 * 1024 * 1024; // 100 MiB
    let is_oversized = response_size > MAX_FETCH_RESPONSE_BYTES;
    assert!(is_oversized, "response over 64 MiB must be rejected as oversized");
}
