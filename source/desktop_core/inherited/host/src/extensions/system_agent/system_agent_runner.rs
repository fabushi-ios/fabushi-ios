use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemAgentOperationKind {
    OnboardingSetup,
    OverviewInspect,
    DiagnosticAudit,
    CustomCommand(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemAgentSetupProposal {
    pub proposed_workspace: String,
    pub proposed_agent_name: String,
    pub welcome_variant: Option<String>,
}

#[derive(Clone, Default)]
pub struct SystemAgentRunner {
    verified_inference_ready: bool,
}

impl SystemAgentRunner {
    pub fn new(verified_inference_ready: bool) -> Self {
        Self {
            verified_inference_ready,
        }
    }

    pub fn parse_operation(&self, input: &str) -> SystemAgentOperationKind {
        let trimmed = input.trim().to_ascii_lowercase();
        if trimmed == "setup" || trimmed == "onboarding" || trimmed == "init" {
            SystemAgentOperationKind::OnboardingSetup
        } else if trimmed == "overview" || trimmed == "status" {
            SystemAgentOperationKind::OverviewInspect
        } else if trimmed == "doctor" || trimmed == "diag" || trimmed == "audit" {
            SystemAgentOperationKind::DiagnosticAudit
        } else {
            SystemAgentOperationKind::CustomCommand(input.trim().to_string())
        }
    }

    pub fn prepare_setup_proposal(
        &self,
        workspace_override: Option<&str>,
        agent_name_override: Option<&str>,
        is_onboarding: bool,
    ) -> Result<SystemAgentSetupProposal, String> {
        if !self.verified_inference_ready {
            return Err("System agent inference route unavailable".to_string());
        }

        let workspace = workspace_override
            .map(|s| s.to_string())
            .unwrap_or_else(|| "~/.openclaw/workspace".to_string());
        let agent_name = agent_name_override
            .map(|s| s.to_string())
            .unwrap_or_else(|| "default-agent".to_string());

        let welcome_variant = if is_onboarding {
            Some("onboarding".to_string())
        } else {
            None
        };

        Ok(SystemAgentSetupProposal {
            proposed_workspace: workspace,
            proposed_agent_name: agent_name,
            welcome_variant,
        })
    }
}
