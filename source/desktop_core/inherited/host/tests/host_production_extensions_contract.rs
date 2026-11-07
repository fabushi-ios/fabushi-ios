use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use mahayana_host_runtime::extensions::extension_ids_generated::HostExtensionId;
use mahayana_host_runtime::extensions::registry::HOST_EXTENSION_ORDER;
use mahayana_host_runtime::extensions::inference::extension::{
    INFERENCE_DEPENDENCIES, inference_extension_id,
};
use mahayana_host_runtime::extensions::inference::production::ProductionInferenceExtension;
use mahayana_host_runtime::host_production_extensions::{
    CURRENT_SHIPPING_PRODUCTION_EXTENSION_IDS, ProductionBrowserUaLog,
    ProductionHostExtensions, project_box_disk_pressure, project_conversation_gc,
    project_host_diagnostic, project_host_event_bus_failure, project_session_diagnostic,
};

const PRODUCTION_OWNER: &str = include_str!("../src/host_production_extensions.rs");
const MEMORY_PRODUCTION: &str = include_str!("../src/extensions/memory/production.rs");
const SHIPPING_HOST: &str = include_str!("../app/src/main.rs");

#[test]
fn current_shipping_subset_is_declared_in_the_frozen_36_slot_registry() {
    let all = HOST_EXTENSION_ORDER.iter().copied().collect::<BTreeSet<_>>();
    let shipping = CURRENT_SHIPPING_PRODUCTION_EXTENSION_IDS
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        shipping.len(),
        CURRENT_SHIPPING_PRODUCTION_EXTENSION_IDS.len(),
        "shipping production extension ids must not contain duplicates"
    );
    assert!(
        shipping.iter().all(|id| all.contains(id)),
        "every shipping production extension must occupy a frozen Grok registry slot"
    );
    assert_eq!(HOST_EXTENSION_ORDER.len(), 36);
    assert!(shipping.contains(&HostExtensionId::Auth));
    assert!(shipping.contains(&HostExtensionId::Notifications));
    assert!(shipping.contains(&HostExtensionId::Automations));
    assert!(shipping.contains(&HostExtensionId::Settings));
    assert!(shipping.contains(&HostExtensionId::BrowserUa));
    assert!(shipping.contains(&HostExtensionId::CodebaseTelemetry));
    assert!(shipping.contains(&HostExtensionId::ContentSearch));
    assert!(shipping.contains(&HostExtensionId::Telemetry));
    assert!(shipping.contains(&HostExtensionId::LocalToolPermission));
    assert!(shipping.contains(&HostExtensionId::LocalExec));
    assert!(shipping.contains(&HostExtensionId::CloudAgents));
    assert!(shipping.contains(&HostExtensionId::CloudWorkers));
    assert!(shipping.contains(&HostExtensionId::WebauthnProxy));
    assert!(shipping.contains(&HostExtensionId::StateBackstop));
    assert!(shipping.contains(&HostExtensionId::Inference));
    assert!(shipping.contains(&HostExtensionId::Mcp));
    assert!(shipping.contains(&HostExtensionId::Wallpaper));
    assert!(shipping.contains(&HostExtensionId::ForeverBox));
    assert!(shipping.contains(&HostExtensionId::Attachments));
    assert!(shipping.contains(&HostExtensionId::Secrets));
    assert!(shipping.contains(&HostExtensionId::TurnExecution));
    assert!(shipping.contains(&HostExtensionId::Session));
    assert!(shipping.contains(&HostExtensionId::AutoReview));
    assert!(shipping.contains(&HostExtensionId::Transcript));
    assert_eq!(inference_extension_id(), HostExtensionId::Inference);
    assert_eq!(
        INFERENCE_DEPENDENCIES,
        &[
            HostExtensionId::Auth,
            HostExtensionId::Experiments,
            HostExtensionId::Settings,
        ]
    );
}

#[test]
fn shipping_production_extension_owner_remains_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ProductionHostExtensions>();
    assert_send_sync::<mahayana_host_runtime::host_production_extensions::ProductionHostBoxExtensions>();
    assert_send_sync::<ProductionBrowserUaLog>();
    assert_send_sync::<ProductionInferenceExtension>();
}

#[test]
fn production_host_owns_the_canonical_plugin_capability_catalog() {
    for needle in [
        "pub plugin_capability_catalog: Arc<PluginCapabilityCatalog>",
        "let plugin_capability_catalog = Arc::new(PluginCapabilityCatalog::new());",
        "register_bundled_channel_capabilities(&plugin_capability_catalog)?;",
        "register_bundled_worker_capabilities(&plugin_capability_catalog);",
        "plugin_capability_catalog,",
    ] {
        assert!(
            PRODUCTION_OWNER.contains(needle),
            "ProductionHostExtensions must own the plugin capability catalog: {needle}"
        );
    }
    assert!(
        PRODUCTION_OWNER.contains("pub fn register_channel_delivery_provider("),
        "provider registration must be owned by ProductionHostExtensions"
    );
    assert!(
        PRODUCTION_OWNER.contains("family: CAPABILITY_FAMILY_WORKER_PROVIDERS.into()")
            && PRODUCTION_OWNER.contains("provider_id: \"crabbox\".into()")
            && PRODUCTION_OWNER.contains("plugin_id: \"crabbox\".into()"),
        "bundled Crabbox registration must remain capability metadata only"
    );
    assert!(
        SHIPPING_HOST.contains("plugin_capability_catalog: Arc::clone(&production_extensions.plugin_capability_catalog)")
            && SHIPPING_HOST.contains("if method == \"cloudWorkers.capabilities\"")
            && SHIPPING_HOST.contains("\"metadataOnly\": true"),
        "worker capability metadata must be projected through the existing Host catalog"
    );
    assert!(
        PRODUCTION_OWNER.contains("if self.plugin_capability_catalog.get_channel(channel_id).is_none()"),
        "provider registration must reject channels absent from the capability catalog"
    );
}

#[test]
fn production_host_owns_the_channel_delivery_boundary_consumed_by_gateway() {
    for needle in [
        "pub channel_delivery: HostChannelDelivery",
        "channel_delivery_router: ChannelDeliveryRouter",
        "let channel_delivery_router = ChannelDeliveryRouter::new();",
        "let channel_delivery = HostChannelDelivery::new(Arc::new(channel_delivery_router.clone()));",
        "channel_delivery,",
        "channel_delivery_router,",
    ] {
        assert!(
            PRODUCTION_OWNER.contains(needle),
            "ProductionHostExtensions must own channel delivery: {needle}"
        );
    }
    assert!(
        SHIPPING_HOST.contains("channel_delivery: production_extensions.channel_delivery.clone()"),
        "Gateway must consume the canonical ProductionHostExtensions channel delivery"
    );
    for needle in [
        "pub fn replace_channel_delivery_provider(",
        "pub fn unregister_channel_delivery_provider(",
        "pub fn revoke_bundled_channel(",
    ] {
        assert!(
            PRODUCTION_OWNER.contains(needle),
            "provider lifecycle operation must be owned by ProductionHostExtensions: {needle}"
        );
    }
    assert!(
        PRODUCTION_OWNER.contains("fn register_bundled_channel_capabilities("),
        "production startup must own bundled channel capability discovery"
    );
    assert!(
        PRODUCTION_OWNER.contains("PluginRegistrationMode::Capabilities"),
        "bundled startup discovery must remain capability-only"
    );
}


#[test]
fn browser_ua_and_wallpaper_have_one_production_lifecycle_owner() {
    for needle in [
        "pub browser_ua: Mutex<Option<BrowserUaExtensionRuntime>>",
        "pub wallpaper: Mutex<Option<HostWallpaperExtension>>",
        "let browser_ua = start_production_browser_ua(",
        "let wallpaper = start_wallpaper_extension(",
        "browser_ua: Mutex::new(Some(browser_ua))",
        "wallpaper: Mutex::new(Some(wallpaper))",
    ] {
        assert!(
            PRODUCTION_OWNER.contains(needle),
            "ProductionHostExtensions must own the centralized lifecycle: {needle}"
        );
    }

    for forbidden in [
        "let browser_ua_runtime = start_production_browser_ua(",
        "let mut wallpaper_extension = start_wallpaper_extension(",
        "browser_ua_runtime.stop()",
        "wallpaper_extension.stop()",
    ] {
        assert!(
            !SHIPPING_HOST.contains(forbidden),
            "shipping Host must not recreate a second extension lifecycle: {forbidden}"
        );
    }
}


#[test]
fn box_stage_extensions_have_one_production_composition_owner() {
    for needle in [
        "pub struct ProductionHostBoxExtensions",
        "start_production_host_box_extensions(",
        "let forever_box = start_forever_box_extension_with_telemetry(",
        "core.telemetry.logs.clone()",
        "let attachments = start_attachments_extension(",
        "let secrets = Arc::new(start_secrets_extension(",
    ] {
        assert!(
            PRODUCTION_OWNER.contains(needle),
            "host production composition must own the Box-stage extension: {needle}"
        );
    }

    for forbidden in [
        "let forever_box = start_forever_box_extension(",
        "let attachments_extension = start_attachments_extension(",
        "Arc::new(start_secrets_extension(",
    ] {
        assert!(
            !SHIPPING_HOST.contains(forbidden),
            "shipping Host must not recreate a second Box-stage extension owner: {forbidden}"
        );
    }

    assert!(
        SHIPPING_HOST.contains("start_production_host_box_extensions(&production_extensions, production_box)"),
        "shipping Host must consume the centralized Box-stage owner"
    );
}



#[test]
fn transcript_has_one_shipping_production_lifecycle_owner() {
    for needle in [
        "HostExtensionId::Transcript",
        "transcript: Mutex<Option<TranscriptExtension>>",
        "pub fn start_transcript(",
        "start_production_transcript_extension(",
        "pub fn stop_transcript(&self)",
    ] {
        assert!(
            PRODUCTION_OWNER.contains(needle),
            "ProductionHostExtensions must own Transcript lifecycle: {needle}"
        );
    }
    assert!(
        SHIPPING_HOST.contains("production_extensions.start_transcript("),
        "shipping Host must start Transcript through the centralized production owner"
    );
    assert!(
        SHIPPING_HOST.contains("production_extensions.stop_transcript()"),
        "shipping Host must stop Transcript through the centralized production owner"
    );
    assert!(
        !SHIPPING_HOST.contains("start_production_transcript_extension("),
        "shipping Host must not construct a second Transcript extension"
    );
    assert!(
        !SHIPPING_HOST.contains("drop(transcript_extension)"),
        "shipping Host must not own Transcript teardown"
    );
}


#[test]
fn auto_review_has_one_shipping_production_lifecycle_owner() {
    for needle in [
        "HostExtensionId::AutoReview",
        "auto_review: Mutex<Option<Arc<HostAutoReviewExtension>>>",
        "pub fn start_auto_review(",
        "start_auto_review_extension_with_expire_sweep_telemetry(",
        "pub fn stop_auto_review(&self)",
    ] {
        assert!(
            PRODUCTION_OWNER.contains(needle),
            "ProductionHostExtensions must own AutoReview lifecycle: {needle}"
        );
    }
    assert!(
        SHIPPING_HOST.contains("production_extensions.start_auto_review("),
        "shipping Host must start AutoReview through the centralized production owner"
    );
    assert!(
        SHIPPING_HOST.contains("production_extensions.stop_auto_review()"),
        "shipping Host must stop AutoReview through the centralized production owner"
    );
    assert!(
        !SHIPPING_HOST.contains("start_auto_review_extension_with_expire_sweep_telemetry("),
        "shipping Host must not construct a second AutoReview extension"
    );
}


#[test]
fn session_has_one_shipping_production_lifecycle_owner() {
    for needle in [
        "HostExtensionId::Session",
        "session: Mutex<Option<SessionExtension>>",
        "pub fn start_session(",
        "start_session_extension(",
        "pub fn shutdown_session(&self)",
    ] {
        assert!(
            PRODUCTION_OWNER.contains(needle),
            "ProductionHostExtensions must own Session lifecycle: {needle}"
        );
    }
    assert!(
        SHIPPING_HOST.contains("production_extensions.start_session("),
        "shipping Host must start Session through the centralized production owner"
    );
    assert!(
        SHIPPING_HOST.contains("production_extensions.shutdown_session()"),
        "shipping Host must settle Session through the centralized production owner"
    );
    assert!(
        !SHIPPING_HOST.contains("start_session_extension("),
        "shipping Host must not construct a second Session extension"
    );
}


#[test]
fn cross_user_host_upgrade_and_teach_recording_have_one_shipping_owner() {
    for needle in [
        "HostExtensionId::CrossUserSharing",
        "cross_user: Mutex<Option<Arc<ProductionCrossUserRuntime>>>",
        "pub fn start_cross_user(",
        "pub fn start_cross_user_background_work(&self)",
        "pub fn stop_cross_user(&self)",
        "HostExtensionId::HostUpgrade",
        "host_upgrade: Mutex<Option<Arc<ProductionHostUpgradeExtension>>>",
        "pub fn start_host_upgrade(",
        "start_production_host_upgrade_extension(",
        "pub fn stop_host_upgrade(&self)",
        "HostExtensionId::TeachRecording",
        "teach_recording: Mutex<Option<TeachRecordingExtension<SandTeachRecordingService>>>",
        "pub fn start_teach_recording(",
        "create_teach_recording_extension(&factory, deps)",
        "pub fn stop_teach_recording(&self)",
    ] {
        assert!(
            PRODUCTION_OWNER.contains(needle),
            "ProductionHostExtensions must own the frozen lifecycle: {needle}"
        );
    }
    for required in [
        "production_extensions.start_cross_user(",
        "production_extensions.start_cross_user_background_work()",
        "production_extensions.start_host_upgrade(",
        "production_extensions.start_teach_recording(",
        "production_extensions.stop_cross_user()",
        "production_extensions.stop_host_upgrade()",
        "production_extensions.stop_teach_recording()",
    ] {
        assert!(
            SHIPPING_HOST.contains(required),
            "shipping Host must consume the centralized owner: {required}"
        );
    }
    for forbidden in [
        "ProductionCrossUserRuntime::new(",
        "start_production_host_upgrade_extension(",
        "create_teach_recording_extension(",
    ] {
        assert!(
            !SHIPPING_HOST.contains(forbidden),
            "shipping Host must not construct a duplicate owner: {forbidden}"
        );
    }
}

#[test]
fn shipping_production_composition_covers_the_exact_frozen_36_slots() {
    let frozen = HOST_EXTENSION_ORDER.iter().copied().collect::<BTreeSet<_>>();
    let shipping = CURRENT_SHIPPING_PRODUCTION_EXTENSION_IDS
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    assert_eq!(CURRENT_SHIPPING_PRODUCTION_EXTENSION_IDS.len(), 36);
    assert_eq!(shipping.len(), 36, "shipping production registry must not duplicate slots");
    assert_eq!(shipping, frozen, "shipping production registry must cover the exact frozen slots");
    assert!(
        !PRODUCTION_OWNER.contains("NoopProductionExtension")
            && !PRODUCTION_OWNER.contains("no_op_production_extension"),
        "no frozen slot may be satisfied by a no-op placeholder"
    );
}


#[test]
fn notifications_have_one_shipping_production_lifecycle_owner() {
    for needle in [
        "HostExtensionId::Notifications",
        "notifications: Mutex<Option<HostNotificationsExtension>>",
        "pub fn start_notifications(",
        "start_notifications_extension(",
        "pub fn stop_notifications(&self)",
    ] {
        assert!(
            PRODUCTION_OWNER.contains(needle),
            "ProductionHostExtensions must own Notifications lifecycle: {needle}"
        );
    }
    assert!(
        SHIPPING_HOST.contains("production_extensions.start_notifications("),
        "shipping Host must start Notifications through the centralized production owner"
    );
    assert!(
        SHIPPING_HOST.contains("production_extensions.stop_notifications()"),
        "shipping Host must stop Notifications through the centralized production owner"
    );
    assert!(
        !SHIPPING_HOST.contains("start_notifications_extension("),
        "shipping Host must not construct a second Notifications extension"
    );
}


#[test]
fn turn_execution_has_one_shipping_production_owner() {
    for needle in [
        "HostExtensionId::TurnExecution",
        "pub turn_execution: Arc<Mutex<TurnExecutionRegistry>>",
        "let (_, turn_execution_registry) = turn_execution_extension()",
        "let turn_execution = Arc::new(Mutex::new(turn_execution_registry))",
    ] {
        assert!(
            PRODUCTION_OWNER.contains(needle),
            "ProductionHostExtensions must own TurnExecution lifecycle: {needle}"
        );
    }
    assert!(
        PRODUCTION_OWNER.contains("turn_execution: Arc::clone(&self.turn_execution)"),
        "centralized Transcript composition must inject the unique TurnExecution owner"
    );
    assert!(
        !SHIPPING_HOST.contains("turn_execution_extension()")
            && !SHIPPING_HOST.contains("production_extensions.turn_execution"),
        "shipping Host must neither construct nor reach around the centralized TurnExecution owner"
    );
}


#[test]
fn mcp_plugin_skills_have_one_shipping_production_owner() {
    for needle in [
        "HostExtensionId::Mcp",
        "mcp: Mutex<Option<McpExtensionRuntime>>",
        "pub fn start_mcp(",
        "CoordinatorPluginSkillsLoader::new(relay)",
        "SandPluginSkillsService::new(",
        "McpHostService::new(backend, Some(plugin_port))",
        "PluginSkillsAuthenticatedStartup::start(",
        "cleanup_legacy_mcp_auth_credentials(sand_root_dir)",
        "sweep_legacy_plugin_skill_references(&sweep_root, &catalog)",
        "Some(on_startup_sync_succeeded)",
        "RealPluginSkillsPolling::daily()",
        "create_production_skill_publish(",
        "pub fn stop_mcp(&self)",
    ] {
        assert!(
            PRODUCTION_OWNER.contains(needle),
            "ProductionHostExtensions must own the MCP production lifecycle: {needle}"
        );
    }

    assert!(
        SHIPPING_HOST.contains("production_extensions.start_mcp("),
        "shipping Host must start MCP through the centralized production extension owner"
    );
    assert!(
        SHIPPING_HOST.contains("production_extensions.stop_mcp()"),
        "shipping Host must stop MCP through the centralized production extension owner"
    );
    assert!(
        !SHIPPING_HOST.contains("McpHostService::new(mcp_manager_backend, None)"),
        "shipping Host must never bypass Plugin Skills with a None port"
    );
}


#[test]
fn automations_have_one_shipping_production_owner() {
    for needle in [
        "HostExtensionId::Automations",
        "automations: Mutex<Option<Arc<ProductionAutomationsLifecycle>>>",
        "pub fn start_automations(",
        "ProductionAutomationsLifecycle::start(",
        "pub fn stop_automations(&self)",
    ] {
        assert!(
            PRODUCTION_OWNER.contains(needle),
            "ProductionHostExtensions must own the Automations production lifecycle: {needle}"
        );
    }
}

#[test]
fn state_backstop_is_composed_after_box_store_sync_and_stopped_before_it() {
    for needle in [
        "HostExtensionId::StateBackstop",
        "state_backstop: Mutex<Option<HostStateBackstopExtension>>",
        "let box_api = extension.api()",
        "create_production_state_backstop_runtime()",
        "store_api.object_store_for(source_id)",
        "let source_map = Arc::clone(&self.source_map)",
        ".get_or_create(agent_id)",
        "start_state_backstop_extension(options)",
        "state_backstop.stop()",
    ] {
        assert!(
            PRODUCTION_OWNER.contains(needle),
            "production Host must own frozen StateBackstop composition: {needle}"
        );
    }
}


#[test]
fn memory_synthesis_is_pinned_to_authenticated_statsig_and_shipping_inference() {
    for needle in [
        "start_authenticated_statsig_bootstrap(",
        "pin_gate_on_authenticated_bootstrap(",
        "\"sand_memory_dreaming\"",
        "MemorySynthesisReport::SkippedGate",
        "create_production_memory_synthesis(",
    ] {
        assert!(
            PRODUCTION_OWNER.contains(needle),
            "production Host must preserve frozen authenticated Memory synthesis wiring: {needle}"
        );
    }
    assert!(
        !PRODUCTION_OWNER.contains("if experiments.check_feature_gate(\"sand_memory_dreaming\")"),
        "Memory synthesis must not read the unpinned startup fallback before authenticated Statsig bootstrap"
    );

    for needle in [
        "run_summarization_prompt(",
        "synthesis_system_prompt()",
        "verification_system_prompt()",
        "is_cancelled()",
        "memory_synthesis_telemetry_report",
        "report_memory_synthesis",
        "ProductionSynthesisTarget",
        "apply_synthesis_with_gate",
    ] {
        assert!(
            MEMORY_PRODUCTION.contains(needle),
            "shipping Memory production must consume the shared inference/telemetry path: {needle}"
        );
    }
    assert!(
        SHIPPING_HOST.contains("bind_memory_synthesis_gate()"),
        "shipping Host must bind Memory synthesis to the session owner before runtime use"
    );
}

#[test]
fn structured_log_domain_reporter_adapters_preserve_frozen_fields() {
    let subscriber = project_host_event_bus_failure(&json!({
        "kind": "subscriber_failed",
        "topic": "turn.updated",
        "errorClass": "Error"
    })).expect("event bus subscriber projection");
    assert_eq!(subscriber.kind, "subscriber_failed");
    assert_eq!(subscriber.topic.as_deref(), Some("turn.updated"));
    assert_eq!(subscriber.error_class, "Error");
    let listener = project_host_event_bus_failure(&json!({
        "kind": "listener_failed",
        "errorClass": "panic"
    })).expect("event bus listener projection");
    assert_eq!(listener.topic, None);

    let host = project_host_diagnostic(&mahayana_host_runtime::host_diagnostics::HostDiagnostic {
        kind: "send_ledger_degraded".into(),
        fields: Map::from_iter([
            ("stage".into(), json!("commit")),
            ("agentId".into(), json!("agent-1")),
            ("reason".into(), json!("io")),
            ("errorClass".into(), json!("disk")),
        ]),
    });
    assert_eq!(host.stage.as_deref(), Some("commit"));
    assert_eq!(host.agent_id.as_deref(), Some("agent-1"));
    assert_eq!(host.error_class.as_deref(), Some("disk"));

    let disk = project_box_disk_pressure(&mahayana_host_runtime::host_diagnostics::HostDiagnostic {
        kind: "disk_pressure".into(),
        fields: Map::from_iter([
            ("level".into(), json!("hard")),
            ("volume".into(), json!("/")),
            ("trigger".into(), json!("heartbeat")),
            ("totalBytes".into(), json!(1000.0)),
            ("availableBytes".into(), json!(100.0)),
            ("usedPercent".into(), json!(90.04)),
        ]),
    }).expect("disk pressure projection");
    assert_eq!(disk.level, "hard");
    assert_eq!(disk.used_percent, 90.04);

    let session = project_session_diagnostic(
        &mahayana_host_runtime::extensions::session::session_diagnostics::SessionDiagnostic {
            family: "store_db".into(),
            kind: "quarantine_copied".into(),
            metadata: BTreeMap::from([
                ("agentId".into(), Value::String("agent-1".into())),
                ("outcome".into(), Value::String("recovered".into())),
                ("salvagedKv".into(), json!(3)),
            ]),
        },
    )
    .expect("known session family");
    assert_eq!(session.agent_id.as_deref(), Some("agent-1"));
    assert_eq!(session.outcome.as_deref(), Some("recovered"));
    assert_eq!(session.salvaged_kv, Some(3));

    let gc = project_conversation_gc(
        &mahayana_host_runtime::extensions::session::conversation_size_limits::ConversationGcReport {
            trigger: "size-cap".into(),
            agent_id: "agent-1".into(),
            outcome: "skipped".into(),
            skip_reason: Some("unresolved-refs".into()),
            unresolved_proto_refs: Some(4),
            deleted_rows: None,
            deleted_bytes: None,
            live_rows: None,
            live_bytes: None,
            vacuumed: None,
            still_over_cap: true,
        },
    );
    match gc {
        mahayana_host_runtime::extensions::telemetry::conversation_gc_telemetry::ConversationGcReport::Skipped {
            skip_reason,
            unresolved_proto_refs,
            ..
        } => {
            assert_eq!(skip_reason, "unresolved-refs");
            assert_eq!(unresolved_proto_refs, Some(4.0));
        }
        other => panic!("unexpected GC projection: {other:?}"),
    }
}
