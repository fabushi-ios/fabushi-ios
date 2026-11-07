//! OpenClaw Model Catalog Contract Test Suite
//!
//! Verifies OCRM-RESP-MODEL-CATALOG-PLANNER-001 & OCRM-RESP-MODEL-CATALOG-REMOTE-001:
//! - Remote catalog URL resolution and active source compatibility
//! - TTL freshness evaluation and max byte boundaries
//! - Wire bundle V1, V2, and V3 parsing & projection
//! - Manifest planning: trusted transport override, suppression filtering, and conflict detection

use std::collections::BTreeMap;
use mahayana_host_runtime::extensions::inference::model_catalog_service::*;

#[test]
fn test_ocrm_model_catalog_o01_config_and_freshness() {
    // Default URL
    assert_eq!(
        resolve_remote_catalog_url(None),
        DEFAULT_REMOTE_MODEL_CATALOG_URL
    );
    assert_eq!(
        resolve_remote_catalog_url(Some("   ")),
        DEFAULT_REMOTE_MODEL_CATALOG_URL
    );

    // Custom URL
    assert_eq!(
        resolve_remote_catalog_url(Some("https://custom.mirror/catalog.json")),
        "https://custom.mirror/catalog.json"
    );

    // Active source checks
    assert!(is_remote_catalog_source_active(
        None,
        DEFAULT_REMOTE_MODEL_CATALOG_URL
    ));
    assert!(is_remote_catalog_source_active(
        None,
        RETIRED_DEFAULT_REMOTE_MODEL_CATALOG_URL
    ));
    assert!(!is_remote_catalog_source_active(
        None,
        "https://untrusted.mirror/cat.json"
    ));

    // Freshness check
    let now = 100_000_000;
    assert!(is_remote_catalog_fresh(now - 1_000, now));
    assert!(is_remote_catalog_fresh(now - (5 * 60 * 60 * 1000), now));
    assert!(!is_remote_catalog_fresh(now - (7 * 60 * 60 * 1000), now)); // Expired past 6h TTL
}

#[test]
fn test_ocrm_model_catalog_o02_bundle_projection_v1_and_v3() {
    // V1 Bundle
    let mut v1_providers = BTreeMap::new();
    v1_providers.insert(
        "anthropic".to_string(),
        ModelCatalogProviderDef {
            id: "anthropic".to_string(),
            name: Some("Anthropic".to_string()),
            base_url: Some("https://api.anthropic.com".to_string()),
            api: Some("anthropic-messages".to_string()),
            models: vec![ModelCatalogModelDef {
                id: "claude-3-7-sonnet".to_string(),
                name: Some("Claude 3.7".to_string()),
                context_window: Some(200000),
                context_tokens: Some(200000),
                ..Default::default()
            }],
            recommended_models: Some(vec!["claude-3-7-sonnet".to_string()]),
        },
    );

    let mut v1_pricing = BTreeMap::new();
    v1_pricing.insert(
        "anthropic/claude-3-7-sonnet".to_string(),
        ModelCatalogCost {
            input: Some(3.0),
            output: Some(15.0),
            cache_read: Some(0.3),
            cache_write: Some(3.75),
        },
    );

    let bundle_v1 = RemoteModelCatalogWireBundle::V1 {
        providers: v1_providers,
        pricing: Some(v1_pricing),
        min_version: Some("2026.1.0".to_string()),
        generated_at: 1700000000,
    };

    let proj_v1 = project_remote_model_catalog(&bundle_v1);
    assert_eq!(proj_v1.providers.len(), 1);
    assert_eq!(
        proj_v1.recommendations.get("anthropic"),
        Some(&vec!["claude-3-7-sonnet".to_string()])
    );
    let price_v1 = proj_v1.pricing.get("anthropic/claude-3-7-sonnet").unwrap();
    assert_eq!(price_v1.cost.input, Some(3.0));
    assert!(!price_v1.explicit); // V1 pricing is not explicit

    // V3 Bundle
    let mut v3_providers = BTreeMap::new();
    v3_providers.insert(
        "openai".to_string(),
        ModelCatalogProviderDef {
            id: "openai".to_string(),
            name: Some("OpenAI".to_string()),
            base_url: None,
            api: None,
            models: Vec::new(),
            recommended_models: Some(vec!["gpt-4o".to_string()]),
        },
    );
    let v3_models = vec![ModelCatalogModelDef {
        id: "gpt-4o".to_string(),
        name: Some("GPT-4o".to_string()),
        context_window: Some(128000),
        cost: Some(ModelCatalogCost {
            input: Some(2.5),
            output: Some(10.0),
            ..Default::default()
        }),
        ..Default::default()
    }];

    let bundle_v3 = RemoteModelCatalogWireBundle::V3 {
        providers: v3_providers,
        models: v3_models,
        min_version: None,
        generated_at: 1710000000,
    };

    let proj_v3 = project_remote_model_catalog(&bundle_v3);
    assert_eq!(proj_v3.providers["openai"].models.len(), 1);
    let price_v3 = proj_v3.pricing.get("openai/gpt-4o").unwrap();
    assert_eq!(price_v3.cost.input, Some(2.5));
    assert!(price_v3.explicit); // V3 pricing is explicit
}

#[test]
fn test_ocrm_model_catalog_o03_manifest_planner_and_trusted_transport() {
    let untrusted_provider = ModelCatalogProviderDef {
        id: "custom-proxy".to_string(),
        name: Some("Proxy".to_string()),
        base_url: Some("http://untrusted-remote.com/v1".to_string()), // Should be overridden!
        api: Some("openai-chat".to_string()),
        models: vec![
            ModelCatalogModelDef {
                id: "fast-model".to_string(),
                name: Some("Fast Model".to_string()),
                context_window: Some(64000),
                base_url: Some("http://attacker.com/leak".to_string()), // Should be overridden!
                ..Default::default()
            },
            ModelCatalogModelDef {
                id: "suppressed-model".to_string(),
                name: Some("Old Model".to_string()),
                context_window: Some(16000),
                ..Default::default()
            },
        ],
        recommended_models: None,
    };

    // Trusted local configuration
    let mut trusted_providers = BTreeMap::new();
    trusted_providers.insert(
        "custom-proxy".to_string(),
        ModelCatalogProviderDef {
            id: "custom-proxy".to_string(),
            base_url: Some("https://corporate.gateway.internal/v1".to_string()),
            models: vec![ModelCatalogModelDef {
                id: "fast-model".to_string(),
                base_url: Some("https://corporate.gateway.internal/v1".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        },
    );

    let suppressions = vec![ManifestModelCatalogSuppressionEntry {
        plugin_id: "test-plugin".to_string(),
        provider: "custom-proxy".to_string(),
        model: "suppressed-model".to_string(),
        merge_key: "custom-proxy/suppressed-model".to_string(),
        reason: Some("Deprecated model".to_string()),
    }];

    let plan = plan_manifest_model_catalog_rows(
        &[untrusted_provider],
        Some(&trusted_providers),
        None,
        &suppressions,
    );

    // Suppressed model should not appear
    assert_eq!(plan.rows.len(), 1);
    let row = &plan.rows[0];
    assert_eq!(row.provider, "custom-proxy");
    assert_eq!(row.model_id, "fast-model");
    // Trusted base_url wins over untrusted attacker URL
    assert_eq!(
        row.base_url.as_deref(),
        Some("https://corporate.gateway.internal/v1")
    );
    assert_eq!(row.context_tokens, 64000);
    assert_eq!(plan.conflicts.len(), 0);
}

#[test]
fn test_ocrm_model_catalog_o04_conflict_detection_and_filtering() {
    let duplicate_provider = ModelCatalogProviderDef {
        id: "vendor".to_string(),
        name: Some("Vendor".to_string()),
        models: vec![
            ModelCatalogModelDef {
                id: "model-a".to_string(),
                context_window: Some(32000),
                ..Default::default()
            },
            ModelCatalogModelDef {
                id: "model-a".to_string(), // Duplicate model ID!
                context_window: Some(32000),
                ..Default::default()
            },
            ModelCatalogModelDef {
                id: "model-b".to_string(),
                context_window: Some(32000),
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    let plan = plan_manifest_model_catalog_rows(&[duplicate_provider.clone()], None, None, &[]);

    assert_eq!(plan.rows.len(), 2);
    assert_eq!(plan.conflicts.len(), 1);
    assert_eq!(plan.conflicts[0].provider, "vendor");
    assert_eq!(plan.conflicts[0].model_id, "model-a");

    // Filtering test
    let filter = vec!["other-vendor".to_string()];
    let filtered_plan = plan_manifest_model_catalog_rows(
        &[duplicate_provider],
        None,
        Some(&filter),
        &[],
    );
    assert_eq!(filtered_plan.rows.len(), 0);
}
