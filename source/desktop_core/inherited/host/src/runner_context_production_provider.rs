use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

use crate::extensions::auth::extension::HostAuthExtension;
use crate::extensions::managed_setup::team_rules::ProductionTeamRulesResolver;
use crate::host_request_context::create_host_request_context;
use crate::runner::routed_provider_runtime::{
    RunnerRequestContextSnapshot, RunnerRequestContextSource,
};

/// Rust adaptation of Grok's production Runner-context provider.
///
/// Grok's Node implementation constructs a process-local Context carrying a
/// silent logger. Mahayana Runner does not carry the Node keyed-Context/logger
/// object across its process boundary; instead the shipping Host supplies an
/// immutable request-context snapshot. Logging remains process-local while
/// user identity, team rules, timezone and transcript location cross the
/// explicit Host -> Runner boundary here.
pub struct ProductionRunnerRequestContextSource {
    auth: Arc<HostAuthExtension>,
    team_rules: Arc<ProductionTeamRulesResolver>,
    transcripts_folder: PathBuf,
}

impl ProductionRunnerRequestContextSource {
    pub fn new(
        auth: Arc<HostAuthExtension>,
        team_rules: Arc<ProductionTeamRulesResolver>,
        transcripts_folder: PathBuf,
    ) -> Self {
        Self {
            auth,
            team_rules,
            transcripts_folder,
        }
    }
}

pub fn build_production_runner_request_context_snapshot(
    transcripts_folder: &Path,
    rules: Option<Vec<Value>>,
    user_full_name: Option<String>,
) -> RunnerRequestContextSnapshot {
    let provider = create_host_request_context(
        transcripts_folder.to_string_lossy().into_owned(),
        || None,
        || rules.clone(),
        || user_full_name.clone(),
    );
    RunnerRequestContextSnapshot {
        context: provider.resolve(),
        rules: provider.resolve_rules(),
    }
}

impl RunnerRequestContextSource for ProductionRunnerRequestContextSource {
    fn resolve(&self) -> RunnerRequestContextSnapshot {
        build_production_runner_request_context_snapshot(
            &self.transcripts_folder,
            self.team_rules.resolve_rules(),
            self.auth.get_user_full_name(),
        )
    }
}
