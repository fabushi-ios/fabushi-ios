use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use super::cloud_agent_launch_error::SandCloudAgentLaunchError;
use super::cloud_agent_request_composition::SavedEnvironment;
use super::model_catalog_fetch::SandModelCatalogEntry;

pub const CLOUD_AGENT_MAX_WAIT_MS: u64 = 5 * 60 * 60_000;
pub const CLOUD_AGENT_POLL_INTERVAL_MS: u64 = 10_000;
pub const CLOUD_AGENT_POLL_RPC_TIMEOUT_MS: u64 = 30_000;
pub const CLOUD_AGENT_RATE_LIMIT_FALLBACK_MS: u64 = 60_000;
pub const CLOUD_AGENT_RATE_LIMIT_JITTER_RATIO: f64 = 0.25;
pub const CLOUD_AGENT_RUN_RESTART_GRACE_MS: u64 = 3 * 60_000;
pub const SAVED_ENVIRONMENT_LIST_LIMIT: usize = 500;
pub const MAX_LISTED_SAVED_ENVIRONMENTS: usize = 25;
pub const MODEL_CATALOG_TTL_MS: u64 = 5 * 60_000;
pub const TEAM_ADMIN_POLICY_TTL_MS: u64 = 5 * 60_000;
pub const TEAM_ADMIN_POLICY_REQUEST_TIMEOUT_MS: u64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudAgentRunStatus {
    Unspecified,
    Creating,
    Running,
    Finished,
    Error,
    Expired,
    Number(i64),
}

impl CloudAgentRunStatus {
    pub fn normalized(self) -> Self {
        match self {
            Self::Number(1) => Self::Running,
            Self::Number(2) => Self::Finished,
            Self::Number(3) => Self::Error,
            Self::Number(4) => Self::Creating,
            Self::Number(5) => Self::Expired,
            Self::Number(_) => Self::Unspecified,
            other => other,
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self.normalized(), Self::Running | Self::Creating)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CloudAgentComposer {
    pub status: Option<CloudAgentRunStatus>,
    pub pr_url: Option<String>,
    pub branch_name: Option<String>,
    pub commit_count: Option<u64>,
    pub files_changed: Option<u64>,
    pub lines_added: Option<u64>,
    pub lines_removed: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CloudAgentPr {
    pub pr_url: Option<String>,
    pub branch_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CloudAgentPermanentErrorDetails {
    pub title: String,
    pub detail: String,
    pub rate_limit_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DetailedComposer {
    pub composer: Option<CloudAgentComposer>,
    pub prs: Vec<CloudAgentPr>,
    pub summary: Option<String>,
    pub permanent_error: Option<CloudAgentPermanentErrorDetails>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudAgentWatchResult {
    pub status: &'static str,
    pub text: String,
}

pub fn cloud_agent_rate_limit_pause_ms(
    retry_after_ms: Option<u64>,
    random_fraction: f64,
) -> u64 {
    let retry_after = retry_after_ms.unwrap_or(CLOUD_AGENT_RATE_LIMIT_FALLBACK_MS);
    let random = if random_fraction.is_finite() {
        random_fraction.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let jitter = (retry_after as f64 * random * CLOUD_AGENT_RATE_LIMIT_JITTER_RATIO).round();
    retry_after.saturating_add(jitter.max(0.0) as u64)
}

pub fn available_environments_note(environments: &[SavedEnvironment]) -> String {
    if environments.is_empty() {
        return " This account has no saved environments; create one from the Cloud Agents dashboard on cursor.com.".into();
    }
    let listed = environments
        .iter()
        .take(MAX_LISTED_SAVED_ENVIRONMENTS)
        .map(|environment| {
            let name = environment.name.trim();
            if name.is_empty() {
                environment.public_id.clone()
            } else {
                format!("{name} ({})", environment.public_id)
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let hidden = environments.len().saturating_sub(MAX_LISTED_SAVED_ENVIRONMENTS);
    if hidden > 0 {
        format!(
            " Available: {listed} (+{hidden} more; set environment.id to launch one that isn't listed)."
        )
    } else {
        format!(" Available: {listed}.")
    }
}

pub fn resolve_sand_limit_error(detailed: Option<&DetailedComposer>) -> Option<String> {
    let details = detailed?.permanent_error.as_ref()?;
    if details.rate_limit_reason.as_deref() != Some("sand_included_limit") {
        return None;
    }
    let values = [details.title.trim(), details.detail.trim()]
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.join("\n"))
}

pub fn format_diff_stats(detailed: Option<&DetailedComposer>) -> Option<String> {
    let composer = detailed?.composer.as_ref()?;
    let mut parts = Vec::new();
    let commits = composer.commit_count.unwrap_or(0);
    if commits > 0 {
        parts.push(format!(
            "{commits} commit{}",
            if commits == 1 { "" } else { "s" }
        ));
    }
    let files = composer.files_changed.unwrap_or(0);
    if files > 0 {
        parts.push(format!(
            "+{}/-{} across {files} file{}",
            composer.lines_added.unwrap_or(0),
            composer.lines_removed.unwrap_or(0),
            if files == 1 { "" } else { "s" }
        ));
    }
    (!parts.is_empty()).then(|| format!("Changes: {}.", parts.join(", ")))
}

pub fn build_watch_result(
    bc_id: &str,
    status: CloudAgentRunStatus,
    detailed: Option<&DetailedComposer>,
) -> CloudAgentWatchResult {
    let normalized = status.normalized();
    let composer = detailed.and_then(|value| value.composer.as_ref());
    let summary = detailed
        .and_then(|value| value.summary.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let pr_url = composer
        .and_then(|value| value.pr_url.as_deref())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            detailed.and_then(|value| {
                value.prs.iter().find_map(|pr| {
                    pr.pr_url.as_deref().filter(|candidate| !candidate.is_empty())
                })
            })
        })
        .unwrap_or_default();

    if matches!(normalized, CloudAgentRunStatus::Error | CloudAgentRunStatus::Expired) {
        let mut lines = vec![format!(
            "The Cursor agent ({bc_id}) {} before finishing.",
            if normalized == CloudAgentRunStatus::Expired {
                "expired"
            } else {
                "errored"
            }
        )];
        if let Some(limit) = resolve_sand_limit_error(detailed) {
            lines.push(String::new());
            lines.push(limit);
        }
        if let Some(summary) = summary {
            lines.push(String::new());
            lines.push(summary.to_string());
        }
        return CloudAgentWatchResult {
            status: "error",
            text: lines.join("\n"),
        };
    }

    let mut lines = vec!["The Cursor agent finished.".to_string()];
    if let Some(branch) = composer
        .and_then(|value| value.branch_name.as_deref())
        .filter(|value| !value.is_empty())
    {
        lines.push(format!("Branch: {branch}"));
    }
    if pr_url.is_empty() {
        lines.push("No pull request link is available yet (the agent may have made no changes, or the PR is still being created).".into());
    } else {
        lines.push(format!("Pull request: {pr_url}"));
    }
    if let Some(stats) = format_diff_stats(detailed) {
        lines.push(stats);
    }
    if let Some(summary) = summary {
        lines.push(String::new());
        lines.push("Summary from the Cursor agent:".into());
        lines.push(summary.to_string());
    }
    CloudAgentWatchResult {
        status: "completed",
        text: lines.join("\n"),
    }
}

pub trait SavedEnvironmentClient: Send + Sync {
    fn get_environment(&self, public_id: &str) -> Result<Option<SavedEnvironment>, String>;
    fn list_environments(&self, limit: usize) -> Result<Vec<SavedEnvironment>, String>;
}

pub fn resolve_saved_environment(
    client: &dyn SavedEnvironmentClient,
    public_id: Option<&str>,
    name: Option<&str>,
) -> Result<SavedEnvironment, SandCloudAgentLaunchError> {
    let id = public_id.unwrap_or_default().trim();
    let name = name.unwrap_or_default().trim();

    if !id.is_empty() {
        if let Some(value) = client
            .get_environment(id)
            .map_err(SandCloudAgentLaunchError::new)?
        {
            return Ok(value);
        }
        let values = client
            .list_environments(SAVED_ENVIRONMENT_LIST_LIMIT)
            .map_err(SandCloudAgentLaunchError::new)?;
        return Err(SandCloudAgentLaunchError::new(format!(
            "No saved environment with id '{id}'.{}",
            available_environments_note(&values)
        )));
    }

    if name.is_empty() {
        return Err(SandCloudAgentLaunchError::new(
            "A saved-environment launch requires the environment's public id or name.",
        ));
    }

    let values = client
        .list_environments(SAVED_ENVIRONMENT_LIST_LIMIT)
        .map_err(SandCloudAgentLaunchError::new)?;
    let matches = values
        .iter()
        .filter(|value| value.name.trim().eq_ignore_ascii_case(name))
        .cloned()
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [value] => Ok(value.clone()),
        [] => Err(SandCloudAgentLaunchError::new(format!(
            "No saved environment named '{name}'.{}",
            available_environments_note(&values)
        ))),
        values => Err(SandCloudAgentLaunchError::new(format!(
            "Multiple saved environments are named '{name}'. Set environment.id to one of: {}.",
            values
                .iter()
                .map(|value| value.public_id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

pub type ModelCatalogLoader =
    Arc<dyn Fn() -> Result<Vec<SandModelCatalogEntry>, String> + Send + Sync>;

pub struct CloudAgentModelCatalogCache {
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    load: ModelCatalogLoader,
    cache: Mutex<Option<(Vec<SandModelCatalogEntry>, u64)>>,
}

impl CloudAgentModelCatalogCache {
    pub fn new(
        clock: Arc<dyn Fn() -> u64 + Send + Sync>,
        load: ModelCatalogLoader,
    ) -> Self {
        Self {
            clock,
            load,
            cache: Mutex::new(None),
        }
    }

    pub fn list_models(&self) -> Result<Vec<SandModelCatalogEntry>, String> {
        let now = (self.clock)();
        if let Some((catalog, fetched_at)) = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
        {
            if now.saturating_sub(*fetched_at) < MODEL_CATALOG_TTL_MS {
                return Ok(catalog.clone());
            }
        }
        let catalog = (self.load)()?;
        *self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some((catalog.clone(), now));
        Ok(catalog)
    }
}

pub type TeamAdminPolicyLoader = Arc<dyn Fn() -> Result<bool, String> + Send + Sync>;

struct TeamAdminState {
    cached: Option<(bool, u64)>,
}

pub struct CloudAgentTeamAdminPolicyCache {
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    load: TeamAdminPolicyLoader,
    state: Arc<Mutex<TeamAdminState>>,
    refreshing: Arc<AtomicBool>,
}

impl CloudAgentTeamAdminPolicyCache {
    pub fn new(
        clock: Arc<dyn Fn() -> u64 + Send + Sync>,
        load: TeamAdminPolicyLoader,
    ) -> Self {
        Self {
            clock,
            load,
            state: Arc::new(Mutex::new(TeamAdminState { cached: None })),
            refreshing: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn is_disabled_by_team_admin(&self) -> bool {
        // Match the frozen Promise semantics: callers observe the cache snapshot that
        // existed before this call scheduled a background refresh. Even a loader that
        // completes immediately must not turn the synchronous fail-open read into a
        // blocking/fail-closed result.
        let disabled = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .cached
            .map(|(disabled, _)| disabled)
            .unwrap_or(false);
        self.refresh();
        disabled
    }

    pub fn prefetch_team_admin_policy(&self) {
        self.refresh();
    }

    fn refresh(&self) {
        let now = (self.clock)();
        {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some((_, fetched_at)) = state.cached {
                if now.saturating_sub(fetched_at) < TEAM_ADMIN_POLICY_TTL_MS {
                    return;
                }
            }
        }
        if self.refreshing.swap(true, Ordering::AcqRel) {
            return;
        }

        let state = Arc::clone(&self.state);
        let refreshing = Arc::clone(&self.refreshing);
        let load = Arc::clone(&self.load);
        let clock = Arc::clone(&self.clock);
        let _ = std::thread::Builder::new()
            .name("cloud-agent-team-admin-policy".into())
            .spawn(move || {
                let previous = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .cached
                    .map(|(disabled, _)| disabled)
                    .unwrap_or(false);
                let disabled = load().unwrap_or(previous);
                let fetched_at = clock();
                state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .cached = Some((disabled, fetched_at));
                refreshing.store(false, Ordering::Release);
            });
    }

    pub fn wait_for_refresh_for_testing(&self) {
        while self.refreshing.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudAgentPollError {
    pub message: String,
    pub is_rate_limit: bool,
    pub retry_after_ms: Option<u64>,
}

pub type CloudAgentInfoFetcher =
    Arc<dyn Fn(&str, u64) -> Result<DetailedComposer, CloudAgentPollError> + Send + Sync>;

pub struct CloudAgentCompletionPoller {
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    sleep: Arc<dyn Fn(Duration) + Send + Sync>,
    get_info: CloudAgentInfoFetcher,
    max_wait_ms: u64,
    poll_interval_ms: u64,
    restart_grace_ms: u64,
    random: Arc<dyn Fn() -> f64 + Send + Sync>,
    disposed: AtomicBool,
}

impl CloudAgentCompletionPoller {
    pub fn new(
        clock: Arc<dyn Fn() -> u64 + Send + Sync>,
        sleep: Arc<dyn Fn(Duration) + Send + Sync>,
        get_info: CloudAgentInfoFetcher,
    ) -> Self {
        Self {
            clock,
            sleep,
            get_info,
            max_wait_ms: CLOUD_AGENT_MAX_WAIT_MS,
            poll_interval_ms: CLOUD_AGENT_POLL_INTERVAL_MS,
            restart_grace_ms: CLOUD_AGENT_RUN_RESTART_GRACE_MS,
            random: Arc::new(rand_fraction_from_time),
            disposed: AtomicBool::new(false),
        }
    }

    pub fn with_limits(
        mut self,
        max_wait_ms: u64,
        poll_interval_ms: u64,
        restart_grace_ms: u64,
    ) -> Self {
        self.max_wait_ms = max_wait_ms;
        self.poll_interval_ms = poll_interval_ms;
        self.restart_grace_ms = restart_grace_ms;
        self
    }

    pub fn with_random(mut self, random: Arc<dyn Fn() -> f64 + Send + Sync>) -> Self {
        self.random = random;
        self
    }

    pub fn dispose(&self) {
        self.disposed.store(true, Ordering::Release);
    }

    fn timeout_result(&self, bc_id: &str) -> CloudAgentWatchResult {
        CloudAgentWatchResult {
            status: "error",
            text: format!(
                "The Cursor agent ({bc_id}) is still running after {} minutes. It keeps running on the VM; check the Cursor cloud agents dashboard for the result.",
                (self.max_wait_ms as f64 / 60_000.0).round() as u64
            ),
        }
    }

    pub fn await_completion(
        &self,
        bc_id: &str,
        wait_for_restart: bool,
    ) -> CloudAgentWatchResult {
        let started = (self.clock)();
        let deadline = started.saturating_add(self.max_wait_ms);
        let restart_deadline = started.saturating_add(self.restart_grace_ms);
        let mut awaiting_restart = wait_for_restart;
        let mut paused_until = 0_u64;

        loop {
            if self.disposed.load(Ordering::Acquire) {
                return CloudAgentWatchResult {
                    status: "error",
                    text: format!(
                        "The Cursor agent ({bc_id}) status check stopped before completion."
                    ),
                };
            }

            (self.sleep)(Duration::from_millis(self.poll_interval_ms));
            let now = (self.clock)();
            if now >= deadline {
                return self.timeout_result(bc_id);
            }
            if now < paused_until {
                continue;
            }

            let detailed = match (self.get_info)(bc_id, CLOUD_AGENT_POLL_RPC_TIMEOUT_MS) {
                Ok(value) => value,
                Err(error) => {
                    if error.is_rate_limit {
                        paused_until = now.saturating_add(cloud_agent_rate_limit_pause_ms(
                            error.retry_after_ms,
                            (self.random)(),
                        ));
                    }
                    continue;
                }
            };
            let status = detailed
                .composer
                .as_ref()
                .and_then(|value| value.status)
                .unwrap_or(CloudAgentRunStatus::Unspecified)
                .normalized();
            let active = status.is_active();

            if awaiting_restart {
                if active || now >= restart_deadline {
                    awaiting_restart = false;
                } else {
                    continue;
                }
            }

            if active || status == CloudAgentRunStatus::Unspecified {
                continue;
            }
            return build_watch_result(bc_id, status, Some(&detailed));
        }
    }
}

fn rand_fraction_from_time() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    f64::from(nanos) / f64::from(u32::MAX)
}
