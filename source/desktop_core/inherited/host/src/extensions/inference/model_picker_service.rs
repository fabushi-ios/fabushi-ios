//! Model Picker Capability Profile & Authoritative Session Model Selection Service
//!
//! Mirrors upstream OpenClaw `src/model-picker/`:
//! - `capabilities.ts`: `ModelPickerCapabilityProfile` for presentation, callback bounds, and response modes.
//! - `apply-session-model-selection.ts`: Authoritative session model selection resolution,
//!   operator policy enforcement, visibility checks, active placement execution-mode fences,
//!   session mutation lifecycle, conflict detection, and switch event formatting.

use serde::{Deserialize, Serialize};

pub const MODEL_SELECTION_LOCKED_MESSAGE: &str = "Model selection is locked for this session.";
pub const SESSION_PLACEMENT_CHANGED_MESSAGE: &str = "Session placement changed; retry model selection.";
pub const SESSION_CONFLICT_MESSAGE: &str = "Model change was not applied because the session changed. Retry.";
pub const MODEL_NOT_IN_CATALOG_MESSAGE: &str =
    "Model selection is not present in the canonical model catalog.";

/// Unit for callback bounds in channel presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CallbackLimitUnit {
    Utf8Bytes,
    Utf16Units,
}

/// Callback profile constraints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelPickerCallbackLimits {
    pub limit: usize,
    pub unit: CallbackLimitUnit,
}

/// Response capabilities for picker interactions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPickerResponseCapabilities {
    pub supports_ephemeral: bool,
    pub supports_edit: bool,
    pub supports_replace: bool,
}

/// Presentation capabilities for picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ChannelPresentationCapabilities {
    pub supports_markdown: bool,
    pub max_text_length: Option<usize>,
}

/// Upstream `ModelPickerCapabilityProfile` from `capabilities.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPickerCapabilityProfile {
    pub presentation: ChannelPresentationCapabilities,
    pub callback: ModelPickerCallbackLimits,
    pub response: ModelPickerResponseCapabilities,
}

impl Default for ModelPickerCapabilityProfile {
    fn default() -> Self {
        Self {
            presentation: ChannelPresentationCapabilities {
                supports_markdown: true,
                max_text_length: Some(4096),
            },
            callback: ModelPickerCallbackLimits {
                limit: 1024,
                unit: CallbackLimitUnit::Utf8Bytes,
            },
            response: ModelPickerResponseCapabilities {
                supports_ephemeral: true,
                supports_edit: true,
                supports_replace: true,
            },
        }
    }
}

/// Runtime selection directive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SessionModelSelectionRuntime {
    Unchanged,
    Clear,
    Set { runtime: String },
}

/// Upstream `SessionModelSelectionRequest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionModelSelectionRequest {
    pub provider: String,
    pub model: String,
    pub is_default: bool,
    #[serde(default)]
    pub reset_to_default: bool,
    pub alias: Option<String>,
    pub profile_override: Option<String>,
    pub runtime: SessionModelSelectionRuntime,
}

/// Model catalog entry reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalogEntry {
    pub provider: String,
    pub id: String,
    pub name: Option<String>,
    pub context_window: Option<u64>,
    pub context_tokens: Option<u64>,
    pub api: Option<String>,
    pub base_url: Option<String>,
    pub reasoning: Option<bool>,
}

/// Model visibility policy for agents.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelVisibilityPolicy {
    pub allow: Vec<String>,
}

impl ModelVisibilityPolicy {
    pub fn new(allow: Vec<String>) -> Self {
        Self { allow }
    }

    pub fn allows(&self, provider: &str, model: &str) -> bool {
        if self.allow.is_empty() {
            return true;
        }
        let qualified = format!("{provider}/{model}");
        let wildcard_provider = format!("{provider}/*");
        self.allow.iter().any(|pattern| {
            pattern == "*" || pattern == &qualified || pattern == &wildcard_provider || pattern == model
        })
    }
}

/// Operator authority context.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdmittedRunOperatorAuthority {
    pub profile_id: String,
    pub scopes: Vec<String>,
    pub allowed_models: Option<Vec<String>>,
}

impl AdmittedRunOperatorAuthority {
    pub fn allows(&self, provider: &str, model: &str) -> bool {
        match &self.allowed_models {
            None => true,
            Some(allowed) => {
                if allowed.is_empty() {
                    return false;
                }
                let qualified = format!("{provider}/{model}");
                let wildcard_provider = format!("{provider}/*");
                allowed.iter().any(|p| {
                    p == "*" || p == &qualified || p == &wildcard_provider || p == model
                })
            }
        }
    }
}

/// Active cloud-worker placement context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerPlacementContext {
    pub session_id: String,
    pub state: String, // "local" | "active" | ...
    pub execution_mode: Option<String>,
    pub generation: u64,
}

/// Authoritative session entry state for model selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionModelEntryState {
    pub session_id: String,
    pub provider_override: Option<String>,
    pub model_override: Option<String>,
    pub auth_profile_override: Option<String>,
    pub live_model_switch_pending: bool,
    pub is_locked: bool,
    pub thinking_level: Option<String>,
    pub updated_at: u64,
}

/// Rejection reasons for model selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelPickerRejectReason {
    Locked,
    NotAllowed,
    InvalidRuntime,
    UnknownProvider,
}

/// Outcome of applying session model selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPickerAppliedOutcome {
    pub provider: String,
    pub model: String,
    pub effective_model_ref: String,
    pub agent_runtime: String,
    pub changed: bool,
    pub context_tokens: u64,
    pub runtime_change: Option<SessionModelSelectionRuntime>,
}

/// Result of model picker application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum ApplySessionModelSelectionResult {
    Applied(ModelPickerAppliedOutcome),
    Rejected {
        reason: ModelPickerRejectReason,
        message: String,
    },
    Conflict {
        message: String,
    },
}

/// Formats a model switch notification message.
pub fn format_model_switch_event(provider: &str, model: &str, alias: Option<&str>) -> String {
    let label = format!("{provider}/{model}");
    match alias {
        Some(a) => format!("Model switched to {a} ({label})."),
        None => format!("Model switched to {label}."),
    }
}

/// Normalizes model key from provider and model id.
pub fn model_key(provider: &str, model: &str) -> String {
    format!("{}/{}", provider.trim().to_lowercase(), model.trim().to_lowercase())
}

/// Resolves context tokens given catalog entry or fallback.
pub fn resolve_model_context_tokens(entry: Option<&ModelCatalogEntry>) -> u64 {
    entry
        .and_then(|e| e.context_tokens.or(e.context_window))
        .unwrap_or(8192)
}

/// Evaluates active placement compatibility.
/// Rejects when cloud-worker placement is active and runtime execution mode is incompatible.
pub fn resolve_active_placement_model_selection_error(
    placement: Option<&WorkerPlacementContext>,
    candidate_execution_mode: Option<&str>,
) -> Option<String> {
    let placement = placement?;
    if placement.state == "local" {
        return None;
    }
    match (placement.execution_mode.as_deref(), candidate_execution_mode) {
        (Some(current), Some(candidate)) if current == candidate => None,
        (Some(_), Some(_)) => Some(format!(
            "Session cannot change cloud placement execution mode while placement is {}.",
            placement.state
        )),
        (Some(_), None) => Some(format!(
            "Session cannot select a runtime without cloud placement support while cloud worker placement is {}.",
            placement.state
        )),
        _ => None,
    }
}

/// Parameters for applying session model selection.
pub struct ApplySessionModelSelectionParams<'a> {
    pub agent_id: &'a str,
    pub session_key: &'a str,
    pub default_provider: &'a str,
    pub default_model: &'a str,
    pub current_provider: &'a str,
    pub current_model: &'a str,
    pub model_catalog: &'a [ModelCatalogEntry],
    pub visibility_policy: Option<&'a ModelVisibilityPolicy>,
    pub operator_authority: Option<&'a AdmittedRunOperatorAuthority>,
    pub placement_context: Option<&'a WorkerPlacementContext>,
    pub candidate_execution_mode: Option<&'a str>,
    pub initial_generation: Option<u64>,
    pub current_generation: Option<u64>,
    pub request: SessionModelSelectionRequest,
}

/// Core authoritative session model selection evaluation and transition.
pub fn apply_session_model_selection(
    entry: &mut SessionModelEntryState,
    params: ApplySessionModelSelectionParams<'_>,
    current_time_ms: u64,
) -> ApplySessionModelSelectionResult {
    // 1. Check if model selection is locked
    if entry.is_locked {
        return ApplySessionModelSelectionResult::Rejected {
            reason: ModelPickerRejectReason::Locked,
            message: MODEL_SELECTION_LOCKED_MESSAGE.to_string(),
        };
    }

    // 2. Concurrency / generation fence check
    if let (Some(init_gen), Some(curr_gen)) = (params.initial_generation, params.current_generation) {
        if init_gen != curr_gen {
            return ApplySessionModelSelectionResult::Conflict {
                message: SESSION_CONFLICT_MESSAGE.to_string(),
            };
        }
    }

    let reset_to_default = params.request.reset_to_default;
    let selected_provider = if reset_to_default {
        params.default_provider
    } else {
        &params.request.provider
    };
    let selected_model = if reset_to_default {
        params.default_model
    } else {
        &params.request.model
    };

    // 3. Operator authority enforcement
    if let Some(operator) = params.operator_authority {
        if !operator.allows(selected_provider, selected_model) {
            return ApplySessionModelSelectionResult::Rejected {
                reason: ModelPickerRejectReason::NotAllowed,
                message: format!(
                    "Model {selected_provider}/{selected_model} is not available for this operator role and agent."
                ),
            };
        }
    }

    // 4. Model visibility policy enforcement
    if !reset_to_default {
        if let Some(policy) = params.visibility_policy {
            if !policy.allows(selected_provider, selected_model) {
                return ApplySessionModelSelectionResult::Rejected {
                    reason: ModelPickerRejectReason::NotAllowed,
                    message: format!(
                        "Model {selected_provider}/{selected_model} is not available for this agent."
                    ),
                };
            }
        }
    }

    // 5. Active worker placement fence check
    if let Some(placement_err) = resolve_active_placement_model_selection_error(
        params.placement_context,
        params.candidate_execution_mode,
    ) {
        return ApplySessionModelSelectionResult::Rejected {
            reason: ModelPickerRejectReason::InvalidRuntime,
            message: placement_err,
        };
    }

    // 6. Resolve catalog entry & context tokens
    let catalog_match = params.model_catalog.iter().find(|e| {
        e.provider.eq_ignore_ascii_case(selected_provider)
            && e.id.eq_ignore_ascii_case(selected_model)
    });
    if !reset_to_default && catalog_match.is_none() {
        return ApplySessionModelSelectionResult::Rejected {
            reason: ModelPickerRejectReason::UnknownProvider,
            message: MODEL_NOT_IN_CATALOG_MESSAGE.to_string(),
        };
    }
    let context_tokens = resolve_model_context_tokens(catalog_match);

    // 7. Check if selection changed
    let effective_ref = format!("{selected_provider}/{selected_model}");
    let current_ref = format!("{}/{}", params.current_provider, params.current_model);
    let is_default_selection = reset_to_default
        || params.request.is_default
        || (model_key(selected_provider, selected_model)
            == model_key(params.default_provider, params.default_model));

    let model_changed = if is_default_selection {
        entry.model_override.is_some() || entry.provider_override.is_some()
    } else {
        entry.model_override.as_deref() != Some(selected_model)
            || entry.provider_override.as_deref() != Some(selected_provider)
    };

    let runtime_changed = match &params.request.runtime {
        SessionModelSelectionRuntime::Unchanged => false,
        SessionModelSelectionRuntime::Clear | SessionModelSelectionRuntime::Set { .. } => true,
    };
    let selection_changed = model_changed || runtime_changed;

    // 8. Mutate session entry state
    if is_default_selection {
        entry.provider_override = None;
        entry.model_override = None;
    } else {
        entry.provider_override = Some(selected_provider.to_string());
        entry.model_override = Some(selected_model.to_string());
    }

    if let Some(profile) = &params.request.profile_override {
        entry.auth_profile_override = Some(profile.clone());
    }
    if selection_changed {
        entry.live_model_switch_pending = true;
    }
    entry.updated_at = current_time_ms;

    let agent_runtime = match &params.request.runtime {
        SessionModelSelectionRuntime::Set { runtime } => runtime.clone(),
        _ => "default".to_string(),
    };

    let runtime_change = match params.request.runtime {
        SessionModelSelectionRuntime::Unchanged => None,
        other => Some(other),
    };

    ApplySessionModelSelectionResult::Applied(ModelPickerAppliedOutcome {
        provider: selected_provider.to_string(),
        model: selected_model.to_string(),
        effective_model_ref: effective_ref,
        agent_runtime,
        changed: selection_changed,
        context_tokens,
        runtime_change,
    })
}
