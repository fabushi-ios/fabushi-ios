use sha2::Digest;
use mahayana_host_runtime::extensions::plugins::activation_planner::{
    ActivationPlanner, ActivationTrigger, PluginManifestRecord,
};
use mahayana_host_runtime::extensions::plugins::bundle_manifest::{
    encode_hex, parse_clawhub_plugin_spec, verify_archive_limits, verify_sha256_integrity,
    ClawHubPluginSpec, PluginBundleFormat, PluginManifest, AGENT_BUNDLE_MANIFEST_RELATIVE_PATH,
    CLAUDE_BUNDLE_MANIFEST_RELATIVE_PATH, CODEX_BUNDLE_MANIFEST_RELATIVE_PATH,
    CURSOR_BUNDLE_MANIFEST_RELATIVE_PATH, DEFAULT_MAX_ARCHIVE_BYTES_ZIP,
    DEFAULT_MAX_ENTRIES, DEFAULT_MAX_EXTRACTED_BYTES,
};
use mahayana_host_runtime::extensions::plugins::capability_catalog::{
    CapabilityProviderEntry, ChannelCatalogEntry, PluginCapabilityCatalog,
    PluginContractRegistryEntry, PluginOrigin, ProviderAuthChoice,
    CAPABILITY_FAMILY_SPEECH_PROVIDERS, CAPABILITY_FAMILY_TRANSCRIPTION_PROVIDERS,
    BundledPluginScanner, BundledProviderCompatValidator, BundledSourceOverlayResolver,
    BundledPluginNamingAndArtworkValidator,
    BundledPluginSource, BundledPluginLookup, BundledPluginSourceResolver,
    CandidateInstallOwnerResolver,
    CapabilityCatalogFamily, CapabilityCatalogValidator,
    PluginAcceptedDeclaredSurface, CapabilityConsentManager,
    PluginRuntimeCapabilityLease, CapabilitySummaryBuilder,
    CapturedPluginRegistration, PluginCapabilityAcquisitionScope,
    CatalogDiscoveryCodec, CatalogIconRegistry,
    CatalogPackageSearchResult, CatalogSearchRanking,
    ChannelCatalogCandidate, ChannelCatalogResolver,
};

#[test]
fn test_ocrm_plugins_bundle_manifest() {
    // 1. Bundle manifest paths
    assert_eq!(
        PluginBundleFormat::Codex.manifest_relative_path(),
        CODEX_BUNDLE_MANIFEST_RELATIVE_PATH
    );
    assert_eq!(
        PluginBundleFormat::Claude.manifest_relative_path(),
        CLAUDE_BUNDLE_MANIFEST_RELATIVE_PATH
    );
    assert_eq!(
        PluginBundleFormat::Cursor.manifest_relative_path(),
        CURSOR_BUNDLE_MANIFEST_RELATIVE_PATH
    );
    assert_eq!(
        PluginBundleFormat::Agent.manifest_relative_path(),
        AGENT_BUNDLE_MANIFEST_RELATIVE_PATH
    );

    // 2. ClawHub plugin spec parsing
    let scoped = parse_clawhub_plugin_spec("@openclaw/browser-tools@1.4.2").expect("Parse scoped spec");
    assert_eq!(
        scoped,
        ClawHubPluginSpec {
            scope: Some("openclaw".to_string()),
            name: "browser-tools".to_string(),
            version: "1.4.2".to_string(),
        }
    );

    let unscoped = parse_clawhub_plugin_spec("discord-bot@latest").expect("Parse unscoped spec");
    assert_eq!(
        unscoped,
        ClawHubPluginSpec {
            scope: None,
            name: "discord-bot".to_string(),
            version: "latest".to_string(),
        }
    );

    let default_latest = parse_clawhub_plugin_spec("@vendor/my-pack").expect("Parse default latest");
    assert_eq!(default_latest.version, "latest");
    assert_eq!(default_latest.scope, Some("vendor".to_string()));

    // 3. Archive limits
    assert!(verify_archive_limits(1024, 2048, 10).is_ok());
    assert!(verify_archive_limits(DEFAULT_MAX_ARCHIVE_BYTES_ZIP + 1, 1024, 10).is_err());
    assert!(verify_archive_limits(1024, DEFAULT_MAX_EXTRACTED_BYTES + 1, 10).is_err());
    assert!(verify_archive_limits(1024, 1024, DEFAULT_MAX_ENTRIES + 1).is_err());

    // 4. SHA-256 integrity
    let test_data = b"openclaw-plugin-bundle-data";
    // verify with hex
    let hex_digest = encode_hex(&sha2::Sha256::digest(test_data));
    assert!(verify_sha256_integrity(test_data, &hex_digest).is_ok());
    assert!(verify_sha256_integrity(test_data, "bad-digest").is_err());

    // 5. Package manifest authoring validation (package-manifest.contract.test.ts)
    use mahayana_host_runtime::extensions::plugins::bundle_manifest::{
        resolve_package_extension_entries, validate_package_extension_entries_for_install,
        OpenClawMetadata,
    };
    use serde_json::json;

    // Valid package manifest
    let valid_json = json!({
        "openclaw": {
            "extensions": ["./src/index.ts"],
            "runtimeExtensions": ["./dist/index.js"],
            "setupEntry": "./src/setup.ts",
            "runtimeSetupEntry": "./dist/setup.js"
        }
    });
    let res = resolve_package_extension_entries(&valid_json);
    assert_eq!(res.status, "ok");
    assert_eq!(res.entries, vec!["./src/index.ts"]);

    // Non-object openclaw metadata
    let non_obj = json!({"openclaw": "invalid"});
    let res_non_obj = resolve_package_extension_entries(&non_obj);
    assert_eq!(res_non_obj.status, "invalid");
    assert_eq!(
        res_non_obj.error.as_deref(),
        Some("package.json openclaw must be an object")
    );

    // Non-array extensions
    let non_arr = json!({"openclaw": {"extensions": "./index.js"}});
    let res_non_arr = resolve_package_extension_entries(&non_arr);
    assert_eq!(res_non_arr.status, "invalid");
    assert_eq!(
        res_non_arr.error.as_deref(),
        Some("package.json openclaw.extensions must be an array")
    );

    // Blank extension
    let blank = json!({"openclaw": {"extensions": [" "]}});
    let res_blank = resolve_package_extension_entries(&blank);
    assert_eq!(res_blank.status, "invalid");
    assert_eq!(
        res_blank.error.as_deref(),
        Some("package.json openclaw.extensions[0] must be a non-empty string")
    );

    // Runtime extensions length mismatch
    let mismatch_meta = OpenClawMetadata {
        extensions: vec!["./src/one.ts".to_string(), "./src/two.ts".to_string()],
        runtime_extensions: vec!["./dist/one.js".to_string()],
        setup_entry: None,
        runtime_setup_entry: None,
    };
    assert!(validate_package_extension_entries_for_install(&mismatch_meta).is_err());

    // Runtime setup without source setup
    let runtime_setup_only = OpenClawMetadata {
        extensions: vec![],
        runtime_extensions: vec![],
        setup_entry: None,
        runtime_setup_entry: Some("./dist/setup.js".to_string()),
    };
    assert!(validate_package_extension_entries_for_install(&runtime_setup_only).is_err());

    // Tool manifest contracts validation (plugin-tool-contracts.test.ts)
    use mahayana_host_runtime::extensions::plugins::bundle_manifest::{
        validate_plugin_manifest_tool_contracts, VirtualTestPluginHarness,
    };

    // Valid: all registered tools declared
    assert!(validate_plugin_manifest_tool_contracts(
        "my-plugin",
        &["tool_a".to_string(), "tool_b".to_string()],
        &["tool_a".to_string()],
    ).is_ok());

    // Invalid: registers tools but has no contracts.tools
    let err_no_tools = validate_plugin_manifest_tool_contracts(
        "bad-plugin",
        &[],
        &["tool_a".to_string()],
    ).unwrap_err();
    assert_eq!(err_no_tools, "bad-plugin: registers agent tools but has no contracts.tools");

    // Invalid: missing contracts.tools for specific tools
    let err_missing = validate_plugin_manifest_tool_contracts(
        "bad-plugin",
        &["tool_a".to_string()],
        &["tool_a".to_string(), "tool_b".to_string()],
    ).unwrap_err();
    assert_eq!(err_missing, "bad-plugin: missing contracts.tools for tool_b");

    // Virtual test plugin tool execution (plugin-testing-harness.contract.test.ts)
    let outcome = VirtualTestPluginHarness::execute_fixture_echo_tool(
        "scoped",
        "/tmp/virtual-home",
        "call-1",
        "alpha",
    );
    assert_eq!(outcome.text, "scoped:alpha");
    assert_eq!(outcome.home, "/tmp/virtual-home");
    assert_eq!(outcome.message, "alpha");

    // 8. PluginManifest model validation (OCRM-RESP-PLUGINS-BUNDLE-MANIFEST-001)
    let valid_manifest = PluginManifest::new("openclaw-sample-plugin", "1.2.0");
    assert!(valid_manifest.validate().is_ok());

    let mut invalid_manifest = PluginManifest::new("", "1.2.0");
    assert!(invalid_manifest.validate().is_err());

    invalid_manifest.name = "sample".to_string();
    invalid_manifest.entry = Some("../outside/index.js".to_string());
    assert!(invalid_manifest.validate().is_err());
}

#[test]
fn test_ocrm_plugins_activation_planner() {
    let mut planner = ActivationPlanner::new();

    // Bundled plugin
    planner.register_manifest(PluginManifestRecord {
        plugin_id: "bundled-git".to_string(),
        commands: vec!["git:commit".to_string(), "git:status".to_string()],
        providers: vec![],
        channels: vec![],
        capabilities: vec!["vcs".to_string()],
        is_bundled: true,
        is_trusted: true,
    });

    // Trusted external plugin
    planner.register_manifest(PluginManifestRecord {
        plugin_id: "slack-channel".to_string(),
        commands: vec!["slack:send".to_string()],
        providers: vec![],
        channels: vec!["slack".to_string()],
        capabilities: vec![],
        is_bundled: false,
        is_trusted: true,
    });

    // Untrusted external plugin
    planner.register_manifest(PluginManifestRecord {
        plugin_id: "untrusted-addon".to_string(),
        commands: vec!["exploit:run".to_string()],
        providers: vec![],
        channels: vec![],
        capabilities: vec!["dangerous".to_string()],
        is_bundled: false,
        is_trusted: false,
    });

    // Plan with commands
    let plan1 = planner.plan_activation(&[ActivationTrigger::Command("git:commit".to_string())]);
    assert_eq!(plan1, vec!["bundled-git"]);

    // Plan with channel
    let plan2 = planner.plan_activation(&[ActivationTrigger::Channel("slack".to_string())]);
    assert_eq!(plan2, vec!["slack-channel"]);

    // Untrusted plugin must NOT be activated
    let plan3 = planner.plan_activation(&[ActivationTrigger::Command("exploit:run".to_string())]);
    assert!(plan3.is_empty());
}

#[test]
fn test_ocrm_plugins_capability_catalog() {
    use mahayana_host_runtime::extensions::plugin_sdk::channel_entry::{
        BundledChannelEntryContract, PluginRegistrationMode,
    };

    let catalog = PluginCapabilityCatalog::new();

    // Register capability
    catalog.register_capability(CapabilityProviderEntry {
        family: CAPABILITY_FAMILY_SPEECH_PROVIDERS.to_string(),
        provider_id: "elevenlabs".to_string(),
        plugin_id: "elevenlabs-voice-plugin".to_string(),
        label: Some("ElevenLabs Text-to-Speech".to_string()),
        aliases: vec![],
    });
    catalog.register_capability(CapabilityProviderEntry {
        family: CAPABILITY_FAMILY_TRANSCRIPTION_PROVIDERS.to_string(),
        provider_id: "whisper-local".to_string(),
        plugin_id: "whisper-plugin".to_string(),
        label: Some("Local Whisper".to_string()),
        aliases: vec!["whisper".to_string()],
    });

    let entry = catalog
        .get_capability(CAPABILITY_FAMILY_SPEECH_PROVIDERS, "ElevenLabs")
        .expect("Case-insensitive lookup");
    assert_eq!(entry.plugin_id, "elevenlabs-voice-plugin");

    let speech_list = catalog.list_capabilities_for_family(CAPABILITY_FAMILY_SPEECH_PROVIDERS);
    assert_eq!(speech_list.len(), 1);

    // Cold descriptor matching with aliases (capability-catalog.contract.test.ts)
    assert!(catalog.matches_provider_id_or_alias(CAPABILITY_FAMILY_TRANSCRIPTION_PROVIDERS, "whisper-local"));
    assert!(catalog.matches_provider_id_or_alias(CAPABILITY_FAMILY_TRANSCRIPTION_PROVIDERS, "whisper"));
    assert!(!catalog.matches_provider_id_or_alias(CAPABILITY_FAMILY_TRANSCRIPTION_PROVIDERS, "deepgram"));

    // Channel catalog
    catalog.register_channel(ChannelCatalogEntry {
        plugin_id: "discord-plugin".to_string(),
        origin: PluginOrigin::Bundled,
        package_name: Some("@openclaw/discord".to_string()),
        channel_name: "discord".to_string(),
        trusted_official_install: true,
    });

    let channel = catalog.get_channel("DISCORD").expect("Channel lookup");
    assert_eq!(channel.plugin_id, "discord-plugin");
    assert!(channel.trusted_official_install);

    let bundled = BundledChannelEntryContract::new("telegram", "Telegram", "Bundled Telegram")
        .with_handler(|mode| match mode {
            PluginRegistrationMode::Full => vec!["telegram-full".to_string()],
            _ => vec!["telegram-mode".to_string()],
        });
    assert_eq!(
        catalog
            .register_bundled_channel_entry(
                &bundled,
                PluginOrigin::Bundled,
                Some("@openclaw/telegram".to_string()),
                PluginRegistrationMode::Full,
            )
            .expect("first bundled registration"),
        vec!["telegram-full"]
    );
    let registered = catalog.get_channel("TELEGRAM").expect("Bundled entry registration");
    assert_eq!(registered.plugin_id, "telegram");
    assert_eq!(registered.package_name.as_deref(), Some("@openclaw/telegram"));
    assert!(registered.trusted_official_install);
    assert!(catalog
        .register_bundled_channel_entry(
            &bundled,
            PluginOrigin::Bundled,
            Some("@openclaw/telegram".to_string()),
            PluginRegistrationMode::Full,
        )
        .is_err());
    assert_eq!(catalog.unregister_channel("telegram").unwrap().plugin_id, "telegram");
    assert!(catalog.get_channel("telegram").is_none());
}

#[test]
fn test_ocrm_plugins_contract_registry_oracle() {
    // Tests equivalent behavior from upstream src/plugins/contracts/registry.contract.test.ts
    let catalog = PluginCapabilityCatalog::new();

    // 1. Canonical-first aliases oracle
    catalog.register_canonical_aliases(
        "azure-speech:speechProviderIds",
        vec!["azure-speech".to_string(), "azure".to_string()],
    );
    catalog.register_canonical_aliases(
        "volcengine:speechProviderIds",
        vec!["volcengine".to_string(), "bytedance".to_string(), "doubao".to_string()],
    );
    catalog.register_canonical_aliases(
        "xai:realtimeVoiceProviderIds",
        vec!["xai".to_string(), "grok-voice".to_string(), "xai-realtime-voice".to_string()],
    );

    assert_eq!(
        catalog.get_canonical_aliases("azure-speech:speechProviderIds"),
        Some(vec!["azure-speech".to_string(), "azure".to_string()])
    );
    assert_eq!(
        catalog.get_canonical_aliases("volcengine:speechProviderIds"),
        Some(vec!["volcengine".to_string(), "bytedance".to_string(), "doubao".to_string()])
    );
    assert_eq!(
        catalog.get_canonical_aliases("xai:realtimeVoiceProviderIds"),
        Some(vec!["xai".to_string(), "grok-voice".to_string(), "xai-realtime-voice".to_string()])
    );

    // 2. Provider ID uniqueness validator
    let valid_entries = vec![
        PluginContractRegistryEntry {
            plugin_id: "plugin-a".to_string(),
            provider_ids: vec!["provider-1".to_string(), "provider-2".to_string()],
            worker_provider_ids: vec![],
            web_fetch_provider_ids: vec![],
            web_search_provider_ids: vec![],
            speech_provider_ids: vec![],
            realtime_voice_provider_ids: vec![],
            realtime_transcription_provider_ids: vec![],
            image_generation_provider_ids: vec![],
        },
        PluginContractRegistryEntry {
            plugin_id: "plugin-b".to_string(),
            provider_ids: vec!["provider-3".to_string()],
            worker_provider_ids: vec![],
            web_fetch_provider_ids: vec![],
            web_search_provider_ids: vec![],
            speech_provider_ids: vec![],
            realtime_voice_provider_ids: vec![],
            realtime_transcription_provider_ids: vec![],
            image_generation_provider_ids: vec![],
        },
    ];
    assert!(PluginCapabilityCatalog::validate_unique_provider_ids(&valid_entries).is_ok());

    let duplicate_entries = vec![
        PluginContractRegistryEntry {
            plugin_id: "plugin-a".to_string(),
            provider_ids: vec!["provider-dup".to_string()],
            worker_provider_ids: vec![],
            web_fetch_provider_ids: vec![],
            web_search_provider_ids: vec![],
            speech_provider_ids: vec![],
            realtime_voice_provider_ids: vec![],
            realtime_transcription_provider_ids: vec![],
            image_generation_provider_ids: vec![],
        },
        PluginContractRegistryEntry {
            plugin_id: "plugin-b".to_string(),
            provider_ids: vec!["provider-dup".to_string()],
            worker_provider_ids: vec![],
            web_fetch_provider_ids: vec![],
            web_search_provider_ids: vec![],
            speech_provider_ids: vec![],
            realtime_voice_provider_ids: vec![],
            realtime_transcription_provider_ids: vec![],
            image_generation_provider_ids: vec![],
        },
    ];
    assert!(PluginCapabilityCatalog::validate_unique_provider_ids(&duplicate_entries).is_err());

    // 3. Onboarding separation: video/image-only provider auth choices kept out of text onboarding
    catalog.register_auth_choice(ProviderAuthChoice {
        provider: "alibaba".to_string(),
        method: "api-key".to_string(),
        choice_id: "alibaba-model-studio-api-key".to_string(),
        choice_label: "Alibaba Model Studio API key".to_string(),
        onboarding_scopes: vec!["image-generation".to_string()],
        cli_flag: Some("--alibaba-model-studio-api-key".to_string()),
        option_key: Some("alibabaModelStudioApiKey".to_string()),
    });
    catalog.register_auth_choice(ProviderAuthChoice {
        provider: "github-copilot".to_string(),
        method: "device".to_string(),
        choice_id: "github-copilot".to_string(),
        choice_label: "GitHub Copilot".to_string(),
        onboarding_scopes: vec!["text-chat".to_string(), "code".to_string()],
        cli_flag: Some("--github-copilot-token".to_string()),
        option_key: Some("githubCopilotToken".to_string()),
    });

    let filtered = catalog.filter_text_onboarding_auth_choices();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].provider, "github-copilot");

    // 4. Shared resolver for web search with activation-scoped overrides
    let bundled_search = vec!["bing".to_string(), "google".to_string(), "duckduckgo".to_string()];
    let activation_scoped = vec!["codex".to_string(), "qa-lab".to_string()];
    let resolved_search = PluginCapabilityCatalog::resolve_web_search_providers(&bundled_search, &activation_scoped);
    assert_eq!(
        resolved_search,
        vec!["bing", "codex", "duckduckgo", "google", "qa-lab"]
    );
}

#[test]
fn test_ocrm_plugins_providers_and_web_fetch_contract() {
    // Tests behavior from providers.contract.test.ts and web-fetch-provider.contract.test.ts
    let bundled_providers = [
        "anthropic",
        "fal",
        "google",
        "minimax",
        "moonshot",
        "openai",
        "openrouter",
        "xai",
    ];
    let bundled_web_search = [
        "brave",
        "duckduckgo",
        "exa",
        "firecrawl",
        "google",
        "minimax",
        "moonshot",
        "parallel",
        "perplexity",
        "tavily",
        "xai",
    ];

    assert_eq!(bundled_providers.len(), 8);
    assert_eq!(bundled_web_search.len(), 11);

    let entries = vec![
        PluginContractRegistryEntry {
            plugin_id: "google-search-plugin".to_string(),
            provider_ids: vec!["google".to_string()],
            worker_provider_ids: vec![],
            web_fetch_provider_ids: vec!["google-fetch".to_string()],
            web_search_provider_ids: vec!["google".to_string()],
            speech_provider_ids: vec![],
            realtime_voice_provider_ids: vec![],
            realtime_transcription_provider_ids: vec![],
            image_generation_provider_ids: vec![],
        },
        PluginContractRegistryEntry {
            plugin_id: "anthropic-plugin".to_string(),
            provider_ids: vec!["anthropic".to_string()],
            worker_provider_ids: vec![],
            web_fetch_provider_ids: vec![],
            web_search_provider_ids: vec![],
            speech_provider_ids: vec![],
            realtime_voice_provider_ids: vec![],
            realtime_transcription_provider_ids: vec![],
            image_generation_provider_ids: vec![],
        },
    ];

    let fetch_plugins: Vec<&str> = entries
        .iter()
        .filter(|e| !e.web_fetch_provider_ids.is_empty())
        .map(|e| e.plugin_id.as_str())
        .collect();
    assert_eq!(fetch_plugins, vec!["google-search-plugin"]);
}

#[test]
fn test_ocrm_plugins_shape_compatibility_matrix() {
    // Tests equivalent behavior from upstream src/plugins/contracts/shape.contract.test.ts
    use mahayana_host_runtime::extensions::plugins::activation_planner::{
        build_plugin_shape_summary, CapabilityMode, PluginCapabilityEntry, PluginInspectShape,
        PluginShapeParams,
    };

    // 1. Hook only
    let hook_only = build_plugin_shape_summary(&PluginShapeParams {
        plugin_id: "hook-only".to_string(),
        has_hooks: true,
        ..Default::default()
    });
    assert_eq!(hook_only.shape, PluginInspectShape::HookOnly);
    assert_eq!(hook_only.capability_mode, CapabilityMode::None);
    assert!(hook_only.capabilities.is_empty());

    // 2. Plain capability (text-inference)
    let plain_provider = build_plugin_shape_summary(&PluginShapeParams {
        plugin_id: "plain-provider".to_string(),
        capabilities: vec![PluginCapabilityEntry {
            kind: "text-inference".to_string(),
            ids: vec!["plain-provider".to_string()],
        }],
        ..Default::default()
    });
    assert_eq!(plain_provider.shape, PluginInspectShape::PlainCapability);
    assert_eq!(plain_provider.capability_mode, CapabilityMode::Plain);
    assert_eq!(plain_provider.capabilities.len(), 1);

    // 3. Hybrid capability (text-inference + web-search)
    let hybrid = build_plugin_shape_summary(&PluginShapeParams {
        plugin_id: "hybrid-company".to_string(),
        capabilities: vec![
            PluginCapabilityEntry {
                kind: "text-inference".to_string(),
                ids: vec!["hybrid-provider".to_string()],
            },
            PluginCapabilityEntry {
                kind: "web-search".to_string(),
                ids: vec!["hybrid-search".to_string()],
            },
        ],
        ..Default::default()
    });
    assert_eq!(hybrid.shape, PluginInspectShape::HybridCapability);
    assert_eq!(hybrid.capability_mode, CapabilityMode::Hybrid);
    assert_eq!(hybrid.capabilities.len(), 2);

    // 4. Channel demo
    let channel_demo = build_plugin_shape_summary(&PluginShapeParams {
        plugin_id: "channel-demo".to_string(),
        capabilities: vec![PluginCapabilityEntry {
            kind: "channel".to_string(),
            ids: vec!["channel-demo".to_string()],
        }],
        ..Default::default()
    });
    assert_eq!(channel_demo.shape, PluginInspectShape::PlainCapability);
    assert_eq!(channel_demo.capability_mode, CapabilityMode::Plain);

    // 5. Session catalog demo
    let session_catalog_demo = build_plugin_shape_summary(&PluginShapeParams {
        plugin_id: "session-catalog-demo".to_string(),
        capabilities: vec![PluginCapabilityEntry {
            kind: "session-catalog".to_string(),
            ids: vec!["session-catalog-demo".to_string()],
        }],
        ..Default::default()
    });
    assert_eq!(session_catalog_demo.shape, PluginInspectShape::PlainCapability);
    assert_eq!(session_catalog_demo.capability_mode, CapabilityMode::Plain);

    // 6. Document extractors
    let doc_extract = build_plugin_shape_summary(&PluginShapeParams {
        plugin_id: "document-extract-test".to_string(),
        capabilities: vec![PluginCapabilityEntry {
            kind: "document-extractors".to_string(),
            ids: vec!["pdf".to_string()],
        }],
        ..Default::default()
    });
    assert_eq!(doc_extract.shape, PluginInspectShape::PlainCapability);
    assert_eq!(doc_extract.capability_mode, CapabilityMode::Plain);
}

#[test]
fn test_ocrm_plugins_registry_public_artifacts_retry_precedence() {
    // Tests equivalent behavior from upstream src/plugins/contracts/registry.retry.test.ts
    use std::collections::HashMap;

    let catalog = PluginCapabilityCatalog::new();
    let mut public_artifacts = HashMap::new();
    public_artifacts.insert("openai".to_string(), vec!["openai".to_string()]);
    public_artifacts.insert("google".to_string(), vec!["gemini".to_string()]);

    // 1. Uses provider public artifacts before falling back to bundled runtime registry
    let resolved_providers = catalog.resolve_provider_contract_providers_for_plugin_ids(
        &["openai"],
        &public_artifacts,
    );
    assert_eq!(resolved_providers, vec!["openai"]);

    // 2. Uses web search public artifacts before falling back to bundled runtime registry
    let resolved_web_search = catalog.resolve_web_search_providers_for_plugin_id(
        "google",
        &public_artifacts,
    );
    assert_eq!(resolved_web_search, vec!["gemini"]);
}

#[test]
fn test_ocrm_plugins_shared_upstream_model_contract() {
    // Tests equivalent behavior from upstream src/plugins/contracts/shared-upstream-model.contract.test.ts
    use mahayana_host_runtime::extensions::plugins::capability_catalog::{
        ModelCatalogRow, SharedUpstreamModelCatalog,
    };

    let valid_catalog = vec![
        ModelCatalogRow {
            provider_id: "openai".to_string(),
            model_id: "gpt-4o".to_string(),
            upstream_model: None,
            code_mode: Some("preferred".to_string()),
        },
        ModelCatalogRow {
            provider_id: "azure".to_string(),
            model_id: "gpt-4o".to_string(),
            upstream_model: Some("openai/gpt-4o".to_string()),
            code_mode: Some("preferred".to_string()),
        },
        ModelCatalogRow {
            provider_id: "anthropic".to_string(),
            model_id: "claude-3-5-sonnet".to_string(),
            upstream_model: None,
            code_mode: None,
        },
    ];

    assert!(SharedUpstreamModelCatalog::validate_upstream_model_refs(&valid_catalog).is_ok());
    assert!(SharedUpstreamModelCatalog::validate_code_mode_tier_consistency(&valid_catalog).is_ok());

    // 1. Unresolved upstream model ref
    let bad_refs = vec![
        ModelCatalogRow {
            provider_id: "openrouter".to_string(),
            model_id: "nonexistent".to_string(),
            upstream_model: Some("missing/model".to_string()),
            code_mode: None,
        },
        // Self-referencing upstream model
        ModelCatalogRow {
            provider_id: "self".to_string(),
            model_id: "loop".to_string(),
            upstream_model: Some("self/loop".to_string()),
            code_mode: None,
        },
    ];
    let ref_errors = SharedUpstreamModelCatalog::validate_upstream_model_refs(&bad_refs).unwrap_err();
    assert_eq!(ref_errors.len(), 2);
    assert!(ref_errors[0].contains("openrouter/nonexistent -> missing/model"));
    assert!(ref_errors[1].contains("self/loop -> self/loop"));

    // 2. Silent sibling in shared model group missing code-mode tier
    let silent_sibling_catalog = vec![
        ModelCatalogRow {
            provider_id: "openai".to_string(),
            model_id: "gpt-4o".to_string(),
            upstream_model: None,
            code_mode: Some("capable".to_string()),
        },
        ModelCatalogRow {
            provider_id: "azure".to_string(),
            model_id: "gpt-4o".to_string(),
            upstream_model: Some("openai/gpt-4o".to_string()),
            code_mode: None, // Missing!
        },
    ];
    let tier_errors = SharedUpstreamModelCatalog::validate_code_mode_tier_consistency(&silent_sibling_catalog).unwrap_err();
    assert_eq!(tier_errors.len(), 1);
    assert!(tier_errors[0].contains("azure/gpt-4o"));
    assert!(tier_errors[0].contains("shared model gpt-4o"));
}

#[test]
fn test_ocrm_plugins_provider_catalog_deprecation_guard() {
    // Tests equivalent behavior from upstream src/plugins/contracts/provider-catalog-deprecation.contract.test.ts
    use mahayana_host_runtime::extensions::plugins::capability_catalog::ProviderCatalogDeprecationGuard;

    let clean_sources = [
        (
            "extensions/openai/src/index.ts",
            "api.registerProvider({ id: 'openai', label: 'OpenAI', auth: [] })",
        ),
        (
            "extensions/anthropic/src/index.ts",
            "const plugin: ProviderPlugin = { id: 'anthropic', label: 'Anthropic' }",
        ),
    ];
    assert!(ProviderCatalogDeprecationGuard::assert_no_deprecated_discovery_hooks(&clean_sources).is_ok());

    let deprecated_sources = [
        (
            "extensions/legacy/src/index.ts",
            "api.registerProvider({ id: 'legacy', discovery: async () => [] })",
        ),
    ];
    let err = ProviderCatalogDeprecationGuard::assert_no_deprecated_discovery_hooks(&deprecated_sources).unwrap_err();
    assert_eq!(err, vec!["extensions/legacy/src/index.ts"]);
}

#[test]
fn test_ocrm_plugins_auth_choice_contract() {
    // Tests equivalent behavior from upstream src/plugins/contracts/auth-choice.contract.test.ts
    use mahayana_host_runtime::extensions::plugins::capability_catalog::{
        PluginCapabilityCatalog, ProviderAuthChoice,
    };

    let catalog = PluginCapabilityCatalog::new();
    let choice_id = PluginCapabilityCatalog::build_provider_method_choice("demo-oauth-provider", "oauth");
    assert_eq!(choice_id, "demo-oauth-provider:oauth");

    catalog.register_auth_choice(ProviderAuthChoice {
        provider: "demo-oauth-provider".to_string(),
        method: "oauth".to_string(),
        choice_id: choice_id.clone(),
        choice_label: "OAuth".to_string(),
        onboarding_scopes: vec!["chat".to_string()],
        cli_flag: None,
        option_key: None,
    });

    // Resolves preferred provider for auth choice
    assert_eq!(
        catalog.resolve_preferred_provider_for_auth_choice(&choice_id),
        Some("demo-oauth-provider".to_string())
    );

    // Resolves None for unknown choice
    assert_eq!(
        catalog.resolve_preferred_provider_for_auth_choice("unknown"),
        None
    );
}

#[test]
fn test_ocrm_plugins_embedding_provider_registration() {
    // Tests equivalent behavior from upstream src/plugins/contracts/embedding-provider.contract.test.ts
    use mahayana_host_runtime::extensions::plugins::capability_catalog::{
        EmbeddingProviderRegistry, RegisteredEmbeddingProvider,
    };

    // Public SDK mutation methods are forbidden
    assert!(!EmbeddingProviderRegistry::is_public_sdk_mutation_allowed("registerEmbeddingProvider"));
    assert!(!EmbeddingProviderRegistry::is_public_sdk_mutation_allowed("listRegisteredEmbeddingProviders"));
    assert!(!EmbeddingProviderRegistry::is_public_sdk_mutation_allowed("clearEmbeddingProviders"));
    assert!(!EmbeddingProviderRegistry::is_public_sdk_mutation_allowed("restoreEmbeddingProviders"));
    assert!(!EmbeddingProviderRegistry::is_public_sdk_mutation_allowed("restoreRegisteredEmbeddingProviders"));
    assert!(EmbeddingProviderRegistry::is_public_sdk_mutation_allowed("getEmbeddingProvider"));

    let mut registry = EmbeddingProviderRegistry::new();

    // Rejects plugins that did not declare the capability contract
    let err_undeclared = registry.register("not-embedding", "forbidden", false).unwrap_err();
    assert_eq!(
        err_undeclared,
        "plugin must declare contracts.embeddingProviders for adapter: forbidden"
    );
    assert_eq!(registry.providers().len(), 0);

    // Allows plugins that declare the capability contract
    assert!(registry.register("embedding-owner", "embedding-owner", true).is_ok());
    assert_eq!(
        registry.providers(),
        &[RegisteredEmbeddingProvider {
            provider_id: "embedding-owner".to_string(),
            plugin_id: "embedding-owner".to_string(),
        }]
    );

    // Rejects duplicate provider ids
    let mut dup_registry = EmbeddingProviderRegistry::new();
    assert!(dup_registry.register("first-owner", "shared", true).is_ok());
    let err_dup = dup_registry.register("second-owner", "shared", true).unwrap_err();
    assert_eq!(
        err_dup,
        "embedding provider already registered: shared (owner: first-owner)"
    );

    // Keeps core embedding provider ids reserved
    let mut core_registry = EmbeddingProviderRegistry::new();
    let err_core = core_registry.register("core-shadow", "openai-compatible", true).unwrap_err();
    assert_eq!(
        err_core,
        "embedding provider already registered: openai-compatible (owner: core)"
    );
}

#[test]
fn test_ocrm_plugins_wizard_choice_resolution_and_model_picker() {
    // Tests equivalent behavior from upstream wizard.choice-resolution.contract.test.ts and wizard.model-picker.contract.test.ts
    use mahayana_host_runtime::extensions::plugins::capability_catalog::{
        ProviderWizardChoiceResolver, WizardAuthMethod, WizardProviderEntry,
    };

    let test_providers = vec![
        WizardProviderEntry {
            id: "alpha".to_string(),
            setup_wizard: Some("alpha-setup-wizard".to_string()),
            model_picker: false,
            auth: vec![
                WizardAuthMethod {
                    id: "api-key".to_string(),
                    wizard: Some("alpha-api-key-wizard".to_string()),
                    model_picker: false,
                },
                WizardAuthMethod {
                    id: "oauth".to_string(),
                    wizard: Some("alpha-oauth-wizard".to_string()),
                    model_picker: true,
                },
            ],
        },
        WizardProviderEntry {
            id: "beta".to_string(),
            setup_wizard: Some("beta-setup-wizard".to_string()),
            model_picker: true,
            auth: vec![WizardAuthMethod {
                id: "token".to_string(),
                wizard: None,
                model_picker: false,
            }],
        },
        WizardProviderEntry {
            id: "gamma".to_string(),
            setup_wizard: Some("gamma-setup-wizard".to_string()),
            model_picker: false,
            auth: vec![WizardAuthMethod {
                id: "alt".to_string(),
                wizard: None,
                model_picker: false,
            }],
        },
    ];

    // Choice resolution cases matching upstream it.each matrix
    let c1 = ProviderWizardChoiceResolver::resolve_choice(&test_providers, "provider-plugin:alpha:api-key").unwrap();
    assert_eq!(c1.provider_id, "alpha");
    assert_eq!(c1.method_id, "api-key");
    assert_eq!(c1.wizard_source, "method");
    assert_eq!(c1.wizard.as_deref(), Some("alpha-api-key-wizard"));

    let c2 = ProviderWizardChoiceResolver::resolve_choice(&test_providers, "alpha").unwrap();
    assert_eq!(c2.provider_id, "alpha");
    assert_eq!(c2.method_id, "api-key");
    assert_eq!(c2.wizard_source, "method");
    assert_eq!(c2.wizard.as_deref(), Some("alpha-api-key-wizard"));

    let c3 = ProviderWizardChoiceResolver::resolve_choice(&test_providers, "alpha-oauth").unwrap();
    assert_eq!(c3.provider_id, "alpha");
    assert_eq!(c3.method_id, "oauth");
    assert_eq!(c3.wizard_source, "method");
    assert_eq!(c3.wizard.as_deref(), Some("alpha-oauth-wizard"));

    let c4 = ProviderWizardChoiceResolver::resolve_choice(&test_providers, "beta").unwrap();
    assert_eq!(c4.provider_id, "beta");
    assert_eq!(c4.method_id, "token");
    assert_eq!(c4.wizard_source, "provider");
    assert_eq!(c4.wizard.as_deref(), Some("beta-setup-wizard"));

    let c5 = ProviderWizardChoiceResolver::resolve_choice(&test_providers, "gamma-alt").unwrap();
    assert_eq!(c5.provider_id, "gamma");
    assert_eq!(c5.method_id, "alt");
    assert_eq!(c5.wizard_source, "provider");
    assert_eq!(c5.wizard.as_deref(), Some("gamma-setup-wizard"));

    // Model picker entries
    let model_picker_entries = ProviderWizardChoiceResolver::resolve_model_picker_entries(&test_providers);
    assert_eq!(
        model_picker_entries,
        vec!["beta".to_string(), "provider-plugin:alpha:oauth".to_string()]
    );
    for choice in &model_picker_entries {
        assert!(ProviderWizardChoiceResolver::resolve_choice(&test_providers, choice).is_some());
    }
}

#[test]
fn test_ocrm_plugins_bundled_capability_metadata_and_subpath_bundle() {
    // Tests equivalent behavior from bundled-capability-metadata.ts, bundled-plugin-roots.ts,
    // and plugin-sdk-index.bundle.test.ts
    use mahayana_host_runtime::extensions::plugins::bundle_manifest::{
        BundledPluginRootsManager, BundledPluginSubpathValidator,
    };
    use mahayana_host_runtime::extensions::plugins::capability_catalog::BundledPluginContractSnapshot;

    // 1. Snapshot has capabilities
    let mut snapshot = BundledPluginContractSnapshot::default();
    assert!(!snapshot.has_capabilities());
    snapshot.speech_provider_ids.push("azure-speech".to_string());
    assert!(snapshot.has_capabilities());

    // 2. Bundled plugin roots resolution
    let roots = BundledPluginRootsManager::new(&[("matrix", "/app/extensions/matrix")]);
    assert_eq!(
        roots.resolve_file("matrix", "src/matrix/sdk.ts").unwrap(),
        "/app/extensions/matrix/src/matrix/sdk.ts"
    );
    assert!(roots.resolve_file("unknown", "sdk.ts").is_err());

    // 3. Subpath bundle validator never_bundle and bare matrix imports
    let never_bundle = BundledPluginSubpathValidator::never_bundle_dependencies();
    assert!(never_bundle.contains(&"@lancedb/lancedb"));
    assert!(never_bundle.contains(&"@matrix-org/matrix-sdk-crypto-nodejs"));
    assert!(never_bundle.contains(&"matrix-js-sdk"));

    let clean_sources = [("matrix/src/matrix/sdk.ts", "import { MatrixClient } from './client.js';")];
    assert!(BundledPluginSubpathValidator::assert_no_bare_matrix_sdk_imports(&clean_sources).is_ok());

    let bare_sources = [("matrix/src/bad.ts", "import * as sdk from 'matrix-js-sdk';")];
    let err_bare = BundledPluginSubpathValidator::assert_no_bare_matrix_sdk_imports(&bare_sources).unwrap_err();
    assert_eq!(err_bare, vec!["matrix/src/bad.ts"]);
}

#[test]
fn test_ocrm_plugins_host_hook_fixture_and_ui_projection() {
    // Tests equivalent behavior from host-hook-fixture.ts and test-helpers/host-hook-ui-projection.ts
    use mahayana_host_runtime::extensions::plugins::bundle_manifest::{
        HostHookFixturePlugin, HostHookUiProjection,
    };

    // 1. Fixture plugin doubles
    let fixture = HostHookFixturePlugin::register_fixture_defaults();
    assert_eq!(fixture.session_extensions, vec!["workflow"]);
    assert_eq!(fixture.tool_metadata_names, vec!["approval_fixture_tool"]);
    assert_eq!(fixture.control_ui_descriptors, vec!["workflow-card"]);
    assert_eq!(fixture.lifecycle_hooks, vec!["workflow-cleanup"]);
    assert_eq!(fixture.agent_event_subscriptions, vec!["workflow-events"]);
    assert_eq!(fixture.session_scheduler_jobs, vec!["workflow-nudge"]);
    assert_eq!(fixture.command_names, vec!["host-hook-fixture"]);

    let (cmd_text, cont) = fixture.execute_command(Some("test-arg"));
    assert_eq!(cmd_text, "fixture:test-arg");
    assert!(cont);

    assert_eq!(HostHookFixturePlugin::turn_prepare_context(), "fixture turn context");
    assert_eq!(HostHookFixturePlugin::heartbeat_prompt_contribution(), "fixture heartbeat context");

    // 2. UI Projection
    let projection = HostHookUiProjection::build(42);
    assert!(projection.ok);
    assert_eq!(projection.generation, 42);
    assert!(projection.methods.contains(&"plugins.uiDescriptors".to_string()));
    assert!(projection.methods.contains(&"plugins.sessionAction".to_string()));
    assert_eq!(projection.control_ui_widget_kinds.len(), 3);
    assert_eq!(projection.descriptors.len(), 2);

    let admin_desc = projection.descriptors.iter().find(|d| d.id == "admin-panel").unwrap();
    assert_eq!(admin_desc.surface, "settings");
    assert_eq!(admin_desc.required_scopes, vec!["operator.admin"]);

    let approval_desc = projection.descriptors.iter().find(|d| d.id == "approval-panel").unwrap();
    assert_eq!(approval_desc.surface, "session");
    assert!(approval_desc.required_scopes.is_empty());
}

#[test]
fn test_ocrm_plugins_bundle_manifest_guardrails() {
    // Tests equivalent behavior from config-footprint-guardrails, extension-package-project-boundaries,
    // extension-runtime-dependencies, and plugin-entry-guardrails contract test suites.
    use mahayana_host_runtime::extensions::plugins::bundle_manifest::{
        ConfigFootprintGuardrail, ExtensionPackageBoundaryGuard,
        ExtensionRuntimeDependenciesGuard, PluginEntryGuardrails,
    };

    // 1. Config footprint guardrail
    let clean_paths = ["plugins.entries.*.config", "channels.telegram.botToken"];
    assert!(ConfigFootprintGuardrail::assert_no_retired_legacy_paths(&clean_paths).is_ok());

    let retired_paths = ["talk.voiceId", "channels.telegram.streamMode"];
    let err_retired = ConfigFootprintGuardrail::assert_no_retired_legacy_paths(&retired_paths).unwrap_err();
    assert_eq!(err_retired, vec!["channels.telegram.streamMode", "talk.voiceId"]);


    let clean_network_paths = ["network.dangerouslyAllowPrivateNetwork"];
    assert!(ConfigFootprintGuardrail::assert_canonical_private_network_paths(&clean_network_paths).is_ok());

    let bad_network_paths = ["channels.matrix.allowPrivateNetwork"];
    let err_network = ConfigFootprintGuardrail::assert_canonical_private_network_paths(&bad_network_paths).unwrap_err();
    assert_eq!(err_network, vec!["channels.matrix.allowPrivateNetwork"]);

    // 2. Extension package boundary guard
    let clean_extension_src = "import { createPlugin } from 'openclaw/plugin-sdk/core';";
    assert!(ExtensionPackageBoundaryGuard::assert_allowed_cross_package_imports(
        "extensions/telegram/src/index.ts",
        clean_extension_src
    ).is_ok());

    let bad_extension_src = "import { secret } from 'openclaw/plugin-sdk/private/secrets';";
    let err_boundary = ExtensionPackageBoundaryGuard::assert_allowed_cross_package_imports(
        "extensions/telegram/src/index.ts",
        bad_extension_src
    ).unwrap_err();
    assert!(err_boundary[0].contains("imports forbidden private entrypoint"));

    // 3. Extension runtime dependencies whitelist
    assert!(ExtensionRuntimeDependenciesGuard::is_whitelisted_optional_peer("extensions/canvas", "@a2ui/markdown-it"));
    assert!(ExtensionRuntimeDependenciesGuard::is_whitelisted_optional_peer("extensions/discord", "@discordjs/opus"));
    assert!(ExtensionRuntimeDependenciesGuard::is_whitelisted_optional_peer("extensions/qa-lab", "crabline"));
    assert!(ExtensionRuntimeDependenciesGuard::is_whitelisted_optional_peer("extensions/browser", "zod"));
    assert!(ExtensionRuntimeDependenciesGuard::is_whitelisted_optional_peer("extensions/whatsapp", "audio-decode"));
    assert!(ExtensionRuntimeDependenciesGuard::is_whitelisted_optional_peer("extensions/memory-lancedb", "apache-arrow"));
    assert!(!ExtensionRuntimeDependenciesGuard::is_whitelisted_optional_peer("extensions/telegram", "random-dep"));

    // 4. Plugin entry guardrails (no test modules in entrypoints)
    let clean_entry = "export function register(api) { api.registerTool(); }";
    assert!(PluginEntryGuardrails::assert_no_testing_imports_in_entrypoint("index.ts", clean_entry).is_ok());

    let bad_entry = "import { describe, it } from 'vitest';\nimport { fixture } from './test-harness';";
    let err_entry = PluginEntryGuardrails::assert_no_testing_imports_in_entrypoint("index.ts", bad_entry).unwrap_err();
    assert_eq!(err_entry.len(), 2);
}

#[test]
fn test_ocrm_plugins_sdk_package_and_runtime_api_guardrails() {
    // Tests equivalent behavior from plugin-sdk-package-contract-guardrails,
    // plugin-sdk-runtime-api-guardrails, and plugin-sdk-subpaths contract test suites.
    use mahayana_host_runtime::extensions::plugins::bundle_manifest::{
        PluginSdkPackageContractGuard, PluginSdkRuntimeApiGuardrails,
    };

    // 1. Public plugin SDK entrypoints
    let public_entrypoints = PluginSdkPackageContractGuard::public_plugin_sdk_entrypoints();
    assert!(public_entrypoints.contains(&"channel-contract"));
    assert!(public_entrypoints.contains(&"core"));
    assert!(public_entrypoints.contains(&"media-runtime"));
    assert!(public_entrypoints.contains(&"plugin-entry"));

    // 2. Private local-only entrypoint boundary
    let clean_subpaths = ["openclaw/plugin-sdk/core", "openclaw/plugin-sdk/channel-contract"];
    assert!(PluginSdkPackageContractGuard::assert_no_private_entrypoint_in_public_subpaths(&clean_subpaths).is_ok());

    let leaky_subpaths = ["openclaw/plugin-sdk/browser-maintenance"];
    let err_leaky = PluginSdkPackageContractGuard::assert_no_private_entrypoint_in_public_subpaths(&leaky_subpaths).unwrap_err();
    assert_eq!(err_leaky, vec!["openclaw/plugin-sdk/browser-maintenance"]);

    // 3. Runtime API export guardrails
    let clean_exports = ["export { createFaceTimeRuntime } from './src/runtime.js';"];
    assert!(PluginSdkRuntimeApiGuardrails::assert_runtime_api_exports_allowed("facetime", &clean_exports).is_ok());

    let bad_exports = ["export { testHelper } from './test-api.js';"];
    let err_exports = PluginSdkRuntimeApiGuardrails::assert_runtime_api_exports_allowed("facetime", &bad_exports).unwrap_err();
    assert!(err_exports[0].contains("exports internal test module"));
}

#[test]
fn test_ocrm_plugins_activation_context_payload_and_runtime_registry() {
    use std::collections::HashSet;
    use serde_json::json;
    use mahayana_host_runtime::extensions::plugins::activation_planner::{
        ActivationPlanner, ActivationTrigger, PluginManifestRecord,
        PluginActivationSourceResolver, ActivePayloadVerification,
        PluginInstallRecordStub, ActiveRegistryIdentity, ActiveRuntimeRegistryResolver,
    };

    // 1. Activation planner with scoped plugin IDs
    let mut planner = ActivationPlanner::new();
    planner.register_manifest(PluginManifestRecord {
        plugin_id: "telegram".into(),
        commands: vec!["tg".into()],
        providers: vec![],
        channels: vec!["telegram".into()],
        capabilities: vec![],
        is_bundled: true,
        is_trusted: true,
    });
    planner.register_manifest(PluginManifestRecord {
        plugin_id: "discord".into(),
        commands: vec!["dc".into()],
        providers: vec![],
        channels: vec!["discord".into()],
        capabilities: vec![],
        is_bundled: true,
        is_trusted: true,
    });

    let triggers = vec![
        ActivationTrigger::Channel("telegram".into()),
        ActivationTrigger::Channel("discord".into()),
    ];

    // Explicit empty scope returns empty
    let empty_scope: Vec<String> = vec![];
    assert_eq!(planner.plan_activation_scoped(&triggers, Some(&empty_scope)), Vec::<String>::new());

    // Restricted scope returns only permitted
    let restricted_scope = vec!["telegram".to_string()];
    assert_eq!(planner.plan_activation_scoped(&triggers, Some(&restricted_scope)), vec!["telegram"]);

    // Unrestricted returns both
    assert_eq!(planner.plan_activation_scoped(&triggers, None), vec!["discord", "telegram"]);

    // 2. Activation source config resolution precedence
    let root_cfg = json!({"root": true});
    let captured_cfg = json!({"captured": true});
    let explicit_source = json!({"explicit": true});

    // Explicit source takes highest precedence
    assert_eq!(
        PluginActivationSourceResolver::resolve_source_config(Some(&root_cfg), Some(&explicit_source), Some(&captured_cfg)),
        explicit_source
    );
    // Captured takes next precedence
    assert_eq!(
        PluginActivationSourceResolver::resolve_source_config(Some(&root_cfg), None, Some(&captured_cfg)),
        captured_cfg
    );
    // Fallback to root
    assert_eq!(
        PluginActivationSourceResolver::resolve_source_config(Some(&root_cfg), None, None),
        root_cfg
    );
    // Fallback to empty object
    assert_eq!(
        PluginActivationSourceResolver::resolve_source_config(None, None, None),
        json!({})
    );

    // 3. Active payload verification filtering
    let records = vec![
        PluginInstallRecordStub {
            plugin_id: "denied-plug".into(),
            version: "1.0.0".into(),
            is_operator_managed: false,
            official_npm_spec: None,
            official_clawhub_spec: None,
        },
        PluginInstallRecordStub {
            plugin_id: "operator-plug".into(),
            version: "1.0.0".into(),
            is_operator_managed: true,
            official_npm_spec: None,
            official_clawhub_spec: None,
        },
        PluginInstallRecordStub {
            plugin_id: "disabled-regular".into(),
            version: "1.0.0".into(),
            is_operator_managed: false,
            official_npm_spec: None,
            official_clawhub_spec: None,
        },
        PluginInstallRecordStub {
            plugin_id: "disabled-official".into(),
            version: "1.0.0".into(),
            is_operator_managed: false,
            official_npm_spec: Some("@openclaw/voice@1.0.0".into()),
            official_clawhub_spec: None,
        },
        PluginInstallRecordStub {
            plugin_id: "enabled-plug".into(),
            version: "1.0.0".into(),
            is_operator_managed: false,
            official_npm_spec: None,
            official_clawhub_spec: None,
        },
    ];

    let mut denied_ids = HashSet::new();
    denied_ids.insert("denied-plug".into());

    let mut disabled_ids = HashSet::new();
    disabled_ids.insert("disabled-regular".into());
    disabled_ids.insert("disabled-official".into());

    let source_bundled_ids = HashSet::new();

    let active_records = ActivePayloadVerification::filter_records_to_active(
        &records,
        &disabled_ids,
        &denied_ids,
        &source_bundled_ids,
    );

    let active_ids: Vec<String> = active_records.into_iter().map(|r| r.plugin_id).collect();
    // Denied dropped; operator-managed dropped; disabled-regular dropped;
    // disabled-official preserved; enabled-plug preserved.
    assert_eq!(active_ids, vec!["disabled-official", "enabled-plug"]);

    // 4. Active runtime registry compatibility
    let active_id = ActiveRegistryIdentity {
        cache_key: "cache-key-123".into(),
        workspace_dir: Some("/workspace/alpha".into()),
        loaded_plugin_ids: vec!["telegram".into(), "discord".into(), "telegram".into()],
    };

    // Workspace mismatch rejected
    assert!(!ActiveRuntimeRegistryResolver::is_compatible(
        Some(&active_id),
        "cache-key-123",
        Some("/workspace/beta"),
    ));
    assert!(!ActiveRuntimeRegistryResolver::is_compatible(
        Some(&active_id),
        "cache-key-123",
        None,
    ));

    // Cache key mismatch rejected
    assert!(!ActiveRuntimeRegistryResolver::is_compatible(
        Some(&active_id),
        "cache-key-456",
        Some("/workspace/alpha"),
    ));

    // Exact match accepted
    assert!(ActiveRuntimeRegistryResolver::is_compatible(
        Some(&active_id),
        "cache-key-123",
        Some("/workspace/alpha"),
    ));

    // Sorted and deduplicated runtime plugin IDs
    assert_eq!(
        ActiveRuntimeRegistryResolver::list_runtime_plugin_ids(&active_id),
        vec!["discord", "telegram"]
    );
}

#[test]
fn test_ocrm_plugins_bundled_mcp_lsp_capabilities_and_command_scopes() {
    use std::collections::HashMap;
    use serde_json::json;
    use mahayana_host_runtime::extensions::plugins::bundle_manifest::{
        BundledMcpServerConfig, BundledMcpTransport, BundledMcpServerValidator,
        BundledLspServerValidator, BundleCapabilitySupport, BundleCommandScopeValidator,
    };

    // 1. Bundled MCP Server Validation
    let valid_mcp = BundledMcpServerConfig {
        name: "local-sqlite".into(),
        command: "bin/mcp-server".into(),
        args: vec!["--db".into(), "data/store.db".into()],
        env: HashMap::new(),
        transport: BundledMcpTransport::Stdio,
    };
    assert!(BundledMcpServerValidator::validate(&valid_mcp).is_ok());

    let bad_name = BundledMcpServerConfig {
        name: "local/sqlite".into(),
        ..valid_mcp.clone()
    };
    let err_name = BundledMcpServerValidator::validate(&bad_name).unwrap_err();
    assert!(err_name.contains("Invalid MCP server name"));

    let bad_cmd = BundledMcpServerConfig {
        command: "../../../bin/sh".into(),
        ..valid_mcp.clone()
    };
    let err_cmd = BundledMcpServerValidator::validate(&bad_cmd).unwrap_err();
    assert!(err_cmd.contains("cannot escape bundle root"));

    let bad_arg = BundledMcpServerConfig {
        args: vec!["--target".into(), "../../etc/passwd".into()],
        ..valid_mcp.clone()
    };
    let err_arg = BundledMcpServerValidator::validate(&bad_arg).unwrap_err();
    assert!(err_arg.contains("cannot escape bundle root"));

    // 2. Bundled LSP Server Validation
    assert!(BundledLspServerValidator::validate("rust-analyzer", "rust", "bin/ra").is_ok());
    assert!(BundledLspServerValidator::validate("", "rust", "bin/ra").is_err());
    assert!(BundledLspServerValidator::validate("ra", "", "bin/ra").is_err());
    assert!(BundledLspServerValidator::validate("ra", "rust", "../ra").is_err());

    // 3. Bundle Capability Support from Manifest
    let manifest = json!({
        "mcpServers": {
            "sqlite": { "command": "bin/sqlite" }
        },
        "tools": [
            { "name": "search" }
        ],
        "skills": [
            { "name": "web-browsing" }
        ]
    });
    let caps = BundleCapabilitySupport::from_manifest(&manifest);
    assert!(caps.has_mcp);
    assert!(caps.has_tools);
    assert!(caps.has_skills);
    assert!(!caps.has_lsp);
    assert!(!caps.has_channels);

    // 4. Bundle Command Scope Validation
    let user_scopes = ["operator.read", "tool.execute"];
    assert!(BundleCommandScopeValidator::assert_command_scope_authorized("run-tool", Some("tool.execute"), &user_scopes).is_ok());
    assert!(BundleCommandScopeValidator::assert_command_scope_authorized("read-info", None, &user_scopes).is_ok());

    // Unauthorized scope fails
    let err_scope = BundleCommandScopeValidator::assert_command_scope_authorized("delete-db", Some("operator.admin"), &user_scopes).unwrap_err();
    assert!(err_scope.contains("requires scope 'operator.admin'"));

    // Admin scope bypasses requirement
    let admin_scopes = ["operator.admin"];
    assert!(BundleCommandScopeValidator::assert_command_scope_authorized("delete-db", Some("restricted.write"), &admin_scopes).is_ok());
}

#[test]
fn test_ocrm_plugins_bundled_channel_metadata_compat_and_dir_resolver() {
    use mahayana_host_runtime::extensions::plugins::capability_catalog::{
        BundledChannelMetadata, BundledChannelMetadataValidator,
        BundledCompatResolver, BundledDirResolver,
    };

    // 1. Channel Metadata Validation
    let valid_channel = BundledChannelMetadata {
        channel_id: "telegram".into(),
        stream_mode: Some("block".into()),
        reply_to_mode: Some("all".into()),
        history_limit: Some(500),
        media_max_mb: Some(50),
        provider_skills: vec!["voice".into()],
    };
    assert!(BundledChannelMetadataValidator::validate(&valid_channel).is_ok());

    let empty_id = BundledChannelMetadata {
        channel_id: "".into(),
        ..valid_channel.clone()
    };
    assert!(BundledChannelMetadataValidator::validate(&empty_id).is_err());

    let bad_history = BundledChannelMetadata {
        history_limit: Some(200_000),
        ..valid_channel.clone()
    };
    assert!(BundledChannelMetadataValidator::validate(&bad_history).is_err());

    let bad_media = BundledChannelMetadata {
        media_max_mb: Some(5000),
        ..valid_channel.clone()
    };
    assert!(BundledChannelMetadataValidator::validate(&bad_media).is_err());

    // 2. Bundled Compat ID Normalization
    assert_eq!(BundledCompatResolver::normalize_plugin_id("legacy-telegram"), "telegram");
    assert_eq!(BundledCompatResolver::normalize_plugin_id("legacy-discord"), "discord");
    assert_eq!(BundledCompatResolver::normalize_plugin_id("legacy-slack"), "slack");
    assert_eq!(BundledCompatResolver::normalize_plugin_id("custom-plugin"), "custom-plugin");

    // 3. Bundled Directory Path Traversal Guard
    assert_eq!(
        BundledDirResolver::resolve_relative_path("/app/bundled/plugins", "my-plugin/index.js").unwrap(),
        "/app/bundled/plugins/my-plugin/index.js"
    );

    let err_traversal = BundledDirResolver::resolve_relative_path("/app/bundled/plugins", "../../../etc/passwd").unwrap_err();
    assert!(err_traversal.contains("Path traversal detected"));
}

#[test]
fn test_ocrm_plugins_bundled_metadata_scan_compat_and_naming_artwork() {
    use std::collections::{HashMap, HashSet};

    // 1. Bundled Plugin Scanner - entry path rewrite
    assert_eq!(
        BundledPluginScanner::rewrite_bundled_plugin_entry_to_built_path("./index.ts"),
        "index.js"
    );
    assert_eq!(
        BundledPluginScanner::rewrite_bundled_plugin_entry_to_built_path(".\\src\\runner.mts"),
        "src\\runner.js"
    );
    assert_eq!(
        BundledPluginScanner::rewrite_bundled_plugin_entry_to_built_path("dist/main.js"),
        "dist/main.js"
    );

    // 2. Bundled Plugin Scanner - id hint derivation
    assert_eq!(
        BundledPluginScanner::derive_bundled_plugin_id_hint(
            "./index.ts",
            "telegram",
            Some("@openclaw/telegram"),
            false
        ),
        "telegram"
    );
    assert_eq!(
        BundledPluginScanner::derive_bundled_plugin_id_hint(
            "./handlers/worker.ts",
            "telegram",
            Some("@openclaw/telegram-worker"),
            true
        ),
        "telegram-worker/worker"
    );

    // 3. Top-level public surface source validation
    assert!(BundledPluginScanner::is_top_level_public_surface_source("runtime-api.ts"));
    assert!(BundledPluginScanner::is_top_level_public_surface_source("helper-api.js"));
    assert!(!BundledPluginScanner::is_top_level_public_surface_source(".hidden.ts"));
    assert!(!BundledPluginScanner::is_top_level_public_surface_source("test-utils.ts"));
    assert!(!BundledPluginScanner::is_top_level_public_surface_source("types.d.ts"));
    assert!(!BundledPluginScanner::is_top_level_public_surface_source("config-api.ts"));
    assert!(!BundledPluginScanner::is_top_level_public_surface_source("runner.test.ts"));

    // 4. Runtime sidecar collection
    let public_surfaces = vec![
        "runtime-api.js".to_string(),
        "custom-helper.js".to_string(),
        "light-runtime-api.js".to_string(),
    ];
    let sidecars = BundledPluginScanner::collect_bundled_plugin_runtime_sidecar_artifacts(&public_surfaces);
    assert_eq!(sidecars, vec!["runtime-api.js", "light-runtime-api.js"]);

    // 5. Startup manifest fast-path resolution
    assert!(BundledPluginScanner::has_bundled_plugin_startup_manifest(
        "alpha",
        Some(r#"{"id":"alpha","name":"Alpha Plugin"}"#)
    ));
    assert!(!BundledPluginScanner::has_bundled_plugin_startup_manifest(
        "alpha",
        Some(r#"{"id":"beta","name":"Beta Plugin"}"#)
    ));
    assert!(!BundledPluginScanner::has_bundled_plugin_startup_manifest("alpha", None));

    // 6. Bundled Provider Compat validation
    assert!(BundledProviderCompatValidator::is_bundled_provider_compat_contract("speechProviders"));
    assert!(BundledProviderCompatValidator::is_bundled_provider_compat_contract("migrationProviders"));
    assert!(!BundledProviderCompatValidator::is_bundled_provider_compat_contract("unknownContract"));

    let mut counts = HashMap::new();
    counts.insert("speechProviders".to_string(), 1);
    assert!(BundledProviderCompatValidator::is_bundled_provider_compat_plugin("bundled", 0, &counts));
    assert!(BundledProviderCompatValidator::is_bundled_provider_compat_plugin("bundled", 2, &HashMap::new()));
    assert!(!BundledProviderCompatValidator::is_bundled_provider_compat_plugin("installed", 2, &counts));

    // 7. Bundled Source Overlays
    let mount_info_sample = "36 35 98:0 / /mnt/overlay rw,relatime - ext4 /dev/root rw\n\
                             37 36 98:1 / /home/user/extensions rw - ext4 /dev/sda1 rw";
    let mount_points = BundledSourceOverlayResolver::parse_linux_mount_info_mount_points(mount_info_sample);
    assert!(mount_points.contains("/mnt/overlay"));
    assert!(mount_points.contains("/home/user/extensions"));
    assert!(!mount_points.contains("/var/log"));

    assert!(BundledSourceOverlayResolver::source_overlays_disabled(Some("1")));
    assert!(BundledSourceOverlayResolver::source_overlays_disabled(Some("TRUE")));
    assert!(!BundledSourceOverlayResolver::source_overlays_disabled(Some("0")));
    assert!(!BundledSourceOverlayResolver::source_overlays_disabled(None));

    let peers = vec!["discord".to_string(), "telegram".to_string()];
    let mut custom_mounts = HashSet::new();
    custom_mounts.insert("/legacy/extensions/discord".to_string());
    let overlays = BundledSourceOverlayResolver::list_bundled_source_overlay_dirs(
        "/app/extensions",
        "/legacy/extensions",
        &peers,
        &custom_mounts,
        false
    );
    assert_eq!(overlays, vec!["/legacy/extensions/discord"]);

    // 8. Bundled Plugin Naming & Artwork Validator
    let cats = vec!["channels".to_string()];
    assert_eq!(
        BundledPluginNamingAndArtworkValidator::validate_bundled_plugin_categories(Some(&cats)).unwrap(),
        "channels"
    );
    assert!(BundledPluginNamingAndArtworkValidator::validate_bundled_plugin_categories(None).is_err());
    let invalid_cats = vec!["channels".to_string(), "tools".to_string()];
    assert!(BundledPluginNamingAndArtworkValidator::validate_bundled_plugin_categories(Some(&invalid_cats)).is_err());

    // Valid 512x512 PNG header (24 bytes minimum)
    let mut valid_png = vec![137, 80, 78, 71, 13, 10, 26, 10]; // signature
    valid_png.extend_from_slice(&[0, 0, 0, 13]); // IHDR chunk length
    valid_png.extend_from_slice(b"IHDR");
    valid_png.extend_from_slice(&512u32.to_be_bytes()); // width
    valid_png.extend_from_slice(&512u32.to_be_bytes()); // height
    assert!(BundledPluginNamingAndArtworkValidator::validate_bundled_plugin_icon_png(&valid_png).is_ok());

    let mut invalid_dim_png = valid_png.clone();
    invalid_dim_png[16..20].copy_from_slice(&256u32.to_be_bytes());
    assert!(BundledPluginNamingAndArtworkValidator::validate_bundled_plugin_icon_png(&invalid_dim_png).is_err());

    // Naming rules (including kimi-coding -> kimi exception)
    assert!(BundledPluginNamingAndArtworkValidator::validate_bundled_plugin_naming(
        "telegram",
        "telegram",
        "@openclaw/telegram"
    ).is_ok());
    assert!(BundledPluginNamingAndArtworkValidator::validate_bundled_plugin_naming(
        "kimi-coding",
        "kimi",
        "@openclaw/kimi-provider"
    ).is_ok());
    assert!(BundledPluginNamingAndArtworkValidator::validate_bundled_plugin_naming(
        "slack",
        "telegram",
        "@openclaw/slack"
    ).is_err());
    assert!(BundledPluginNamingAndArtworkValidator::validate_bundled_plugin_naming(
        "slack",
        "slack",
        "third-party/slack"
    ).is_err());
}

#[test]
fn test_ocrm_plugins_bundled_sources_catalog_and_capability_consent() {
    use std::collections::HashMap;

    // 1. Bundled Plugin Source Resolver & Config Requirements
    let empty_schema = serde_json::json!({});
    assert!(!BundledPluginSourceResolver::schema_has_required_fields(&empty_schema));

    let no_req_schema = serde_json::json!({
        "type": "object",
        "properties": { "name": { "type": "string" } }
    });
    assert!(!BundledPluginSourceResolver::schema_has_required_fields(&no_req_schema));

    let req_schema = serde_json::json!({
        "type": "object",
        "required": ["embedding"]
    });
    assert!(BundledPluginSourceResolver::schema_has_required_fields(&req_schema));

    let mut sources_map = HashMap::new();
    sources_map.insert("feishu".to_string(), BundledPluginSource {
        plugin_id: "feishu".to_string(),
        local_path: "/app/plugins/feishu".to_string(),
        npm_spec: Some("@openclaw/feishu".to_string()),
        version: Some("1.0.0".to_string()),
        config_schema: None,
        requires_config: false,
    });
    sources_map.insert("memory-lancedb".to_string(), BundledPluginSource {
        plugin_id: "memory-lancedb".to_string(),
        local_path: "/app/plugins/memory-lancedb".to_string(),
        npm_spec: Some("@openclaw/memory-lancedb".to_string()),
        version: Some("2.0.0".to_string()),
        config_schema: Some(req_schema),
        requires_config: true,
    });

    // Lookup tests
    let found_by_id = BundledPluginSourceResolver::find_in_map(
        &sources_map,
        BundledPluginLookup::PluginId("feishu"),
    );
    assert!(found_by_id.is_some());
    assert_eq!(found_by_id.unwrap().plugin_id, "feishu");

    let not_found_by_id = BundledPluginSourceResolver::find_in_map(
        &sources_map,
        BundledPluginLookup::PluginId("missing"),
    );
    assert!(not_found_by_id.is_none());

    let found_by_spec = BundledPluginSourceResolver::find_in_map(
        &sources_map,
        BundledPluginLookup::NpmSpec("@openclaw/memory-lancedb"),
    );
    assert!(found_by_spec.is_some());
    assert_eq!(found_by_spec.unwrap().plugin_id, "memory-lancedb");

    let found_by_path = BundledPluginSourceResolver::find_in_map(
        &sources_map,
        BundledPluginLookup::LocalPath("/app/plugins/feishu"),
    );
    assert!(found_by_path.is_some());
    assert_eq!(found_by_path.unwrap().plugin_id, "feishu");

    // Command hint
    let hint = BundledPluginSourceResolver::resolve_install_command_hint(found_by_id);
    assert_eq!(hint, Some("openclaw plugins install /app/plugins/feishu".to_string()));
    assert_eq!(BundledPluginSourceResolver::resolve_install_command_hint(None), None);

    // 2. Candidate Install Owner & Immutable Identity
    let identity = CandidateInstallOwnerResolver::resolve_immutable_install_identity(
        "global",
        Some("owner-pkg"),
        Some("npm"),
        Some("/var/plugins/owner-pkg"),
        Some("@openclaw/owner@1.0"),
        Some("@openclaw/owner@1.0.0"),
        Some("sha512-testintegrity"),
        Some("2026-10-27T00:00:00Z"),
    );
    assert!(identity.is_some());
    let id_str = identity.unwrap();
    assert!(id_str.contains("owner-pkg"));
    assert!(id_str.contains("sha512-testintegrity"));

    // Origin not global or source not npm -> None
    assert!(CandidateInstallOwnerResolver::resolve_immutable_install_identity(
        "bundled",
        Some("owner-pkg"),
        Some("npm"),
        Some("/var/plugins/owner-pkg"),
        None,
        None,
        Some("sha512-testintegrity"),
        None,
    ).is_none());

    assert!(CandidateInstallOwnerResolver::resolve_immutable_install_identity(
        "global",
        Some("owner-pkg"),
        Some("git"),
        Some("/var/plugins/owner-pkg"),
        None,
        None,
        Some("sha512-testintegrity"),
        None,
    ).is_none());

    // 3. Capability Catalog Families & Shape Validation
    assert_eq!(CapabilityCatalogFamily::ALL.len(), 3);
    assert_eq!(CapabilityCatalogFamily::SpeechProviders.required_method(), "synthesize");
    assert_eq!(CapabilityCatalogFamily::RealtimeTranscriptionProviders.required_method(), "createSession");
    assert_eq!(CapabilityCatalogFamily::RealtimeVoiceProviders.required_method(), "createBridge");

    let valid_catalog = serde_json::json!({
        "speechProviders": [
            {
                "id": "mock-tts",
                "label": "Mock TTS",
                "isConfigured": true,
                "synthesize": true
            }
        ],
        "realtimeTranscriptionProviders": [
            {
                "id": "mock-stt",
                "label": "Mock STT",
                "isConfigured": true,
                "createSession": true
            }
        ]
    });
    assert!(CapabilityCatalogValidator::validate_catalog_shape(&valid_catalog).is_ok());

    let invalid_family_catalog = serde_json::json!({
        "unknownFamily": []
    });
    assert!(CapabilityCatalogValidator::validate_catalog_shape(&invalid_family_catalog).is_err());

    let missing_method_catalog = serde_json::json!({
        "speechProviders": [
            {
                "id": "mock-tts",
                "label": "Mock TTS",
                "isConfigured": true
                // missing "synthesize"
            }
        ]
    });
    assert!(CapabilityCatalogValidator::validate_catalog_shape(&missing_method_catalog).is_err());

    // 4. Capability Consent Manager: Hashes, Integrity & Widening
    let surface_a = PluginAcceptedDeclaredSurface {
        tools: vec!["read".to_string()],
        channels: vec!["chat".to_string()],
        ..Default::default()
    };
    let hash_a = CapabilityConsentManager::compute_declared_surface_hash(&surface_a);
    assert_eq!(hash_a.len(), 64);

    let surface_b = PluginAcceptedDeclaredSurface {
        tools: vec!["read".to_string(), "write".to_string()],
        channels: vec!["chat".to_string()],
        ..Default::default()
    };
    let hash_b = CapabilityConsentManager::compute_declared_surface_hash(&surface_b);
    assert_ne!(hash_a, hash_b);

    // Widening diff
    let (widened, has_widening) = CapabilityConsentManager::diff_declared_surface_widening(&surface_a, &surface_b);
    assert!(has_widening);
    assert_eq!(widened.get("tools"), Some(&vec!["write".to_string()]));

    let (_widened_rev, has_widening_rev) = CapabilityConsentManager::diff_declared_surface_widening(&surface_b, &surface_a);
    assert!(!has_widening_rev);

    // Integrity precedence
    assert_eq!(
        CapabilityConsentManager::resolve_plugin_install_record_integrity(
            Some("sha512-primary"),
            Some("sha512-npm"),
            Some("clawpack-hash"),
            Some("gitcommit123"),
        ),
        Some(("sha512-primary".to_string(), "ssri"))
    );
    assert_eq!(
        CapabilityConsentManager::resolve_plugin_install_record_integrity(
            None,
            Some("sha512-npm"),
            Some("clawpack-hash"),
            None,
        ),
        Some(("sha512-npm".to_string(), "ssri"))
    );
    assert_eq!(
        CapabilityConsentManager::resolve_plugin_install_record_integrity(
            None,
            None,
            Some("clawpack-hash"),
            None,
        ),
        Some(("clawpack-hash".to_string(), "sha256"))
    );
    assert_eq!(
        CapabilityConsentManager::resolve_plugin_install_record_integrity(
            None,
            None,
            None,
            Some("gitcommit123"),
        ),
        Some(("gitcommit123".to_string(), "git-commit"))
    );

    // Surface currency checks
    assert!(CapabilityConsentManager::resolve_accepted_surface_current(
        Some(&surface_a),
        Some(&hash_a),
        Some("sha512-sig"),
        Some("sha512-sig"),
        &surface_a,
    ));
    // Forged hash
    assert!(!CapabilityConsentManager::resolve_accepted_surface_current(
        Some(&surface_a),
        Some("0000000000000000000000000000000000000000000000000000000000000000"),
        Some("sha512-sig"),
        Some("sha512-sig"),
        &surface_a,
    ));
    // Changed surface
    assert!(!CapabilityConsentManager::resolve_accepted_surface_current(
        Some(&surface_a),
        Some(&hash_a),
        Some("sha512-sig"),
        Some("sha512-sig"),
        &surface_b,
    ));
    // Changed integrity
    assert!(!CapabilityConsentManager::resolve_accepted_surface_current(
        Some(&surface_a),
        Some(&hash_a),
        Some("sha512-new"),
        Some("sha512-old"),
        &surface_a,
    ));

    // Redaction
    let spec = "git:https://fixture-user:fixture-password@example.invalid/plugins/demo.git?token=fixture-token";
    let redacted = CapabilityConsentManager::redact_source_spec(spec);
    assert!(!redacted.contains("fixture-user"));
    assert!(!redacted.contains("fixture-password"));
    assert!(!redacted.contains("fixture-token"));
    assert!(redacted.contains("***"));
    assert!(redacted.starts_with("git:https://"));
}

#[test]
fn test_ocrm_plugins_capability_lease_summary_and_acquisition() {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;

    // 1. Plugin Runtime Capability Lease
    let lease = PluginRuntimeCapabilityLease::new("test-owner");
    assert!(lease.is_active());
    assert!(lease.assert_active("tts").is_ok());

    let cleanup_called = Arc::new(AtomicBool::new(false));
    let cleanup_clone = Arc::clone(&cleanup_called);
    assert!(lease.retain(move || {
        cleanup_clone.store(true, Ordering::SeqCst);
    }).is_ok());

    assert!(!cleanup_called.load(Ordering::SeqCst));
    lease.revoke();
    assert!(!lease.is_active());
    assert!(cleanup_called.load(Ordering::SeqCst));

    // After revoke, assert_active fails
    let err = lease.assert_active("tts");
    assert!(err.is_err());
    assert!(err.unwrap_err().contains("test-owner tts is no longer active"));

    // Retain on revoked lease immediately calls cleanup and returns Err
    let immediate_cleanup = Arc::new(AtomicBool::new(false));
    let imm_clone = Arc::clone(&immediate_cleanup);
    let retain_err = lease.retain(move || {
        imm_clone.store(true, Ordering::SeqCst);
    });
    assert!(retain_err.is_err());
    assert!(immediate_cleanup.load(Ordering::SeqCst));

    // 2. Capability Summary Builder: Surface Merge & Operator Grants
    let s1 = PluginAcceptedDeclaredSurface {
        tools: vec!["tool_a".to_string()],
        channels: vec!["chat".to_string()],
        ..Default::default()
    };
    let s2 = PluginAcceptedDeclaredSurface {
        tools: vec!["tool_b".to_string()],
        channels: vec!["chat".to_string()],
        skills: vec!["skill_x".to_string()],
        ..Default::default()
    };
    let merged = CapabilitySummaryBuilder::merge_declared_surfaces(&[s1, s2]);
    assert_eq!(merged.tools, vec!["tool_a".to_string(), "tool_b".to_string()]);
    assert_eq!(merged.channels, vec!["chat".to_string()]);
    assert_eq!(merged.skills, vec!["skill_x".to_string()]);

    let bundled_grants = CapabilitySummaryBuilder::build_operator_grants(
        &PluginOrigin::Bundled,
        None,
        None,
    );
    // Bundled defaults to true for conversation access
    assert!(bundled_grants.hooks.allow_conversation_access.effective);
    assert!(!bundled_grants.hooks.allow_prompt_injection.effective);

    let installed_grants = CapabilitySummaryBuilder::build_operator_grants(
        &PluginOrigin::Installed,
        None,
        None,
    );
    // Non-bundled defaults to false for conversation access unless explicitly configured
    assert!(!installed_grants.hooks.allow_conversation_access.effective);

    // 3. Captured Plugin Registration across Capability Families
    let mut captured = CapturedPluginRegistration::new();
    assert!(captured.register_capability_provider("speechProviders", "tts.piper").is_ok());
    assert!(captured.register_capability_provider("realtimeTranscriptionProviders", "stt.whisper").is_ok());
    assert!(captured.register_capability_provider("realtimeVoiceProviders", "voice.bridge").is_ok());
    assert!(captured.register_capability_provider("mediaUnderstandingProviders", "vision.ocr").is_ok());
    assert!(captured.register_capability_provider("transcriptSourceProviders", "transcripts.live").is_ok());
    assert!(captured.register_capability_provider("imageGenerationProviders", "image.sd").is_ok());
    assert!(captured.register_capability_provider("videoGenerationProviders", "video.gen").is_ok());
    assert!(captured.register_capability_provider("musicGenerationProviders", "music.gen").is_ok());
    assert!(captured.register_capability_provider("webFetchProviders", "fetch.default").is_ok());
    assert!(captured.register_capability_provider("webSearchProviders", "search.tavily").is_ok());
    assert!(captured.register_capability_provider("embeddingProviders", "embed.text").is_ok());
    assert!(captured.register_capability_provider("nonExistentFamily", "dummy").is_err());
    assert_eq!(captured.total_registered_capabilities(), 11);

    // 4. Plugin Capability Acquisition Scope
    let current_gen = Arc::new(AtomicU64::new(1));
    let scope = PluginCapabilityAcquisitionScope::new("acq-owner", Arc::clone(&current_gen));
    assert!(scope.assert_open().is_ok());

    // Advance generation (simulating reload)
    current_gen.fetch_add(1, Ordering::SeqCst);
    let acq_err = scope.assert_open();
    assert!(acq_err.is_err());
    assert!(acq_err.unwrap_err().contains("The provider setup changed while preparing this request"));

    // Release closes scope
    scope.release();
    assert!(scope.assert_open().is_err());
}

#[test]
fn test_ocrm_plugins_catalog_discovery_icon_ranking_and_channel_registry() {
    // 1. Catalog Discovery Codec: Encoding, Decoding & Roundtrip
    let pkg_name = "@openclaw/feishu";
    let encoded_ch = CatalogDiscoveryCodec::encode_plugin_discovery_id(pkg_name).unwrap();
    assert!(encoded_ch.starts_with(CatalogDiscoveryCodec::DISCOVERY_ID_PREFIX));

    let resolved_ch = CatalogDiscoveryCodec::resolve_plugin_discovery_identity(&encoded_ch);
    assert_eq!(resolved_ch, Some(("clawhub", pkg_name.to_string())));

    let local_id = "my-local-plugin";
    let encoded_local = CatalogDiscoveryCodec::encode_local_plugin_discovery_id(local_id).unwrap();
    assert!(encoded_local.starts_with(CatalogDiscoveryCodec::LOCAL_DISCOVERY_ID_PREFIX));

    let resolved_local = CatalogDiscoveryCodec::resolve_plugin_discovery_identity(&encoded_local);
    assert_eq!(resolved_local, Some(("local", local_id.to_string())));

    // Rejection of empty or invalid discovery identities
    assert!(CatalogDiscoveryCodec::encode_plugin_discovery_id("   ").is_err());
    assert_eq!(CatalogDiscoveryCodec::resolve_plugin_discovery_identity("invalid_prefix"), None);
    assert_eq!(CatalogDiscoveryCodec::resolve_plugin_discovery_identity("ch_$$$invalid$$$"), None);

    // Join helper: plugin_catalog_id
    assert_eq!(
        CatalogDiscoveryCodec::plugin_catalog_id("ignored-id", Some("@openclaw/pkg")).unwrap(),
        CatalogDiscoveryCodec::encode_plugin_discovery_id("@openclaw/pkg").unwrap()
    );
    assert_eq!(
        CatalogDiscoveryCodec::plugin_catalog_id("local-id", None).unwrap(),
        CatalogDiscoveryCodec::encode_local_plugin_discovery_id("local-id").unwrap()
    );

    // 2. Catalog Icon Registry & Normalization Guardrails
    let valid_icon = "https://example.com/icons/feishu.png";
    assert_eq!(
        CatalogIconRegistry::normalize_catalog_icon_url(valid_icon),
        Some("https://example.com/icons/feishu.png".to_string())
    );
    // Insecure http rejected
    assert_eq!(CatalogIconRegistry::normalize_catalog_icon_url("http://example.com/icon.png"), None);
    // Fragment rejected
    assert_eq!(CatalogIconRegistry::normalize_catalog_icon_url("https://example.com/icon.png#hash"), None);
    // User/pass credentials rejected
    assert_eq!(CatalogIconRegistry::normalize_catalog_icon_url("https://user:pass@example.com/icon.png"), None);

    // Icon registry LRU behavior
    let registry = CatalogIconRegistry::new(2);
    registry.register_icon_urls(["https://example.com/1.png", "https://example.com/2.png"]);
    assert_eq!(
        registry.resolve_icon_url("https://example.com/1.png"),
        Some("https://example.com/1.png".to_string())
    );
    // Eviction on overflow
    registry.register_icon_urls(["https://example.com/3.png"]);
    assert_eq!(registry.resolve_icon_url("https://example.com/1.png"), None);
    assert_eq!(
        registry.resolve_icon_url("https://example.com/3.png"),
        Some("https://example.com/3.png".to_string())
    );

    // 3. Catalog Search Ranking & Deduplication
    assert_eq!(CatalogSearchRanking::clamp_limit(None), 20);
    assert_eq!(CatalogSearchRanking::clamp_limit(Some(0)), 20);
    assert_eq!(CatalogSearchRanking::clamp_limit(Some(5)), 5);
    assert_eq!(CatalogSearchRanking::clamp_limit(Some(500)), 100);

    let raw_search = vec![
        CatalogPackageSearchResult {
            name: "pkg-a".to_string(),
            family: "code-plugin".to_string(),
            score: 0.8,
        },
        CatalogPackageSearchResult {
            name: "pkg-a".to_string(),
            family: "code-plugin".to_string(),
            score: 0.95, // higher score wins
        },
        CatalogPackageSearchResult {
            name: "pkg-b".to_string(),
            family: "unsupported-family".to_string(),
            score: 1.0, // filtered out
        },
        CatalogPackageSearchResult {
            name: "pkg-c".to_string(),
            family: "bundle-plugin".to_string(),
            score: 0.95, // tie with pkg-a score
        },
    ];
    let ranked = CatalogSearchRanking::deduplicate_and_rank(raw_search, Some(10));
    assert_eq!(ranked.len(), 2);
    // Score 0.95 entries come first, sorted by name: pkg-a then pkg-c
    assert_eq!(ranked[0].name, "pkg-a");
    assert_eq!(ranked[0].score, 0.95);
    assert_eq!(ranked[1].name, "pkg-c");
    assert_eq!(ranked[1].score, 0.95);

    // 4. Channel Catalog Registry & Candidate Resolution
    let candidates = vec![
        ChannelCatalogCandidate {
            id_hint: Some("hint-plugin".to_string()),
            bundled_manifest_id: Some("bundled-tg".to_string()),
            package_plugin_id: Some("pkg-tg".to_string()),
            package_name: Some("@openclaw/telegram".to_string()),
            channel_id: Some("telegram".to_string()),
            channel_name: Some("Telegram".to_string()),
            origin: PluginOrigin::Bundled,
            root_dir: "/app/plugins/telegram".to_string(),
            is_trusted_official: true,
        },
        ChannelCatalogCandidate {
            id_hint: Some("custom-discord".to_string()),
            bundled_manifest_id: None,
            package_plugin_id: Some("pkg-discord".to_string()),
            package_name: Some("@vendor/discord".to_string()),
            channel_id: Some("discord".to_string()),
            channel_name: Some("Discord".to_string()),
            origin: PluginOrigin::Installed,
            root_dir: "/home/user/.openclaw/discord".to_string(),
            is_trusted_official: false,
        },
        ChannelCatalogCandidate {
            id_hint: Some("no-channel-plugin".to_string()),
            bundled_manifest_id: None,
            package_plugin_id: None,
            package_name: None,
            channel_id: None, // No channel id -> skipped
            channel_name: None,
            origin: PluginOrigin::Installed,
            root_dir: "/home/user/.openclaw/dummy".to_string(),
            is_trusted_official: false,
        },
    ];

    let all_channels = ChannelCatalogResolver::resolve_channel_catalog_entries(&candidates, None);
    assert_eq!(all_channels.len(), 2);
    assert_eq!(all_channels[0].plugin_id, "bundled-tg");
    assert_eq!(all_channels[0].channel_name, "telegram");
    assert!(all_channels[0].trusted_official_install);
    assert_eq!(all_channels[1].plugin_id, "pkg-discord");
    assert!(!all_channels[1].trusted_official_install);

    let bundled_only = ChannelCatalogResolver::resolve_channel_catalog_entries(
        &candidates,
        Some(&PluginOrigin::Bundled),
    );
    assert_eq!(bundled_only.len(), 1);
    assert_eq!(bundled_only[0].plugin_id, "bundled-tg");
}

#[test]
fn test_ocrm_plugins_channel_policy_validation_and_adoption() {
    use mahayana_host_runtime::extensions::plugins::capability_catalog::{
        ActivePluginChannelRegistration, ChannelManifestRecord, ChannelPluginConfigHandlers,
        ChannelPluginMeta, ChannelPluginValidator, ChannelPresencePolicy, ChannelPresenceSource,
        ChannelRegistryAdoption, ChannelStartupPolicy, NormalizedPluginConfigEntry,
        NormalizedPluginsConfig, PluginOrigin, RuntimeChannelSource,
    };
    use serde_json::json;
    use std::collections::HashMap;

    // 1. Channel Plugin Validation & Metadata Repair
    let valid_handlers = ChannelPluginConfigHandlers {
        has_list_accounts: true,
        has_resolve_account: true,
    };

    // 1a. Missing required config helpers -> rejected with error
    let missing_helpers_res = ChannelPluginValidator::validate_and_normalize(
        "broken-channel",
        ChannelPluginMeta::default(),
        None,
        "broken-plugin",
    );
    assert!(missing_helpers_res.is_err());
    let err = missing_helpers_res.unwrap_err();
    assert_eq!(err.level, "error");
    assert!(err.message.contains("missing required config helpers"));

    // 1b. Incomplete metadata and mismatched meta.id repaired with warnings
    let incomplete_meta = ChannelPluginMeta {
        id: "other-id".to_string(), // Mismatched meta.id
        label: "".to_string(),
        selection_label: "".to_string(),
        docs_path: "".to_string(),
        blurb: "".to_string(),
        aliases: vec![],
    };
    let (normalized_tg, diags) = ChannelPluginValidator::validate_and_normalize(
        "telegram",
        incomplete_meta,
        Some(&valid_handlers),
        "tg-plugin",
    )
    .expect("validation should succeed with repairs");

    assert_eq!(normalized_tg.id, "telegram");
    assert_eq!(normalized_tg.meta.id, "telegram");
    assert_eq!(normalized_tg.meta.label, "telegram");
    assert_eq!(normalized_tg.meta.docs_path, "/channels/telegram");
    assert_eq!(normalized_tg.meta.blurb, "Telegram bot channel");
    assert_eq!(diags.len(), 2); // meta.id mismatch warning + incomplete metadata warning
    assert_eq!(diags[0].level, "warn");
    assert!(diags[0].message.contains("meta.id mismatch"));
    assert!(diags[1].message.contains("incomplete metadata"));

    // 1c. Official external channel aliases filled if omitted
    let (normalized_wx, _) = ChannelPluginValidator::validate_and_normalize(
        "openclaw-weixin",
        ChannelPluginMeta {
            id: "openclaw-weixin".to_string(),
            label: "Weixin".to_string(),
            selection_label: "Weixin (long-poll)".to_string(),
            docs_path: "/channels/openclaw-weixin".to_string(),
            blurb: "Weixin channel".to_string(),
            aliases: vec![],
        },
        Some(&valid_handlers),
        "openclaw-weixin",
    )
    .unwrap();
    assert_eq!(
        normalized_wx.meta.aliases,
        vec!["weixin".to_string(), "wechat".to_string(), "微信".to_string()]
    );

    // 2. Channel Registry Adoption
    let target_channels = vec![ActivePluginChannelRegistration {
        plugin_id: "demo-plugin".to_string(),
        channel_id: "demo".to_string(),
        origin: PluginOrigin::Bundled,
        source_digest: "sha256-abc".to_string(),
        accepting_calls: true,
        borrowed_runtime_record: None,
    }];

    // 2a. Stale or non-current source rejected
    let stale_source = RuntimeChannelSource {
        registry_id: "donor-reg".to_string(),
        is_current: false,
    };
    let adoption_err = ChannelRegistryAdoption::adopt_runtime_channel_registrations(
        "target-reg",
        &target_channels,
        Some(&stale_source),
        "env-hash",
        "env-hash",
        "gw-1",
        "gw-1",
        false,
    );
    assert!(adoption_err.is_err());
    assert!(adoption_err
        .unwrap_err()
        .contains("Channel runtime owner changed"));

    // 2b. Successful adoption sets borrowed_runtime_record
    let active_source = RuntimeChannelSource {
        registry_id: "donor-reg".to_string(),
        is_current: true,
    };
    let adopted = ChannelRegistryAdoption::adopt_runtime_channel_registrations(
        "target-reg",
        &target_channels,
        Some(&active_source),
        "env-hash",
        "env-hash",
        "gw-1",
        "gw-1",
        false,
    )
    .unwrap();
    assert_eq!(adopted.len(), 1);
    assert_eq!(
        adopted[0].borrowed_runtime_record,
        Some("demo-plugin:borrowed".to_string())
    );

    // 3. Channel Startup Policy
    let mut plugins_config = NormalizedPluginsConfig {
        enabled: true,
        allow: vec!["allowed-plugin".to_string()],
        deny: vec!["denied-plugin".to_string()],
        entries: HashMap::new(),
    };
    plugins_config.entries.insert(
        "external-plugin".to_string(),
        NormalizedPluginConfigEntry { enabled: true },
    );

    // 3a. Denied plugin cannot start
    assert!(!ChannelStartupPolicy::can_start_configured_channel_plugin(
        "denied-plugin",
        &PluginOrigin::Bundled,
        &["ch1".to_string()],
        &plugins_config,
        &[],
    ));

    // 3b. Bundled plugin in restrictive allowlist with explicit channel config is permitted
    assert!(ChannelStartupPolicy::can_start_configured_channel_plugin(
        "bundled-unlisted",
        &PluginOrigin::Bundled,
        &["slack".to_string()],
        &plugins_config,
        &["slack".to_string()],
    ));

    // 3c. Bundled plugin without explicit channel config when not in allowlist is blocked
    assert!(!ChannelStartupPolicy::can_start_configured_channel_plugin(
        "bundled-unlisted",
        &PluginOrigin::Bundled,
        &["slack".to_string()],
        &plugins_config,
        &[],
    ));

    // 3d. Non-bundled plugin explicitly enabled in entries
    plugins_config.allow.push("external-plugin".to_string());
    assert!(ChannelStartupPolicy::can_start_configured_channel_plugin(
        "external-plugin",
        &PluginOrigin::Installed,
        &["ext-ch".to_string()],
        &plugins_config,
        &[],
    ));

    // 4. Channel Presence Policy & Configured Channel IDs
    let mut configured_channels: HashMap<String, serde_json::Value> = HashMap::new();
    configured_channels.insert(
        "slack".to_string(),
        json!({ "enabled": true, "botToken": "xoxb-123" }),
    );
    configured_channels.insert(
        "Discord".to_string(),
        json!({ "enabled": false, "botToken": "dis-123" }),
    );
    configured_channels.insert(
        "_metadata_key".to_string(),
        json!({ "some": "val" }),
    );

    // 4a. has_explicit_channel_config case-insensitive and checks enabled: false
    assert!(ChannelPresencePolicy::has_explicit_channel_config(
        &configured_channels,
        "Slack"
    ));
    assert!(!ChannelPresencePolicy::has_explicit_channel_config(
        &configured_channels,
        "discord"
    ));

    // 4b. list_explicit_configured_channel_ids filters disabled and metadata keys
    let explicit_ids =
        ChannelPresencePolicy::list_explicit_configured_channel_ids(&configured_channels);
    assert_eq!(explicit_ids, vec!["slack".to_string()]);

    // 4c. Presence policy resolution from manifests & env
    let mut env_vars = HashMap::new();
    env_vars.insert("TG_TOKEN".to_string(), "tg-secret".to_string());

    let manifests = vec![
        ChannelManifestRecord {
            plugin_id: "allowed-plugin".to_string(),
            origin: PluginOrigin::Bundled,
            channels: vec!["slack".to_string()],
            enabled_by_default: true,
            required_env: vec![],
        },
        ChannelManifestRecord {
            plugin_id: "tg-plugin".to_string(),
            origin: PluginOrigin::Bundled,
            channels: vec!["telegram".to_string()],
            enabled_by_default: true,
            required_env: vec!["TG_TOKEN".to_string()],
        },
    ];

    let open_plugins_config = NormalizedPluginsConfig {
        enabled: true,
        allow: vec![],
        deny: vec![],
        entries: HashMap::new(),
    };

    let policy_entries = ChannelPresencePolicy::resolve_channel_presence_policy(
        &configured_channels,
        &env_vars,
        &manifests,
        &open_plugins_config,
        false,
    );
    assert_eq!(policy_entries.len(), 2);
    assert_eq!(policy_entries[0].channel_id, "slack");
    assert!(policy_entries[0].effective);
    assert_eq!(policy_entries[0].plugin_ids, vec!["allowed-plugin"]);
    assert_eq!(
        policy_entries[0].sources,
        vec![ChannelPresenceSource::ExplicitConfig]
    );

    assert_eq!(policy_entries[1].channel_id, "telegram");
    assert!(policy_entries[1].effective);
    assert_eq!(
        policy_entries[1].sources,
        vec![ChannelPresenceSource::ManifestEnv]
    );

    // 4d. Announce channel IDs
    let announced = ChannelPresencePolicy::list_configured_announce_channel_ids(
        &configured_channels,
        &env_vars,
        &manifests,
        &open_plugins_config,
    );
    assert_eq!(announced, vec!["slack".to_string(), "telegram".to_string()]);

    // 4e. Configured Channel Plugin IDs
    let plugin_ids = ChannelPresencePolicy::resolve_configured_channel_plugin_ids(
        &configured_channels,
        &env_vars,
        &manifests,
        &open_plugins_config,
    );
    assert_eq!(
        plugin_ids,
        vec!["allowed-plugin".to_string(), "tg-plugin".to_string()]
    );
}

#[test]
fn test_ocrm_plugins_clawhub_package_archive_verification_and_install_policy() {
    use mahayana_host_runtime::extensions::plugins::clawhub::{
        format_clawhub_release_label, format_clawhub_specifier, is_default_official_clawhub_package,
        resolve_clawhub_plugin_url, ClawHubArchiveVerification, ClawHubFileVerificationEntry,
        ClawHubInstallErrorCode, ClawHubInstallFailure, ClawHubInstallPipeline,
        ClawHubSecurityAssessment, CLAWHUB_GENERATED_ARCHIVE_METADATA_FILE,
    };
    use std::collections::HashMap;

    // 1. Error Codes & Target Availability
    let not_found = ClawHubInstallFailure::new(
        "Package not found",
        Some(ClawHubInstallErrorCode::PackageNotFound),
        None,
        None,
    );
    assert!(not_found.is_unavailable_target());

    let version_not_found = ClawHubInstallFailure::new(
        "Version not found",
        Some(ClawHubInstallErrorCode::VersionNotFound),
        None,
        None,
    );
    assert!(version_not_found.is_unavailable_target());

    let download_blocked = ClawHubInstallFailure::new(
        "Malicious",
        Some(ClawHubInstallErrorCode::ClawhubDownloadBlocked),
        None,
        None,
    );
    assert!(!download_blocked.is_unavailable_target());

    // 2. Official ClawHub Source Resolution
    assert!(is_default_official_clawhub_package(None, Some("official"), false));
    assert!(is_default_official_clawhub_package(
        Some("https://clawhub.ai"),
        None,
        true
    ));
    assert!(!is_default_official_clawhub_package(
        Some("https://third-party.registry.org"),
        Some("official"),
        true
    ));

    // 3. Presentation Formatting
    assert_eq!(
        format_clawhub_specifier("demo", Some("1.2.3")),
        "clawhub:demo@1.2.3"
    );
    assert_eq!(format_clawhub_specifier("demo", None), "clawhub:demo");
    assert_eq!(format_clawhub_release_label("demo", "1.2.3"), "demo@1.2.3");
    assert_eq!(
        resolve_clawhub_plugin_url(None, "demo"),
        "https://clawhub.ai/plugins/demo"
    );

    // 4. Archive Verification & _meta.json Validation
    let valid_meta = serde_json::json!({
        "slug": "demo-plugin",
        "version": "2026.3.22"
    });
    let valid_meta_bytes = serde_json::to_vec(&valid_meta).unwrap();
    assert!(ClawHubArchiveVerification::validate_meta_json(
        "demo-plugin",
        "2026.3.22",
        &valid_meta_bytes
    )
    .is_ok());

    let invalid_slug_meta = serde_json::json!({
        "slug": "wrong-slug",
        "version": "2026.3.22"
    });
    let mismatch_res = ClawHubArchiveVerification::validate_meta_json(
        "demo-plugin",
        "2026.3.22",
        &serde_json::to_vec(&invalid_slug_meta).unwrap(),
    );
    assert!(mismatch_res.is_err());
    assert_eq!(
        mismatch_res.unwrap_err().code,
        Some(ClawHubInstallErrorCode::ArchiveIntegrityMismatch)
    );

    // 4b. verify_extracted_files
    let mut actual_files = HashMap::new();
    actual_files.insert(
        CLAWHUB_GENERATED_ARCHIVE_METADATA_FILE.to_string(),
        (valid_meta_bytes, "sha256-meta".to_string()),
    );
    actual_files.insert(
        "index.js".to_string(),
        (b"console.log('hi');".to_vec(), "sha256-index".to_string()),
    );
    actual_files.insert(
        "openclaw.plugin.json".to_string(),
        (b"{}".to_vec(), "sha256-manifest".to_string()),
    );

    let expected_files = vec![
        ClawHubFileVerificationEntry {
            path: "index.js".to_string(),
            sha256: "sha256-index".to_string(),
        },
        ClawHubFileVerificationEntry {
            path: "openclaw.plugin.json".to_string(),
            sha256: "sha256-manifest".to_string(),
        },
    ];

    let verified_paths = ClawHubArchiveVerification::verify_extracted_files(
        &actual_files,
        &expected_files,
        "demo-plugin",
        "2026.3.22",
    )
    .expect("verification should succeed");
    assert_eq!(
        verified_paths,
        vec![CLAWHUB_GENERATED_ARCHIVE_METADATA_FILE.to_string()]
    );

    // 4c. verify_extracted_files missing file failure
    let missing_expected = vec![
        ClawHubFileVerificationEntry {
            path: "index.js".to_string(),
            sha256: "sha256-index".to_string(),
        },
        ClawHubFileVerificationEntry {
            path: "missing.js".to_string(),
            sha256: "sha256-missing".to_string(),
        },
    ];
    let missing_err = ClawHubArchiveVerification::verify_extracted_files(
        &actual_files,
        &missing_expected,
        "demo-plugin",
        "2026.3.22",
    )
    .unwrap_err();
    assert!(missing_err.error.contains("missing \"missing.js\""));

    // 4d. verify_extracted_files hash mismatch
    let mismatch_expected = vec![
        ClawHubFileVerificationEntry {
            path: "index.js".to_string(),
            sha256: "sha256-wrong".to_string(),
        },
        ClawHubFileVerificationEntry {
            path: "openclaw.plugin.json".to_string(),
            sha256: "sha256-manifest".to_string(),
        },
    ];
    let mismatch_err = ClawHubArchiveVerification::verify_extracted_files(
        &actual_files,
        &mismatch_expected,
        "demo-plugin",
        "2026.3.22",
    )
    .unwrap_err();
    assert!(mismatch_err.error.contains("expected index.js to hash to"));

    // 4e. verify_extracted_files unexpected file
    let mut actual_with_extra = actual_files.clone();
    actual_with_extra.insert(
        "extra_file.txt".to_string(),
        (b"extra".to_vec(), "sha256-extra".to_string()),
    );
    let extra_err = ClawHubArchiveVerification::verify_extracted_files(
        &actual_with_extra,
        &expected_files,
        "demo-plugin",
        "2026.3.22",
    )
    .unwrap_err();
    assert!(extra_err.error.contains("unexpected file \"extra_file.txt\""));

    // 5. Install Pipeline Assessment & Guardrails
    // 5a. Skill family rejected
    let skill_err =
        ClawHubInstallPipeline::assess_package_eligibility("skill", None, "my-skill").unwrap_err();
    assert_eq!(skill_err.code, Some(ClawHubInstallErrorCode::SkillPackage));
    assert!(skill_err.error.contains("Use \"openclaw skills install"));

    // 5b. Unsupported family rejected
    let unsupp_err =
        ClawHubInstallPipeline::assess_package_eligibility("unsupported", None, "pkg").unwrap_err();
    assert_eq!(
        unsupp_err.code,
        Some(ClawHubInstallErrorCode::UnsupportedFamily)
    );

    // 5c. Private package rejected without auth
    let priv_err = ClawHubInstallPipeline::assess_package_eligibility(
        "code-plugin",
        Some("private"),
        "corp-plugin",
    )
    .unwrap_err();
    assert_eq!(
        priv_err.code,
        Some(ClawHubInstallErrorCode::PrivatePackage)
    );

    // 5d. Code-plugin and bundle-plugin accepted
    assert!(
        ClawHubInstallPipeline::assess_package_eligibility("code-plugin", None, "my-plugin").is_ok()
    );
    assert!(ClawHubInstallPipeline::assess_package_eligibility(
        "bundle-plugin",
        Some("official"),
        "my-bundle"
    )
    .is_ok());

    // 5e. Security verdict evaluation
    let blocked_sec = ClawHubSecurityAssessment {
        package_name: "bad-plugin".to_string(),
        release_version: "1.0.0".to_string(),
        verdict: "blocked".to_string(),
        scan_status: "malicious".to_string(),
        blocked_from_download: true,
        reasons: vec!["Contains malware".to_string()],
        overview: "Known malicious code".to_string(),
    };
    let sec_err = ClawHubInstallPipeline::assess_security_verdict(&blocked_sec).unwrap_err();
    assert_eq!(
        sec_err.code,
        Some(ClawHubInstallErrorCode::ClawhubDownloadBlocked)
    );

    let review_sec = ClawHubSecurityAssessment {
        package_name: "suspicious-plugin".to_string(),
        release_version: "1.0.0".to_string(),
        verdict: "review-recommended".to_string(),
        scan_status: "warn".to_string(),
        blocked_from_download: false,
        reasons: vec!["Obfuscated code".to_string()],
        overview: "Uses dynamic eval".to_string(),
    };
    let review_warning =
        ClawHubInstallPipeline::assess_security_verdict(&review_sec).expect("should not fail");
    assert!(review_warning.is_some());
    assert!(review_warning
        .unwrap()
        .contains("Security review recommended"));
}

#[test]
fn test_ocrm_plugins_cli_backends_and_command_runtime() {
    use mahayana_host_runtime::extensions::plugins::cli_backend::{
        warn_on_missing_cli_backend_model_provider, CliBackendConfig, CliBackendPluginDescriptor,
        CliBackendRuntimeRegistry, CliCommandRegistry, CliGatewayNodeEntry,
        CliGatewayNodesRuntime, CliNodeInvokeParams, CliRootCommandDescriptor,
    };
    use std::collections::HashMap;

    // 1. Warning on missing modelProvider
    let missing_warn = warn_on_missing_cli_backend_model_provider("my-backend", None);
    assert!(missing_warn.is_some());
    assert!(missing_warn
        .unwrap()
        .contains("is missing modelProvider; provider-to-runtime binding will be skipped"));

    let empty_warn = warn_on_missing_cli_backend_model_provider("my-backend", Some("   "));
    assert!(empty_warn.is_some());

    let valid_warn = warn_on_missing_cli_backend_model_provider("my-backend", Some("anthropic"));
    assert!(valid_warn.is_none());

    // 2. CliBackendRuntimeRegistry operations
    let registry = CliBackendRuntimeRegistry::new();
    let mut model_aliases = HashMap::new();
    model_aliases.insert("smart".to_string(), "claude-3-5-sonnet".to_string());

    let descriptor = CliBackendPluginDescriptor {
        id: "fixture-cli".to_string(),
        model_provider: Some("anthropic".to_string()),
        config: CliBackendConfig {
            command: "anthropic-cli".to_string(),
            args: vec!["--json".to_string()],
            output: "json".to_string(),
            input: "arg".to_string(),
            max_prompt_arg_chars: Some(4096),
            model_arg: Some("--model".to_string()),
            model_aliases,
            session_args: vec!["--session".to_string(), "{sessionId}".to_string()],
            resume_args: vec!["--resume".to_string(), "{sessionId}".to_string()],
            session_mode: "existing".to_string(),
        },
        subscription_auth_dispatch: true,
        plugin_id: "anthropic-plugin".to_string(),
        built_with_version: Some("2026.3.22".to_string()),
    };

    registry.register(descriptor);

    // 2a. Case-insensitive provider probe
    assert!(registry.is_cli_provider(" FIXTURE-CLI "));
    assert!(registry.is_cli_provider("fixture-cli"));
    assert!(!registry.is_cli_provider("unknown-cli"));

    // 2b. Canonical provider resolution & binding listing
    assert_eq!(
        registry.resolve_canonical_provider("fixture-cli"),
        Some("anthropic".to_string())
    );
    let bindings = registry.list_bindings();
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].provider, "anthropic");
    assert_eq!(bindings[0].runtime, "fixture-cli");

    // 2c. Model ID resolution with alias or prefix
    assert_eq!(
        registry.resolve_model_id("fixture-cli", "smart").unwrap(),
        "claude-3-5-sonnet"
    );
    assert_eq!(
        registry.resolve_model_id("fixture-cli", "haiku").unwrap(),
        "anthropic:haiku"
    );

    // 2d. Inactive / retired owner throws error
    registry.unregister_owner("anthropic-plugin");
    let retired_err = registry.resolve_config("fixture-cli").unwrap_err();
    assert!(retired_err.contains("retired or disabled"));
    let retired_model_err = registry.resolve_model_id("fixture-cli", "smart").unwrap_err();
    assert!(retired_model_err.contains("retired or disabled"));

    // 3. CliGatewayNodesRuntime
    let nodes = vec![
        CliGatewayNodeEntry {
            node_id: "node-1".to_string(),
            connected: true,
        },
        CliGatewayNodeEntry {
            node_id: "node-2".to_string(),
            connected: false,
        },
    ];
    let all_nodes = CliGatewayNodesRuntime::filter_nodes(nodes.clone(), false);
    assert_eq!(all_nodes.len(), 2);
    let connected_nodes = CliGatewayNodesRuntime::filter_nodes(nodes, true);
    assert_eq!(connected_nodes.len(), 1);
    assert_eq!(connected_nodes[0].node_id, "node-1");

    // 3b. prepare_node_invoke_request
    let raw_invoke = CliNodeInvokeParams {
        node_id: "node-1".to_string(),
        command: "exec".to_string(),
        params: None,
        timeout_ms: Some(5000),
        idempotency_key: None, // Missing idempotency key
        session_key: Some("sess-1".to_string()),
        scopes: Some(vec!["admin".to_string()]),
    };
    // Non-bundled caller has scopes stripped
    let prepared_untrusted =
        CliGatewayNodesRuntime::prepare_node_invoke_request(raw_invoke.clone(), false);
    assert!(prepared_untrusted.idempotency_key.is_some());
    assert!(prepared_untrusted.scopes.is_none());

    // Bundled caller preserves scopes
    let prepared_bundled =
        CliGatewayNodesRuntime::prepare_node_invoke_request(raw_invoke, true);
    assert_eq!(prepared_bundled.scopes, Some(vec!["admin".to_string()]));

    // 3c. open_duplex is unavailable in CLI
    let duplex_res = CliGatewayNodesRuntime::open_duplex();
    assert!(duplex_res.is_err());
    assert!(duplex_res.unwrap_err().contains("Node duplex is unavailable in the CLI"));

    // 4. CliCommandRegistry Descriptors & Lazy Registration Policy
    let group1 = vec![
        CliRootCommandDescriptor {
            name: "status".to_string(),
            description: "Show status".to_string(),
            aliases: vec!["st".to_string()],
            machine_output: true,
        },
        CliRootCommandDescriptor {
            name: "help".to_string(),
            description: "Help".to_string(),
            aliases: vec![],
            machine_output: false,
        },
    ];
    let group2 = vec![
        CliRootCommandDescriptor {
            name: "status".to_string(), // Duplicate name
            description: "Duplicate status".to_string(),
            aliases: vec![],
            machine_output: false,
        },
        CliRootCommandDescriptor {
            name: "plugins".to_string(),
            description: "Manage plugins".to_string(),
            aliases: vec![],
            machine_output: true,
        },
    ];

    let unique = CliCommandRegistry::collect_unique_command_descriptors(vec![group1, group2]);
    assert_eq!(unique.len(), 3);
    assert_eq!(unique[0].name, "help");
    assert_eq!(unique[1].name, "plugins");
    assert_eq!(unique[2].name, "status");
    assert_eq!(unique[2].description, "Show status"); // First one kept

    // 4b. Lazy registration test
    let placeholders = vec![
        CliRootCommandDescriptor {
            name: "plugins".to_string(),
            description: "".to_string(),
            aliases: vec![],
            machine_output: false,
        },
    ];
    assert!(CliCommandRegistry::can_register_lazily(
        &["plugins".to_string()],
        &placeholders
    ));
    assert!(!CliCommandRegistry::can_register_lazily(
        &["plugins".to_string(), "extra".to_string()],
        &placeholders
    ));
}

#[test]
fn test_ocrm_plugins_command_registration_execution_and_compaction() {
    use mahayana_host_runtime::extensions::plugins::commands::{
        is_reserved_command_name, CodeModeExecutorResolver, CodeModeOwnerCandidate,
        CommandExecutionLock, CommandRegistry, CommandValidator, CompactionProviderRegistry,
        PluginCommandDefinition, RegisteredPluginCommand,
    };

    // 1. Reserved Command Names
    assert!(is_reserved_command_name("HELP"));
    assert!(is_reserved_command_name("status"));
    assert!(is_reserved_command_name("whoami"));
    assert!(is_reserved_command_name("compact"));
    assert!(!is_reserved_command_name("custom_command"));

    // 2. CommandValidator
    let custom_cmd = PluginCommandDefinition {
        name: "my_cmd".to_string(),
        description: "My command".to_string(),
        accepts_args: true,
        required_scopes: vec![],
        ownership: None,
        channels: vec![],
        agent_prompt_guidance: vec![],
    };
    assert!(CommandValidator::validate_command(&custom_cmd, false).is_ok());

    let reserved_cmd = PluginCommandDefinition {
        name: "status".to_string(),
        description: "Status cmd".to_string(),
        accepts_args: false,
        required_scopes: vec![],
        ownership: None,
        channels: vec![],
        agent_prompt_guidance: vec![],
    };
    assert!(CommandValidator::validate_command(&reserved_cmd, false).is_err());
    assert!(CommandValidator::validate_command(&reserved_cmd, true).is_ok());

    let empty_name_cmd = PluginCommandDefinition {
        name: "   ".to_string(),
        description: "Empty".to_string(),
        accepts_args: false,
        required_scopes: vec![],
        ownership: None,
        channels: vec![],
        agent_prompt_guidance: vec![],
    };
    assert!(CommandValidator::validate_command(&empty_name_cmd, false).is_err());

    // 3. CommandExecutionLock
    let lock = CommandExecutionLock::new();
    assert_eq!(lock.get_active_count(), 0);

    let executed = lock
        .with_execution(|| {
            assert_eq!(lock.get_active_count(), 1);
            42
        })
        .unwrap();
    assert_eq!(executed, 42);
    assert_eq!(lock.get_active_count(), 0);

    lock.retire();
    assert!(lock.with_execution(|| 0).is_err());

    // 4. CodeModeExecutorResolver
    let candidates = vec![
        CodeModeOwnerCandidate {
            plugin_id: "bundled-executor".to_string(),
            is_bundled: true,
            code_mode_executors: vec!["python-env".to_string()],
            available_for_control_plane: true,
            passes_base_policy: true,
        },
        CodeModeOwnerCandidate {
            plugin_id: "disabled-executor".to_string(),
            is_bundled: false,
            code_mode_executors: vec!["node-env".to_string()],
            available_for_control_plane: false,
            passes_base_policy: true,
        },
    ];

    let resolved = CodeModeExecutorResolver::resolve_executor("python-env", &candidates).unwrap();
    assert_eq!(resolved, "bundled-executor");

    let disabled_err =
        CodeModeExecutorResolver::resolve_executor("node-env", &candidates).unwrap_err();
    assert!(disabled_err.contains("unavailable or disabled"));

    let duplicate_candidates = vec![
        CodeModeOwnerCandidate {
            plugin_id: "owner-1".to_string(),
            is_bundled: true,
            code_mode_executors: vec!["shared-env".to_string()],
            available_for_control_plane: true,
            passes_base_policy: true,
        },
        CodeModeOwnerCandidate {
            plugin_id: "owner-2".to_string(),
            is_bundled: true,
            code_mode_executors: vec!["shared-env".to_string()],
            available_for_control_plane: true,
            passes_base_policy: true,
        },
    ];
    let dup_err =
        CodeModeExecutorResolver::resolve_executor("shared-env", &duplicate_candidates)
            .unwrap_err();
    assert!(dup_err.contains("multiple plugin owners"));

    // 5. CompactionProviderRegistry
    let compaction_reg = CompactionProviderRegistry::new();
    assert!(compaction_reg
        .register("compact-1", "Default Compaction", "plugin-a")
        .is_ok());

    // Duplicate registration rejected
    let dup_compaction = compaction_reg.register("compact-1", "Other", "plugin-b");
    assert!(dup_compaction.is_err());
    assert!(dup_compaction
        .unwrap_err()
        .contains("compaction provider already registered"));

    // Lookup succeeds for active owner
    let provider = compaction_reg.get_provider("compact-1").unwrap();
    assert_eq!(provider.id, "compact-1");
    assert_eq!(provider.owner_plugin_id, "plugin-a");

    // Retired owner fails
    compaction_reg.unregister_owner("plugin-a");
    let retired_compaction = compaction_reg.get_provider("compact-1");
    assert!(retired_compaction.is_err());
    assert!(retired_compaction
        .unwrap_err()
        .contains("reloaded, disabled or retired"));

    // 6. CommandRegistry Matching & Agent Prompt Guidance
    let registered_commands = vec![
        RegisteredPluginCommand {
            command: PluginCommandDefinition {
                name: "echo".to_string(),
                description: "Echo message".to_string(),
                accepts_args: true,
                required_scopes: vec![],
                ownership: None,
                channels: vec!["slack".to_string()],
                agent_prompt_guidance: vec!["Use /echo to test connectivity.".to_string()],
            },
            plugin_id: "echo-plugin".to_string(),
            trusted_owner_status_exposure: false,
        },
        RegisteredPluginCommand {
            command: PluginCommandDefinition {
                name: "audit".to_string(),
                description: "Run audit".to_string(),
                accepts_args: false,
                required_scopes: vec!["admin".to_string()],
                ownership: Some("reserved".to_string()),
                channels: vec![],
                agent_prompt_guidance: vec!["Use /audit to verify permissions.".to_string()],
            },
            plugin_id: "audit-plugin".to_string(),
            trusted_owner_status_exposure: true,
        },
    ];

    // 6a. Match with slash prefix and channel filtering
    let matched_echo =
        CommandRegistry::match_command(&registered_commands, "/echo hello world", Some("slack"));
    assert!(matched_echo.is_some());
    let (cmd, args) = matched_echo.unwrap();
    assert_eq!(cmd.command.name, "echo");
    assert_eq!(args, Some("hello world".to_string()));

    // Echo command rejected on telegram channel
    let echo_wrong_ch =
        CommandRegistry::match_command(&registered_commands, "/echo hello", Some("telegram"));
    assert!(echo_wrong_ch.is_none());

    // Audit command matches without slash and has owner exposure
    let matched_audit = CommandRegistry::match_command(&registered_commands, "audit", None);
    assert!(matched_audit.is_some());
    let (audit_cmd, _) = matched_audit.unwrap();
    assert!(audit_cmd.is_trusted_reserved_owner());
    assert!(audit_cmd.can_expose_sender_is_owner());

    // 6b. list_agent_prompt_guidance sorted deterministically by plugin_id then command name
    let guidance = CommandRegistry::list_agent_prompt_guidance(&registered_commands);
    assert_eq!(guidance.len(), 2);
    assert_eq!(guidance[0], "Use /audit to verify permissions."); // audit-plugin < echo-plugin
    assert_eq!(guidance[1], "Use /echo to test connectivity.");
}

#[test]
fn test_plugin_compat_registry_validation_and_records() {
    use mahayana_host_runtime::extensions::plugins::compat::{
        get_compat_record, list_plugin_compat_records, validate_all_records,
        PluginCompatOwner, PluginCompatStatus,
    };

    let records = list_plugin_compat_records();
    assert_eq!(records.len(), 146);

    // Validate all records against upstream Vitest invariant rules
    let validation_result = validate_all_records();
    assert!(
        validation_result.is_ok(),
        "Validation failed: {:?}",
        validation_result.err()
    );

    // Test specific known records
    let act_rec = get_compat_record("activation-agent-harness-hint").expect("must exist");
    assert_eq!(act_rec.status, PluginCompatStatus::Active);
    assert_eq!(act_rec.owner, PluginCompatOwner::PluginExecution);
    assert_eq!(act_rec.docs_path, "/plugins/manifest");
    assert!(act_rec.remove_after.is_none());

    let removed_rec =
        get_compat_record("plugin-sdk-allowlist-resolution-entry-mapper").expect("must exist");
    assert_eq!(removed_rec.status, PluginCompatStatus::Removed);
    assert_eq!(removed_rec.owner, PluginCompatOwner::Channel);
    assert_eq!(removed_rec.remove_after.as_deref(), Some("2026-10-04"));

    let fallback_rec = get_compat_record("setup-runtime-fallback").expect("must exist");
    assert_eq!(fallback_rec.status, PluginCompatStatus::Active);
    assert_eq!(fallback_rec.owner, PluginCompatOwner::Setup);

    let demotion_rec =
        get_compat_record("plugin-sdk-tool-plugin-public-demotion").expect("must exist");
    assert_eq!(demotion_rec.status, PluginCompatStatus::Deprecated);
    assert_eq!(demotion_rec.owner, PluginCompatOwner::Sdk);
    assert!(demotion_rec.remove_after.is_none()); // Pending removal code
}

#[test]
fn test_conversation_read_tools_host_restriction() {
    use mahayana_host_runtime::extensions::plugins::compat::{
        is_bundled_conversation_read_tool_registration,
        is_host_restricted_conversation_read_tool,
        registration_includes_host_restricted_conversation_read_tool,
    };

    // Feishu chat is restricted
    assert!(is_host_restricted_conversation_read_tool("feishu", "feishu_chat"));
    assert!(is_host_restricted_conversation_read_tool("  FEISHU ", " FEISHU_CHAT  "));
    // Other tools are not
    assert!(!is_host_restricted_conversation_read_tool("feishu", "feishu_doc"));
    assert!(!is_host_restricted_conversation_read_tool("slack", "slack_chat"));

    // Producible names gate takes precedence over declaredNames
    let entry_doc = vec!["feishu_doc".to_string()];
    let declared_all = vec!["feishu_chat".to_string(), "feishu_doc".to_string()];
    assert!(!registration_includes_host_restricted_conversation_read_tool(
        "feishu",
        &entry_doc,
        &declared_all
    ));

    // Fallback to declaredNames when producible names is empty
    let entry_empty: Vec<String> = vec![];
    assert!(registration_includes_host_restricted_conversation_read_tool(
        "feishu",
        &entry_empty,
        &declared_all
    ));

    // Bundled registration check
    assert!(is_bundled_conversation_read_tool_registration(true, true));
    assert!(!is_bundled_conversation_read_tool_registration(true, false));
    assert!(!is_bundled_conversation_read_tool_registration(false, true));
}

#[test]
fn test_plugin_sdk_deprecation_tracker() {
    use mahayana_host_runtime::extensions::plugins::compat::{
        warn_mention_inbox_deprecation, warn_model_account_connect_deprecation,
        warn_session_transcript_preparation_deprecation, PluginSdkDeprecationTracker,
    };

    let mut tracker = PluginSdkDeprecationTracker::new();

    // Mention inbox warning (non-sqlite removal family)
    let warn1 = warn_mention_inbox_deprecation(
        &mut tracker,
        Some("mention-compat-plugin"),
        "list",
    );
    assert!(warn1.is_some());
    let w1 = warn1.unwrap();
    assert_eq!(w1.code, "DEP_SESSION_PERSISTENCE");
    assert!(w1.message.contains("[mention-compat-plugin]"));
    assert!(w1.message.contains("mentionInbox.list is deprecated"));
    assert!(w1.message.contains("mentionInbox.listAsync"));
    assert!(w1.message.contains("Removal requires an announced ordinary stable release window"));

    // Duplicate call from same plugin & method is deduplicated
    let warn_dup = warn_mention_inbox_deprecation(
        &mut tracker,
        Some("mention-compat-plugin"),
        "list",
    );
    assert!(warn_dup.is_none());

    // Call from another plugin emits
    let warn2 = warn_mention_inbox_deprecation(
        &mut tracker,
        Some("other-plugin"),
        "list",
    );
    assert!(warn2.is_some());

    // Model account connect warning (sqlite removal family: Final compatibility day: 2026-12-31)
    let warn3 = warn_model_account_connect_deprecation(
        &mut tracker,
        Some("model-account-plugin"),
        "listLinks",
    );
    assert!(warn3.is_some());
    let w3 = warn3.unwrap();
    assert!(w3.message.contains("modelAccountConnectService.listLinks"));
    assert!(w3.message.contains("modelAccountConnectService.listLinksAsync"));
    assert!(w3.message.contains("Final compatibility day: 2026-12-31"));

    // Session transcript preparation warning
    let warn4 = warn_session_transcript_preparation_deprecation(
        &mut tracker,
        Some("transcript-plugin"),
        "prepareLegacyTranscript",
        "prepareTranscriptAsync",
    );
    assert!(warn4.is_some());
    let w4 = warn4.unwrap();
    assert!(w4.message.contains("prepareLegacyTranscript is deprecated; use prepareTranscriptAsync"));
}

#[test]
fn test_computer_use_contract_and_execution_manager() {
    use mahayana_host_runtime::extensions::plugins::computer_use::{
        computer_act_v1_action_names, computer_use_v1_action_names,
        is_valid_computer_execution_id, ComputerExecutionManager, COMPUTER_HOST_BUSY,
        COMPUTER_USE_V2_ACTION_NAMES,
    };

    assert_eq!(COMPUTER_USE_V2_ACTION_NAMES.len(), 40);
    assert_eq!(computer_use_v1_action_names().len(), 15);
    assert_eq!(computer_act_v1_action_names().len(), 13);

    // Execution ID validation
    assert!(is_valid_computer_execution_id(
        "12345678-1234-4234-8234-123456789abc"
    ));
    assert!(!is_valid_computer_execution_id("invalid-id"));
    assert!(!is_valid_computer_execution_id(""));
    assert!(!is_valid_computer_execution_id(
        "12345678-1234-6234-8234-123456789abc"
    ));
    assert!(!is_valid_computer_execution_id(
        "12345678-1234-4234-0234-123456789abc"
    ));
    assert!(!is_valid_computer_execution_id(
        "12345678-1234-4234-8234-123456789ABC"
    ));

    let manager = ComputerExecutionManager::new();
    assert!(!manager.is_active());

    // Invalid execution ID rejected
    let bad_open = manager.open_execution("not-a-uuid", None);
    assert!(bad_open.is_err());

    let exec_1 = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    let exec_2 = "11111111-2222-4333-8444-555555555555";

    // First open succeeds
    let ok_open = manager.open_execution(exec_1, Some("session-1".to_string()));
    assert!(ok_open.is_ok());
    assert!(manager.is_active());

    // Same ID idempotent open succeeds
    assert!(manager.open_execution(exec_1, None).is_ok());

    // Different ID rejected while busy
    let busy_err = manager.open_execution(exec_2, None);
    assert!(busy_err.is_err());
    assert_eq!(busy_err.unwrap_err(), COMPUTER_HOST_BUSY);

    // Verification
    assert!(manager.verify_execution(Some(exec_1)).is_ok());
    assert_eq!(
        manager.verify_execution(Some(exec_2)).unwrap_err(),
        COMPUTER_HOST_BUSY
    );
    assert!(manager.verify_execution(None).is_err());

    // Close execution releases lock
    assert!(manager.close_execution(exec_1, "done").is_ok());
    assert!(!manager.is_active());

    // Now exec_2 can acquire
    assert!(manager.open_execution(exec_2, None).is_ok());
    assert!(manager.is_active());
}

#[test]
fn test_plugin_config_contract_matching_and_activation() {
    use mahayana_host_runtime::extensions::plugins::config::{
        collect_plugin_config_contract_matches, has_plugin_config_migration_source,
        resolve_memory_slot_decision, resolve_plugin_activation_state,
        NormalizedPluginsConfig, PluginActivationSource, PluginEntryConfig,
    };
    use serde_json::json;

    // 1. Path pattern contract matching
    let root = json!({
        "plugins": {
            "entries": {
                "my_plugin": {
                    "apiKey": "secret-123",
                    "endpoints": ["https://api.one", "https://api.two"]
                }
            }
        }
    });

    let matches_exact = collect_plugin_config_contract_matches(
        &root,
        "plugins.entries.my_plugin.apiKey",
    );
    assert_eq!(matches_exact.len(), 1);
    assert_eq!(matches_exact[0].value, json!("secret-123"));

    let matches_wildcard = collect_plugin_config_contract_matches(
        &root,
        "plugins.entries.*.apiKey",
    );
    assert_eq!(matches_wildcard.len(), 1);
    assert_eq!(matches_wildcard[0].path, "plugins.entries.my_plugin.apiKey");

    let matches_array = collect_plugin_config_contract_matches(
        &root,
        "plugins.entries.my_plugin.endpoints.*",
    );
    assert_eq!(matches_array.len(), 2);
    assert_eq!(matches_array[0].path, "plugins.entries.my_plugin.endpoints.0");
    assert_eq!(matches_array[1].value, json!("https://api.two"));

    // Migration source check
    let patterns = vec!["plugins.entries.*.apiKey".to_string()];
    assert!(has_plugin_config_migration_source(&root, &patterns, None));

    // 2. Activation state resolution
    let mut config = NormalizedPluginsConfig::default();
    config.entries.insert(
        "custom-plugin".to_string(),
        PluginEntryConfig {
            enabled: Some(true),
            ..Default::default()
        },
    );
    config.entries.insert(
        "disabled-plugin".to_string(),
        PluginEntryConfig {
            enabled: Some(false),
            ..Default::default()
        },
    );
    config.slots.memory = Some("active-memory".to_string());
    config.allow.push("allowed-external".to_string());

    // Explicitly enabled
    let s_explicit = resolve_plugin_activation_state("custom-plugin", "bundled", &config, false);
    assert!(s_explicit.enabled);
    assert_eq!(s_explicit.source, PluginActivationSource::Explicit);

    // Explicitly disabled
    let s_disabled = resolve_plugin_activation_state("disabled-plugin", "bundled", &config, true);
    assert!(!s_disabled.enabled);
    assert_eq!(s_disabled.source, PluginActivationSource::Disabled);

    // Selected memory slot
    let s_mem = resolve_plugin_activation_state("active-memory", "bundled", &config, false);
    assert!(s_mem.enabled);
    assert_eq!(s_mem.source, PluginActivationSource::Explicit);

    // Allowed external plugin
    let s_ext = resolve_plugin_activation_state("allowed-external", "external", &config, false);
    assert!(s_ext.enabled);
    assert_eq!(s_ext.source, PluginActivationSource::Explicit);

    // Default enabled
    let s_def = resolve_plugin_activation_state("unconfigured", "bundled", &config, true);
    assert!(s_def.enabled);
    assert_eq!(s_def.source, PluginActivationSource::Default);

    // Auto disabled
    let s_auto = resolve_plugin_activation_state("unconfigured", "bundled", &config, false);
    assert!(!s_auto.enabled);
    assert_eq!(s_auto.source, PluginActivationSource::Auto);

    // 3. Memory slot decision
    let available = vec!["mem-a".to_string(), "mem-b".to_string()];
    let dec_explicit = resolve_memory_slot_decision(&config, &available);
    assert_eq!(dec_explicit.selected_plugin_id.as_deref(), Some("active-memory"));
    assert!(!dec_explicit.disabled);

    let config_empty = NormalizedPluginsConfig::default();
    let dec_single = resolve_memory_slot_decision(&config_empty, &["single-mem".to_string()]);
    assert_eq!(dec_single.selected_plugin_id.as_deref(), Some("single-mem"));
    assert!(!dec_single.disabled);
}














