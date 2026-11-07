use mahayana_host_runtime::extensions::plugins::hook_runner::{
    HookName, PluginHookRunner, ProviderExtensionBridge,
};
use mahayana_host_runtime::extensions::plugins::installed_index_store::{
    InstalledPluginIndexRecord, InstalledPluginIndexStore, INSTALLED_PLUGIN_INDEX_VERSION,
    INSTALLED_PLUGIN_INDEX_WARNING,
};
use mahayana_host_runtime::extensions::plugins::manifest_doctor::{
    check_plugin_manifest_health, DiagnosticSeverity, PluginDoctorReport,
};
use serde_json::json;
use std::time::Duration;

#[test]
fn test_ocrm_plugins_installed_index_store() {
    let store = InstalledPluginIndexStore::new();
    let snapshot = store.get_snapshot();
    assert_eq!(snapshot.version, INSTALLED_PLUGIN_INDEX_VERSION);
    assert_eq!(snapshot.warning, INSTALLED_PLUGIN_INDEX_WARNING);

    // Insert record
    store.set_record(InstalledPluginIndexRecord {
        plugin_id: "plugin-alpha".to_string(),
        version: "1.0.0".to_string(),
        package_name: Some("@openclaw/alpha".to_string()),
        install_path: "/var/plugins/alpha".to_string(),
        source_type: "clawhub".to_string(),
        signature: Some("sig-xyz".to_string()),
        enabled: true,
    });

    store.set_record(InstalledPluginIndexRecord {
        plugin_id: "plugin-beta".to_string(),
        version: "2.1.0".to_string(),
        package_name: None,
        install_path: "/var/plugins/beta".to_string(),
        source_type: "npm".to_string(),
        signature: None,
        enabled: false,
    });

    let rec = store.get_record("plugin-alpha").expect("Found record");
    assert_eq!(rec.version, "1.0.0");
    assert!(rec.enabled);

    // Filter enabled
    let enabled = store.list_enabled_records();
    assert_eq!(enabled.len(), 1);
    assert_eq!(enabled[0].plugin_id, "plugin-alpha");

    // Toggle enabled
    assert!(store.set_enabled("plugin-beta", true));
    assert_eq!(store.list_enabled_records().len(), 2);

    // Remove record
    let removed = store.remove_record("plugin-alpha").expect("Removed");
    assert_eq!(removed.plugin_id, "plugin-alpha");
    assert!(store.get_record("plugin-alpha").is_none());
}

#[test]
fn test_ocrm_plugins_manifest_doctor() {
    let mut report = PluginDoctorReport::new();

    // 1. Missing manifest
    let diags1 = check_plugin_manifest_health("broken-plugin", false, None, false);
    for d in diags1 {
        report.add_diagnostic(d);
    }
    assert!(report.has_errors());
    assert_eq!(report.errors().len(), 1);
    assert_eq!(report.errors()[0].code, "MISSING_MANIFEST");

    // 2. Valid manifest with deprecated channel config warning
    let diags2 = check_plugin_manifest_health("legacy-channel-plugin", true, Some("main.js"), true);
    assert_eq!(diags2.len(), 1);
    assert_eq!(diags2[0].code, "DEPRECATED_CHANNEL_CONFIG");
    assert_eq!(diags2[0].severity, DiagnosticSeverity::Warning);

    // 3. Clean plugin
    let diags3 = check_plugin_manifest_health("clean-plugin", true, Some("index.js"), false);
    assert!(diags3.is_empty());
}

#[test]
fn test_ocrm_plugins_hook_runner_and_provider_bridge() {
    let hook_runner = PluginHookRunner::new();

    // Register hooks with priority: higher priority runs first
    // priority 10 (runs second)
    hook_runner.register_hook(HookName::BeforeReply, "plugin-mid", 10, |payload| {
        let mut obj = payload.as_object().unwrap().clone();
        let current_text = obj.get("text").and_then(|t| t.as_str()).unwrap_or("");
        obj.insert("text".to_string(), json!(format!("{current_text} -> mid")));
        Ok(Some(json!(obj)))
    });

    // priority 100 (runs first)
    hook_runner.register_hook(HookName::BeforeReply, "plugin-first", 100, |payload| {
        let mut obj = payload.as_object().unwrap().clone();
        let current_text = obj.get("text").and_then(|t| t.as_str()).unwrap_or("");
        obj.insert("text".to_string(), json!(format!("{current_text} -> first")));
        Ok(Some(json!(obj)))
    });

    // priority 5 (runs third, returns Err but shouldn't abort)
    hook_runner.register_hook(HookName::BeforeReply, "plugin-faulty", 5, |_| {
        Err("Transient hook execution failure".to_string())
    });

    let initial = json!({"text": "start"});
    let final_payload = hook_runner
        .run_hooks(HookName::BeforeReply, &initial)
        .expect("Hook pipeline executes with fail-soft");

    assert_eq!(
        final_payload.get("text").unwrap().as_str().unwrap(),
        "start -> first -> mid"
    );

    // Provider extension bridge
    let bridge = ProviderExtensionBridge::new();
    bridge.register_provider("speech-eleven", |op, params| match op {
        "synthesize" => {
            let text = params.get("text").and_then(|t| t.as_str()).unwrap_or("");
            Ok(json!({"audio_len": text.len(), "sample_rate": 24000}))
        }
        _ => Err("Unsupported operation".to_string()),
    });

    let res = bridge
        .invoke_provider(
            "speech-eleven",
            "synthesize",
            &json!({"text": "Hello world"}),
            Duration::from_secs(1),
        )
        .expect("Invoke provider");

    assert_eq!(res.get("audio_len").unwrap().as_u64().unwrap(), 11);
    assert_eq!(res.get("sample_rate").unwrap().as_u64().unwrap(), 24000);
}

#[test]
fn test_ocrm_plugins_boundary_invariants() {
    // Tests equivalent behavior from upstream src/plugins/contracts/boundary-invariants.test.ts
    use mahayana_host_runtime::extensions::plugins::hook_runner::HookBoundaryInvariantGuard;

    // 1. Allowed bundled typed hooks
    assert!(HookBoundaryInvariantGuard::is_allowed_typed_hook("agent_end"));
    assert!(HookBoundaryInvariantGuard::is_allowed_typed_hook("before_prompt_build"));
    assert!(HookBoundaryInvariantGuard::is_allowed_typed_hook("session_end"));
    assert!(!HookBoundaryInvariantGuard::is_allowed_typed_hook("unknown_untyped_hook"));

    // 2. Production code off raw registerHook calls
    let clean_sources = [
        ("extensions/acpx/index.ts", "api.on('reply_dispatch', () => {})"),
        ("extensions/codex/index.ts", "api.on('inbound_claim', () => {})"),
    ];
    assert!(HookBoundaryInvariantGuard::assert_no_raw_register_hook_calls(&clean_sources).is_ok());

    let bad_sources = [
        ("extensions/legacy/index.ts", "api.registerHook('reply_dispatch', () => {})"),
    ];
    let violations = HookBoundaryInvariantGuard::assert_no_raw_register_hook_calls(&bad_sources).unwrap_err();
    assert_eq!(violations, vec!["extensions/legacy/index.ts"]);

    // 3. Bundled extension import boundary guards
    assert!(HookBoundaryInvariantGuard::is_allowed_bundled_extension_import("extensions/browser/api.js"));
    assert!(HookBoundaryInvariantGuard::is_allowed_bundled_extension_import("extensions/discord/runtime-api.js"));
    assert!(!HookBoundaryInvariantGuard::is_allowed_bundled_extension_import("extensions/browser/src/internal-helper.js"));
    assert!(!HookBoundaryInvariantGuard::is_allowed_bundled_extension_import("extensions/discord/subagent-hooks-api.ts"));
}

#[test]
fn test_ocrm_plugins_agent_event_payload_and_active_run_steering() {
    // Tests equivalent behavior from agent-event-payload.contract.test.ts and active-run-steering.contract.test.ts
    use mahayana_host_runtime::extensions::plugins::hook_runner::{
        ActiveRunSteeringManager, AgentEventSubscription, EventPayloadMode, SteerActiveRunParams,
    };

    // 1. Agent event subscription filtering & metadata isolation
    let sub = AgentEventSubscription {
        id: "events".to_string(),
        streams: vec!["tool".to_string()],
        phases: vec![" start ".to_string()], // trimmed matching
        payload_mode: EventPayloadMode::Metadata,
    };

    assert!(sub.matches("tool", Some("start")));
    assert!(!sub.matches("tool", Some("result")));
    assert!(!sub.matches("chat", Some("start")));

    let full_data = json!({
        "phase": "result",
        "count": 42,
        "enabled": false,
        "short_string": "hello",
        "long_string": "a".repeat(300),
        "nested": {"key": "val"},
        "arr": [1, 2, 3]
    });

    let filtered = sub.filter_payload(&full_data);
    assert_eq!(filtered.get("phase").unwrap().as_str().unwrap(), "result");
    assert_eq!(filtered.get("count").unwrap().as_i64().unwrap(), 42);
    assert_eq!(filtered.get("short_string").unwrap().as_str().unwrap(), "hello");
    // Long string > 256, nested, and arr are excluded in metadata mode
    assert!(filtered.get("long_string").is_none());
    assert!(filtered.get("nested").is_none());
    assert!(filtered.get("arr").is_none());

    // 2. Active run steering
    let steering = ActiveRunSteeringManager::new();

    // Success on active streaming run
    let params = SteerActiveRunParams {
        session_key: "agent:main:steering-test".to_string(),
        text: "Use the corrected result.".to_string(),
        display_text: Some("Check the result.".to_string()),
        idempotency_key: Some("review-1".to_string()),
    };
    let res = steering.steer_active_run(true, Some("steered-run"), true, &params);
    assert!(res.steered);
    assert_eq!(res.run_id.as_deref(), Some("steered-run"));

    // Duplicate idempotency key rejected
    let res_dup = steering.steer_active_run(true, Some("steered-run"), true, &params);
    assert!(!res_dup.steered);
    assert_eq!(res_dup.reason.as_deref(), Some("duplicate"));

    // Disabled prompt injection
    let res_disabled = steering.steer_active_run(false, Some("steered-run"), true, &params);
    assert!(!res_disabled.steered);
    assert!(res_disabled.reason.unwrap().contains("allowPromptInjection=false"));

    // No active run
    let res_no_run = steering.steer_active_run(true, None, true, &params);
    assert!(!res_no_run.steered);
    assert_eq!(res_no_run.reason.as_deref(), Some("no_active_run"));

    // Not streaming
    let res_not_streaming = steering.steer_active_run(true, Some("steered-run"), false, &params);
    assert!(!res_not_streaming.steered);
    assert_eq!(res_not_streaming.reason.as_deref(), Some("not_streaming"));
}

#[test]
fn test_ocrm_plugins_architectural_boundary_guardrails() {
    // Tests equivalent behavior from bundled-extension-config-api-guardrails, core-extension-facade-boundary,
    // deprecated-internal-config-api, and model-catalog-core-imports contract test suites.
    use mahayana_host_runtime::extensions::plugins::hook_runner::{
        BundledExtensionConfigApiGuard, CoreExtensionFacadeBoundaryGuard,
        DeprecatedInternalConfigApiGuard, ModelCatalogCoreImportsGuard,
    };

    // 1. Bundled extension config API guard
    assert!(BundledExtensionConfigApiGuard::validate_extension_config_schema(
        "extensions/telegram/src/config-schema.ts",
        "import { createChannelConfigSchema } from 'openclaw/plugin-sdk/channel-config-schema';"
    ).is_ok());

    let err_forbidden_bundled = BundledExtensionConfigApiGuard::validate_extension_config_schema(
        "extensions/telegram/src/config-schema.ts",
        "import { schema } from 'openclaw/plugin-sdk/bundled-channel-config-schema';"
    ).unwrap_err();
    assert!(err_forbidden_bundled.contains("missing allowed openclaw/plugin-sdk/channel-config-schema"));

    assert!(BundledExtensionConfigApiGuard::validate_extension_config_schema(
        "extensions/whatsapp/src/config-schema.ts",
        "import { defineChannelConfig } from '../config-api.js';"
    ).is_ok());

    let err_forbidden_whatsapp = BundledExtensionConfigApiGuard::validate_extension_config_schema(
        "extensions/whatsapp/src/config-schema.ts",
        "import { defineChannelConfig } from 'openclaw/plugin-sdk/channel-config-schema';"
    ).unwrap_err();
    assert!(err_forbidden_whatsapp.contains("missing allowed ../config-api.js"));

    // 2. Core extension facade boundary guard
    let clean_paths = ["src/plugin-sdk/index.ts", "src/plugins/registry.ts"];
    assert!(CoreExtensionFacadeBoundaryGuard::assert_no_ollama_facades(&clean_paths).is_ok());

    let violating_paths = ["src/plugin-sdk/ollama.ts", "src/plugin-sdk/index.ts"];
    let err_facade = CoreExtensionFacadeBoundaryGuard::assert_no_ollama_facades(&violating_paths).unwrap_err();
    assert_eq!(err_facade, vec!["src/plugin-sdk/ollama.ts"]);

    let clean_fixtures = [
        ("auth-choice.test.ts", "export const config = { provider: 'test' };"),
        ("discovery.test.ts", "test('discovery', () => {});"),
    ];
    assert!(CoreExtensionFacadeBoundaryGuard::assert_no_bundled_provider_terms_in_fixtures(&clean_fixtures).is_ok());

    let dirty_fixtures = [
        ("tts-contract-suites.ts", "const p = 'Ollama';"),
    ];
    let err_terms = CoreExtensionFacadeBoundaryGuard::assert_no_bundled_provider_terms_in_fixtures(&dirty_fixtures).unwrap_err();
    assert!(err_terms[0].contains("contains forbidden term 'Ollama'"));

    // 3. Deprecated internal config API guard
    let clean_config_sources = [("src/config.ts", "export function loadConfig() {}")];
    assert!(DeprecatedInternalConfigApiGuard::assert_no_deprecated_config_api(&clean_config_sources).is_ok());

    let deprecated_sources = [("src/legacy.ts", "loadDeprecatedPluginConfig(id);")];
    let err_deprecated = DeprecatedInternalConfigApiGuard::assert_no_deprecated_config_api(&deprecated_sources).unwrap_err();
    assert!(err_deprecated[0].contains("loadDeprecatedPluginConfig"));

    // 4. Model catalog core imports guard
    let clean_catalog_sources = [("src/agents.ts", "import { normalize } from '@openclaw/model-catalog-core/model-catalog-normalize';")];
    assert!(ModelCatalogCoreImportsGuard::assert_no_legacy_bridge_imports(&clean_catalog_sources).is_ok());

    let dirty_catalog_sources = [("src/agents.ts", "import { normalize } from './src/model-catalog/normalize.js';")];
    let err_bridges = ModelCatalogCoreImportsGuard::assert_no_legacy_bridge_imports(&dirty_catalog_sources).unwrap_err();
    assert!(err_bridges[0].contains("imports legacy bridge"));
}

#[test]
fn test_ocrm_plugins_runtime_import_side_effects_and_seams() {
    // Tests equivalent behavior from runtime-import-side-effects and runtime-seams contract test suites.
    use mahayana_host_runtime::extensions::plugins::runtime_final::{
        RuntimeFacadeSeamManager, RuntimeImportSideEffectsGuard,
    };

    // 1. Runtime import side-effects
    let clean_runtime_sources = [("src/plugins/runtime/runtime-channel.ts", "export const ok = true;")];
    assert!(RuntimeImportSideEffectsGuard::assert_no_direct_channel_registry_imports(&clean_runtime_sources).is_ok());

    let dirty_runtime_sources = [("src/plugins/runtime/index.ts", "import '../../channels/plugins/registry';")];
    let err_direct = RuntimeImportSideEffectsGuard::assert_no_direct_channel_registry_imports(&dirty_runtime_sources).unwrap_err();
    assert_eq!(err_direct, vec!["src/plugins/runtime/index.ts"]);

    // Markdown table mode resolution
    assert_eq!(
        RuntimeImportSideEffectsGuard::resolve_markdown_table_mode("telegram", true, None),
        "bullets"
    );
    assert_eq!(
        RuntimeImportSideEffectsGuard::resolve_markdown_table_mode("telegram", true, Some("block")),
        "block"
    );
    assert_eq!(
        RuntimeImportSideEffectsGuard::resolve_markdown_table_mode("discord", true, None),
        "block"
    );
    assert_eq!(
        RuntimeImportSideEffectsGuard::resolve_markdown_table_mode("discord", false, None),
        "bullets"
    );

    // 2. Runtime facade seam
    assert!(RuntimeFacadeSeamManager::resolve_bundled_plugin_public_surface_access("line", true));
    assert!(!RuntimeFacadeSeamManager::resolve_bundled_plugin_public_surface_access("line", false));

    // SSRF fetch guard
    assert!(RuntimeFacadeSeamManager::fetch_with_ssrf_guard("https://public.example/api", false).is_ok());
    let err_ssrf = RuntimeFacadeSeamManager::fetch_with_ssrf_guard("http://192.168.1.1/admin", true).unwrap_err();
    assert!(err_ssrf.contains("SSRF blocked access to private network"));
}

#[test]
fn test_ocrm_plugins_config_boundary_and_test_api_consumption() {
    // Tests equivalent behavior from config-boundary-guard and extension-test-api-consumption contract test suites.
    use mahayana_host_runtime::extensions::plugins::hook_runner::{
        ConfigBoundaryGuard, ExtensionTestApiConsumptionGuard,
    };

    // 1. Config boundary guard
    let clean_action_src = "export async function send(ctx) { return ctx.config; }";
    assert!(ConfigBoundaryGuard::assert_no_runtime_action_load_config(
        "extensions/telegram/src/send.ts",
        clean_action_src
    ).is_ok());

    let bad_action_src = "export async function send() { return loadConfig(); }";
    let err_action = ConfigBoundaryGuard::assert_no_runtime_action_load_config(
        "extensions/telegram/src/send.ts",
        bad_action_src
    ).unwrap_err();
    assert!(err_action[0].contains("runtime action invokes loadConfig()"));

    // 2. Extension test API consumption guard
    let clean_test_src = "import { testHelper } from '../test-api.js';";
    assert!(ExtensionTestApiConsumptionGuard::assert_test_api_imported_only_in_tests(
        "extensions/telegram/tests/send.test.ts",
        clean_test_src
    ).is_ok());

    let bad_prod_src = "import { testHelper } from './test-api.js';";
    let err_prod = ExtensionTestApiConsumptionGuard::assert_test_api_imported_only_in_tests(
        "extensions/telegram/src/production.ts",
        bad_prod_src
    ).unwrap_err();
    assert!(err_prod[0].contains("production file imports extension test-api"));
}

#[test]
fn test_ocrm_plugins_host_hook_identity_and_gateway_methods() {
    // Tests equivalent behavior from host-hook-state.identity and host-hooks contract test suites.
    use mahayana_host_runtime::extensions::plugins::hook_runner::{
        GatewayPluginHostHooks, NextTurnInjectionManager, PluginSessionExtensionStore,
    };
    use serde_json::json;

    // 1. Next turn injection queue
    let queue = NextTurnInjectionManager::new();
    assert!(queue.is_empty("agent:main:session-1"));
    assert_eq!(queue.drain("agent:main:session-1"), Vec::<String>::new());

    queue.enqueue("agent:main:session-1", "injection 1");
    queue.enqueue("agent:main:session-1", "injection 2");
    assert!(!queue.is_empty("agent:main:session-1"));

    let drained = queue.drain("agent:main:session-1");
    assert_eq!(drained, vec!["injection 1", "injection 2"]);
    assert!(queue.is_empty("agent:main:session-1"));

    // 2. Plugin session extension store
    let store = PluginSessionExtensionStore::new();
    assert!(store.get("agent:main:session-1", "workflow").is_none());

    store.patch("agent:main:session-1", "workflow", json!({"step": 1, "approved": true}));
    assert_eq!(
        store.get("agent:main:session-1", "workflow").unwrap(),
        json!({"step": 1, "approved": true})
    );

    store.clear_owned("agent:main:session-1");
    assert!(store.get("agent:main:session-1", "workflow").is_none());

    // 3. Gateway UI descriptors and session action
    // Without admin scope
    let res_no_admin = GatewayPluginHostHooks::handle_ui_descriptors(&["operator.read".to_string()], 1).unwrap();
    let descs_no_admin = res_no_admin.get("descriptors").unwrap().as_array().unwrap();
    assert_eq!(descs_no_admin.len(), 1);
    assert_eq!(descs_no_admin[0]["id"], "approval-panel");

    // With admin scope
    let res_admin = GatewayPluginHostHooks::handle_ui_descriptors(&["operator.admin".to_string()], 2).unwrap();
    let descs_admin = res_admin.get("descriptors").unwrap().as_array().unwrap();
    assert_eq!(descs_admin.len(), 2);
    assert!(descs_admin.iter().any(|d| d["id"] == "admin-panel"));

    // Session action echo
    let action_res = GatewayPluginHostHooks::handle_session_action("approve", &json!({"id": 123})).unwrap();
    assert!(action_res["ok"].as_bool().unwrap());
    assert_eq!(action_res["action"], "approve");
    assert_eq!(action_res["echo"]["id"], 123);
}

#[test]
fn test_ocrm_plugins_run_context_and_scheduled_turns() {
    // Tests equivalent behavior from run-context-lifecycle, scheduled-turns,
    // and scheduled-turns.test-support contract test suites.
    use mahayana_host_runtime::extensions::plugins::hook_runner::{
        PluginRunContextLifecycle, PluginScheduledTurnsManager, MAX_DATE_TIMESTAMP_MS,
    };
    use serde_json::json;

    // 1. Run context lifecycle
    let lifecycle = PluginRunContextLifecycle::new();
    assert!(lifecycle.get_run_context("run-1", "tool_result").is_none());

    lifecycle.set_run_context("run-1", "tool_result", json!({"status": "success", "count": 1}));
    assert_eq!(
        lifecycle.get_run_context("run-1", "tool_result").unwrap(),
        json!({"status": "success", "count": 1})
    );

    lifecycle.clear_run_context("run-1");
    assert!(lifecycle.get_run_context("run-1", "tool_result").is_none());

    // 2. Scheduled turns manager
    let scheduler = PluginScheduledTurnsManager::new();
    let job_id = scheduler.schedule_turn("agent:main:main", 1_700_000_000_000, "wake up", Some("reminder")).unwrap();
    assert!(job_id.contains("agent:main:main"));

    // Exceeding MAX_DATE_TIMESTAMP_MS is rejected
    let err_max = scheduler.schedule_turn("agent:main:main", MAX_DATE_TIMESTAMP_MS + 1000, "future", None).unwrap_err();
    assert!(err_max.contains("exceeds MAX_DATE_TIMESTAMP_MS"));

    // Unschedule by tag
    let removed = scheduler.unschedule_by_tag("agent:main:main", "reminder");
    assert_eq!(removed, 1);
    let removed_again = scheduler.unschedule_by_tag("agent:main:main", "reminder");
    assert_eq!(removed_again, 0);
}

#[test]
fn test_ocrm_plugins_session_actions_attachments_and_projection() {
    // Tests equivalent behavior from session-actions, session-attachments,
    // and session-entry-projection contract test suites.
    use mahayana_host_runtime::extensions::plugins::hook_runner::{
        PluginSessionActionsManager, PluginSessionAttachmentManager, PluginSessionEntryProjection,
    };
    use serde_json::json;

    // 1. Session actions
    let action_ok = PluginSessionActionsManager::execute_action(
        "agent:main:main",
        "custom_action",
        &["operator.write".to_string()],
        &json!({"key": "val"}),
    ).unwrap();
    assert!(action_ok["ok"].as_bool().unwrap());
    assert_eq!(action_ok["action"], "custom_action");

    let action_err = PluginSessionActionsManager::execute_action(
        "agent:main:main",
        "admin_action",
        &["operator.read".to_string()],
        &json!({}),
    ).unwrap_err();
    assert!(action_err.contains("operator.admin scope required"));

    // 2. Session attachments
    let att_ok = PluginSessionAttachmentManager::send_attachment(
        "agent:main:main",
        "image/png",
        b"fake_png_binary_data",
        "telegram:12345",
    ).unwrap();
    assert!(att_ok.contains("sent:att-agent:main:main-image/png"));

    let att_err = PluginSessionAttachmentManager::send_attachment(
        "agent:main:main",
        "image/png",
        b"",
        "telegram:12345",
    ).unwrap_err();
    assert!(att_err.contains("cannot be empty"));

    // 3. Session entry projection
    let mut entry = json!({"sessionId": "session-123"});
    PluginSessionEntryProjection::project_extension(
        &mut entry,
        "workflow-plugin",
        "state",
        json!({"step": 2, "complete": true}),
    );
    assert_eq!(
        entry["plugins"]["workflow-plugin"]["state"],
        json!({"step": 2, "complete": true})
    );
    assert_eq!(entry["sessionId"], "session-123");

    PluginSessionEntryProjection::strip_extension(&mut entry, "workflow-plugin");
    assert!(entry["plugins"]["workflow-plugin"].is_null());
    assert_eq!(entry["sessionId"], "session-123");
}

#[test]
fn test_ocrm_plugins_agent_executor_tool_middleware_and_before_reply() {
    use std::sync::Arc;
    use serde_json::json;
    use mahayana_host_runtime::extensions::plugins::hook_runner::{
        normalize_agent_prompt_surface_kind, is_openclaw_main_prompt_surface,
        resolve_agent_executor_controller,
        AgentToolResultMiddlewareRuntime, normalize_agent_tool_result_middleware_runtimes,
        AgentToolResultMiddlewareRegistration, AgentToolResultMiddlewarePipeline,
        build_handled_before_agent_reply_payloads, PluginReplyPayload, SILENT_REPLY_TOKEN,
    };

    // 1. Agent prompt surface kind normalization
    assert_eq!(normalize_agent_prompt_surface_kind("pi_main"), "openclaw_main");
    assert_eq!(normalize_agent_prompt_surface_kind("openclaw_main"), "openclaw_main");
    assert_eq!(normalize_agent_prompt_surface_kind("chat_sidebar"), "chat_sidebar");
    assert!(is_openclaw_main_prompt_surface("pi_main"));
    assert!(is_openclaw_main_prompt_surface("openclaw_main"));
    assert!(!is_openclaw_main_prompt_surface("chat_sidebar"));

    // 2. Agent executor controller deprecation error
    let err = resolve_agent_executor_controller("custom-ctrl").unwrap_err();
    assert_eq!(err.capability, "api.registerAgentExecutorController");
    assert!(err.message.contains("Executor controller \"custom-ctrl\" is unavailable"));

    // 3. Tool result middleware normalization and execution pipeline
    let declared_runtimes = vec!["OPENCLAW".into(), "codex".into(), "unknown_runtime".into()];
    let normalized = normalize_agent_tool_result_middleware_runtimes(&declared_runtimes);
    assert_eq!(
        normalized,
        vec![AgentToolResultMiddlewareRuntime::OpenClaw, AgentToolResultMiddlewareRuntime::Codex]
    );

    let pipeline = AgentToolResultMiddlewarePipeline::new();
    pipeline.register(AgentToolResultMiddlewareRegistration {
        plugin_id: "sanitizer-plugin".into(),
        runtimes: vec![AgentToolResultMiddlewareRuntime::OpenClaw],
        tool_names: Some(vec!["web_search".into()]),
        transform: Arc::new(|_tool, val| {
            let mut modified = val.clone();
            if let Some(obj) = modified.as_object_mut() {
                obj.insert("sanitized".into(), json!(true));
            }
            modified
        }),
    });

    // Matching tool and runtime gets transformed
    let raw_search = json!({"results": ["a", "b"]});
    let transformed = pipeline.apply(
        AgentToolResultMiddlewareRuntime::OpenClaw,
        "web_search",
        raw_search.clone(),
    );
    assert_eq!(transformed["sanitized"], json!(true));
    assert_eq!(transformed["results"], json!(["a", "b"]));

    // Non-matching tool is passed through unmodified
    let raw_bash = json!({"output": "hello"});
    let untransformed = pipeline.apply(
        AgentToolResultMiddlewareRuntime::OpenClaw,
        "bash",
        raw_bash.clone(),
    );
    assert_eq!(untransformed, raw_bash);

    // Non-matching runtime is passed through unmodified
    let untransformed_runtime = pipeline.apply(
        AgentToolResultMiddlewareRuntime::Codex,
        "web_search",
        raw_search.clone(),
    );
    assert_eq!(untransformed_runtime, raw_search);

    // 4. Before agent reply silent fallback
    let silent_payloads = build_handled_before_agent_reply_payloads(None);
    assert_eq!(silent_payloads.len(), 1);
    assert_eq!(silent_payloads[0].text, SILENT_REPLY_TOKEN);

    let explicit_payload = build_handled_before_agent_reply_payloads(Some(PluginReplyPayload {
        text: "Hello from agent".into(),
        metadata: Some(json!({"intent": "greeting"})),
    }));
    assert_eq!(explicit_payload.len(), 1);
    assert_eq!(explicit_payload[0].text, "Hello from agent");
    assert_eq!(explicit_payload[0].metadata, Some(json!({"intent": "greeting"})));
}

#[test]
fn test_ocrm_plugins_guarded_api_lifecycle_auth_refs_and_widget_kinds() {
    use std::collections::HashMap;
    use serde_json::json;
    use mahayana_host_runtime::extensions::plugins::runtime_final::{
        GuardedPluginApiLifecycle, PluginAuthProfileRewriter, BoardWidgetContentKindValidator,
    };

    // 1. Guarded Plugin API Lifecycle
    assert!(GuardedPluginApiLifecycle::is_late_callable_method("emitAgentEvent"));
    assert!(GuardedPluginApiLifecycle::is_late_callable_method("sendSessionAttachment"));
    assert!(GuardedPluginApiLifecycle::assert_method_callable_after_activation("emitAgentEvent").is_ok());

    let err_reg = GuardedPluginApiLifecycle::assert_method_callable_after_activation("registerTool").unwrap_err();
    assert!(err_reg.contains("cannot be invoked after plugin activation phase has closed"));

    assert!(GuardedPluginApiLifecycle::assert_workspace_generation_match("gen-1", "gen-1", "my-plugin").is_ok());
    let err_gen = GuardedPluginApiLifecycle::assert_workspace_generation_match("gen-1", "gen-2", "my-plugin").unwrap_err();
    assert!(err_gen.contains("does not belong to the calling workspace generation"));

    // 2. Auth Profile References Rewriter
    let mut id_map = HashMap::new();
    id_map.insert("old-cred-1".into(), "new-cred-1".into());

    let mut cfg = json!({
        "plugins": {
            "entries": {
                "custom-ai": {
                    "config": {
                        "authProfileId": "old-cred-1",
                        "nested": {
                            "defaultAuthProfileId": "old-cred-1"
                        }
                    }
                }
            }
        },
        "channels": {
            "telegram": {
                "authProfileId": "old-cred-1"
            }
        }
    });

    let changed = PluginAuthProfileRewriter::rewrite_auth_profile_refs(&mut cfg, &id_map);
    assert!(changed);
    assert_eq!(cfg["plugins"]["entries"]["custom-ai"]["config"]["authProfileId"], "new-cred-1");
    assert_eq!(cfg["plugins"]["entries"]["custom-ai"]["config"]["nested"]["defaultAuthProfileId"], "new-cred-1");
    assert_eq!(cfg["channels"]["telegram"]["authProfileId"], "new-cred-1");

    // Second run makes no changes
    let changed_second = PluginAuthProfileRewriter::rewrite_auth_profile_refs(&mut cfg, &id_map);
    assert!(!changed_second);

    // 3. Board Widget Content Kind Validation
    let valid_paths = vec!["/public/widget.js".into(), "/public/widget.css".into()];
    assert!(BoardWidgetContentKindValidator::validate(
        "custom-widget",
        "My Custom Widget",
        "main.surface",
        &valid_paths,
    ).is_ok());

    // Reserved kind rejected
    let err_res = BoardWidgetContentKindValidator::validate(
        "html",
        "HTML Widget",
        "main.surface",
        &valid_paths,
    ).unwrap_err();
    assert!(err_res.contains("is reserved"));

    // Invalid kind rejected
    assert!(BoardWidgetContentKindValidator::validate(
        "123-bad",
        "Label",
        "surface",
        &valid_paths,
    ).is_err());

    // Bad path rejected
    let bad_paths = vec!["//double-slash".into()];
    assert!(BoardWidgetContentKindValidator::validate(
        "good-kind",
        "Label",
        "surface",
        &bad_paths,
    ).is_err());
}

#[test]
fn test_ocrm_plugins_bundled_discovery_admission_and_install_guards() {
    use std::collections::{HashMap, HashSet};
    use mahayana_host_runtime::extensions::plugins::installed_index_store::{
        BundledDiscoveryAdmissionValidator, BundledInstallGuard, BundledLoadPathAliasResolver,
    };

    // 1. Bundled Discovery Admission
    let mut admitted = HashSet::new();
    admitted.insert("telegram".to_string());

    assert!(BundledDiscoveryAdmissionValidator::assert_admission_valid(&admitted, "discord", "bundled/plugins/discord").is_ok());
    assert!(BundledDiscoveryAdmissionValidator::assert_admission_valid(&admitted, "", "bundled/plugins/empty").is_err());
    assert!(BundledDiscoveryAdmissionValidator::assert_admission_valid(&admitted, "slack", "").is_err());

    let err_dup = BundledDiscoveryAdmissionValidator::assert_admission_valid(&admitted, "telegram", "bundled/plugins/tg").unwrap_err();
    assert!(err_dup.contains("Duplicate plugin ID 'telegram' already admitted"));

    // 2. Bundled Install Protection Guard
    assert!(BundledInstallGuard::is_immutable_bundled_plugin("telegram"));
    assert!(BundledInstallGuard::is_immutable_bundled_plugin("discord"));
    assert!(!BundledInstallGuard::is_immutable_bundled_plugin("my-custom-plugin"));

    // Production mode rejects mutating immutable plugins
    let err_prod = BundledInstallGuard::assert_can_uninstall_or_overwrite("telegram", false).unwrap_err();
    assert!(err_prod.contains("immutable core bundled plugin and cannot be uninstalled"));

    // Dev mode permits mutation
    assert!(BundledInstallGuard::assert_can_uninstall_or_overwrite("telegram", true).is_ok());

    // Non-bundled plugin permits mutation in prod
    assert!(BundledInstallGuard::assert_can_uninstall_or_overwrite("my-custom-plugin", false).is_ok());

    // 3. Bundled Load Path Aliases
    let mut alias_map = HashMap::new();
    alias_map.insert("bundled/plugins/tg".into(), "/absolute/path/bundled/plugins/telegram".into());

    assert_eq!(
        BundledLoadPathAliasResolver::resolve_alias(&alias_map, "bundled/plugins/tg"),
        "/absolute/path/bundled/plugins/telegram"
    );
    assert_eq!(
        BundledLoadPathAliasResolver::resolve_alias(&alias_map, "bundled/plugins/other"),
        "bundled/plugins/other"
    );
}








