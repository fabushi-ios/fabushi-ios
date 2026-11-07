//! OpenClaw Model Picker Contract Test Suite
//!
//! Verifies OCRM-RESP-MODEL-PICKER-001:
//! - Capability profiles & callback constraints
//! - Session model selection resolution & operator authority checks
//! - Model visibility policy evaluation
//! - Active cloud-worker placement execution mode compatibility fences
//! - Concurrency generation conflict detection
//! - Authoritative session entry state transition & switch event formatting

use mahayana_host_runtime::extensions::inference::model_picker_service::*;

#[test]
fn test_ocrm_model_picker_o01_capability_profile_and_event_formatting() {
    let profile = ModelPickerCapabilityProfile::default();
    assert!(profile.presentation.supports_markdown);
    assert_eq!(profile.presentation.max_text_length, Some(4096));
    assert_eq!(profile.callback.limit, 1024);
    assert_eq!(profile.callback.unit, CallbackLimitUnit::Utf8Bytes);
    assert!(profile.response.supports_ephemeral);
    assert!(profile.response.supports_edit);
    assert!(profile.response.supports_replace);

    // Event formatting
    let msg1 = format_model_switch_event("anthropic", "claude-3-7-sonnet", None);
    assert_eq!(msg1, "Model switched to anthropic/claude-3-7-sonnet.");

    let msg2 = format_model_switch_event("openai", "gpt-4o", Some("Fast GPT"));
    assert_eq!(msg2, "Model switched to Fast GPT (openai/gpt-4o).");

    assert_eq!(model_key("Anthropic", "Claude-3-7"), "anthropic/claude-3-7");
}

#[test]
fn test_ocrm_model_picker_o02_locked_and_conflict_rejections() {
    let mut locked_entry = SessionModelEntryState {
        session_id: "sess-1".to_string(),
        is_locked: true,
        ..Default::default()
    };

    let req = SessionModelSelectionRequest {
        provider: "openai".to_string(),
        model: "gpt-4o".to_string(),
        is_default: false,
        reset_to_default: false,
        alias: None,
        profile_override: None,
        runtime: SessionModelSelectionRuntime::Unchanged,
    };

    let params = ApplySessionModelSelectionParams {
        agent_id: "agent-main",
        session_key: "sess-key-1",
        default_provider: "anthropic",
        default_model: "claude-3-7-sonnet",
        current_provider: "anthropic",
        current_model: "claude-3-7-sonnet",
        model_catalog: &[],
        visibility_policy: None,
        operator_authority: None,
        placement_context: None,
        candidate_execution_mode: None,
        initial_generation: None,
        current_generation: None,
        request: req.clone(),
    };

    let res = apply_session_model_selection(&mut locked_entry, params, 1000);
    assert_eq!(
        res,
        ApplySessionModelSelectionResult::Rejected {
            reason: ModelPickerRejectReason::Locked,
            message: MODEL_SELECTION_LOCKED_MESSAGE.to_string(),
        }
    );

    // Generation conflict
    let mut active_entry = SessionModelEntryState {
        session_id: "sess-2".to_string(),
        is_locked: false,
        ..Default::default()
    };

    let conflict_params = ApplySessionModelSelectionParams {
        agent_id: "agent-main",
        session_key: "sess-key-2",
        default_provider: "anthropic",
        default_model: "claude-3-7-sonnet",
        current_provider: "anthropic",
        current_model: "claude-3-7-sonnet",
        model_catalog: &[],
        visibility_policy: None,
        operator_authority: None,
        placement_context: None,
        candidate_execution_mode: None,
        initial_generation: Some(1),
        current_generation: Some(2), // Mismatch!
        request: req,
    };

    let conflict_res = apply_session_model_selection(&mut active_entry, conflict_params, 1000);
    assert_eq!(
        conflict_res,
        ApplySessionModelSelectionResult::Conflict {
            message: SESSION_CONFLICT_MESSAGE.to_string(),
        }
    );
}

#[test]
fn test_ocrm_model_picker_o03_operator_and_visibility_policy() {
    let mut entry = SessionModelEntryState {
        session_id: "sess-policy".to_string(),
        ..Default::default()
    };

    let catalog = vec![
        ModelCatalogEntry {
            provider: "openai".to_string(),
            id: "gpt-4o".to_string(),
            name: Some("GPT-4o".to_string()),
            context_window: Some(128000),
            context_tokens: Some(128000),
            api: None,
            base_url: None,
            reasoning: None,
        },
        ModelCatalogEntry {
            provider: "anthropic".to_string(),
            id: "claude-3-7-sonnet".to_string(),
            name: Some("Sonnet 3.7".to_string()),
            context_window: Some(200000),
            context_tokens: Some(200000),
            api: None,
            base_url: None,
            reasoning: Some(true),
        },
    ];

    let visibility = ModelVisibilityPolicy::new(vec!["anthropic/*".to_string()]);
    let operator = AdmittedRunOperatorAuthority {
        profile_id: "operator-1".to_string(),
        scopes: vec!["operator.write".to_string()],
        allowed_models: Some(vec!["anthropic/claude-3-7-sonnet".to_string()]),
    };

    // Attempting openai/gpt-4o -> disallowed by operator
    let req_openai = SessionModelSelectionRequest {
        provider: "openai".to_string(),
        model: "gpt-4o".to_string(),
        is_default: false,
        reset_to_default: false,
        alias: None,
        profile_override: None,
        runtime: SessionModelSelectionRuntime::Unchanged,
    };

    let params_disallowed = ApplySessionModelSelectionParams {
        agent_id: "agent-main",
        session_key: "sess-key",
        default_provider: "anthropic",
        default_model: "claude-3-7-sonnet",
        current_provider: "anthropic",
        current_model: "claude-3-7-sonnet",
        model_catalog: &catalog,
        visibility_policy: Some(&visibility),
        operator_authority: Some(&operator),
        placement_context: None,
        candidate_execution_mode: None,
        initial_generation: None,
        current_generation: None,
        request: req_openai,
    };

    let res = apply_session_model_selection(&mut entry, params_disallowed, 1000);
    assert_eq!(
        res,
        ApplySessionModelSelectionResult::Rejected {
            reason: ModelPickerRejectReason::NotAllowed,
            message: "Model openai/gpt-4o is not available for this operator role and agent.".to_string(),
        }
    );

    // Operator allows, but visibility policy forbids
    let operator_broader = AdmittedRunOperatorAuthority {
        profile_id: "operator-2".to_string(),
        scopes: vec!["operator.write".to_string()],
        allowed_models: Some(vec!["*".to_string()]),
    };
    let req_google = SessionModelSelectionRequest {
        provider: "google".to_string(),
        model: "gemini-2.5-pro".to_string(),
        is_default: false,
        reset_to_default: false,
        alias: None,
        profile_override: None,
        runtime: SessionModelSelectionRuntime::Unchanged,
    };
    let params_visibility_rejected = ApplySessionModelSelectionParams {
        agent_id: "agent-main",
        session_key: "sess-key",
        default_provider: "anthropic",
        default_model: "claude-3-7-sonnet",
        current_provider: "anthropic",
        current_model: "claude-3-7-sonnet",
        model_catalog: &catalog,
        visibility_policy: Some(&visibility), // only anthropic/*
        operator_authority: Some(&operator_broader),
        placement_context: None,
        candidate_execution_mode: None,
        initial_generation: None,
        current_generation: None,
        request: req_google,
    };

    let res_vis = apply_session_model_selection(&mut entry, params_visibility_rejected, 1000);
    assert_eq!(
        res_vis,
        ApplySessionModelSelectionResult::Rejected {
            reason: ModelPickerRejectReason::NotAllowed,
            message: "Model google/gemini-2.5-pro is not available for this agent.".to_string(),
        }
    );
}

#[test]
fn test_ocrm_model_picker_rejects_non_default_model_absent_from_catalog() {
    let mut entry = SessionModelEntryState {
        session_id: "sess-catalog".to_string(),
        updated_at: 17,
        ..Default::default()
    };
    let before = entry.clone();
    let request = SessionModelSelectionRequest {
        provider: "unregistered-provider".to_string(),
        model: "unregistered-model".to_string(),
        is_default: false,
        reset_to_default: false,
        alias: None,
        profile_override: None,
        runtime: SessionModelSelectionRuntime::Unchanged,
    };
    let result = apply_session_model_selection(
        &mut entry,
        ApplySessionModelSelectionParams {
            agent_id: "agent-main",
            session_key: "agent-main:main",
            default_provider: "openai",
            default_model: "gpt-4o",
            current_provider: "openai",
            current_model: "gpt-4o",
            model_catalog: &[],
            visibility_policy: None,
            operator_authority: None,
            placement_context: None,
            candidate_execution_mode: None,
            initial_generation: None,
            current_generation: None,
            request,
        },
        99,
    );
    assert_eq!(
        result,
        ApplySessionModelSelectionResult::Rejected {
            reason: ModelPickerRejectReason::UnknownProvider,
            message: MODEL_NOT_IN_CATALOG_MESSAGE.to_string(),
        }
    );
    assert_eq!(entry, before);
}

#[test]
fn test_ocrm_model_picker_o04_placement_fences_and_successful_mutation() {
    let mut entry = SessionModelEntryState {
        session_id: "sess-placement".to_string(),
        provider_override: None,
        model_override: None,
        auth_profile_override: None,
        live_model_switch_pending: false,
        is_locked: false,
        thinking_level: None,
        updated_at: 100,
    };

    let catalog = vec![ModelCatalogEntry {
        provider: "anthropic".to_string(),
        id: "claude-3-7-sonnet".to_string(),
        name: Some("Sonnet 3.7".to_string()),
        context_window: Some(200000),
        context_tokens: Some(200000),
        api: None,
        base_url: None,
        reasoning: Some(true),
    }];

    let active_placement = WorkerPlacementContext {
        session_id: "sess-placement".to_string(),
        state: "active".to_string(),
        execution_mode: Some("worker-turn".to_string()),
        generation: 1,
    };

    let req = SessionModelSelectionRequest {
        provider: "anthropic".to_string(),
        model: "claude-3-7-sonnet".to_string(),
        is_default: false,
        reset_to_default: false,
        alias: Some("Sonnet".to_string()),
        profile_override: Some("prof-work".to_string()),
        runtime: SessionModelSelectionRuntime::Set {
            runtime: "worker-turn".to_string(),
        },
    };

    // Placement fence: candidate missing execution mode
    let params_incompatible = ApplySessionModelSelectionParams {
        agent_id: "agent-main",
        session_key: "sess-key",
        default_provider: "anthropic",
        default_model: "claude-3-5-haiku",
        current_provider: "anthropic",
        current_model: "claude-3-5-haiku",
        model_catalog: &catalog,
        visibility_policy: None,
        operator_authority: None,
        placement_context: Some(&active_placement),
        candidate_execution_mode: None, // Missing!
        initial_generation: None,
        current_generation: None,
        request: req.clone(),
    };

    let res = apply_session_model_selection(&mut entry, params_incompatible, 2000);
    assert_eq!(
        res,
        ApplySessionModelSelectionResult::Rejected {
            reason: ModelPickerRejectReason::InvalidRuntime,
            message: "Session cannot select a runtime without cloud placement support while cloud worker placement is active.".to_string(),
        }
    );

    // Placement fence passed with matching execution mode
    let params_valid = ApplySessionModelSelectionParams {
        agent_id: "agent-main",
        session_key: "sess-key",
        default_provider: "anthropic",
        default_model: "claude-3-5-haiku",
        current_provider: "anthropic",
        current_model: "claude-3-5-haiku",
        model_catalog: &catalog,
        visibility_policy: None,
        operator_authority: None,
        placement_context: Some(&active_placement),
        candidate_execution_mode: Some("worker-turn"),
        initial_generation: None,
        current_generation: None,
        request: req,
    };

    let outcome = apply_session_model_selection(&mut entry, params_valid, 2000);
    match outcome {
        ApplySessionModelSelectionResult::Applied(out) => {
            assert_eq!(out.provider, "anthropic");
            assert_eq!(out.model, "claude-3-7-sonnet");
            assert_eq!(out.effective_model_ref, "anthropic/claude-3-7-sonnet");
            assert_eq!(out.agent_runtime, "worker-turn");
            assert!(out.changed);
            assert_eq!(out.context_tokens, 200000);
        }
        _ => panic!("Expected Applied result"),
    }

    // Verify mutated session state
    assert_eq!(entry.provider_override.as_deref(), Some("anthropic"));
    assert_eq!(entry.model_override.as_deref(), Some("claude-3-7-sonnet"));
    assert_eq!(entry.auth_profile_override.as_deref(), Some("prof-work"));
    assert!(entry.live_model_switch_pending);
    assert_eq!(entry.updated_at, 2000);

    // Reset to default
    let reset_req = SessionModelSelectionRequest {
        provider: "anthropic".to_string(),
        model: "claude-3-7-sonnet".to_string(),
        is_default: true,
        reset_to_default: true,
        alias: None,
        profile_override: None,
        runtime: SessionModelSelectionRuntime::Clear,
    };
    let reset_params = ApplySessionModelSelectionParams {
        agent_id: "agent-main",
        session_key: "sess-key",
        default_provider: "anthropic",
        default_model: "claude-3-5-haiku",
        current_provider: "anthropic",
        current_model: "claude-3-7-sonnet",
        model_catalog: &catalog,
        visibility_policy: None,
        operator_authority: None,
        placement_context: None,
        candidate_execution_mode: None,
        initial_generation: None,
        current_generation: None,
        request: reset_req,
    };

    let reset_outcome = apply_session_model_selection(&mut entry, reset_params, 3000);
    match reset_outcome {
        ApplySessionModelSelectionResult::Applied(out) => {
            assert_eq!(out.provider, "anthropic");
            assert_eq!(out.model, "claude-3-5-haiku");
            assert_eq!(out.effective_model_ref, "anthropic/claude-3-5-haiku");
            assert!(out.changed);
        }
        _ => panic!("Expected Applied result on reset"),
    }

    assert_eq!(entry.provider_override, None);
    assert_eq!(entry.model_override, None);
    assert_eq!(entry.updated_at, 3000);
}
