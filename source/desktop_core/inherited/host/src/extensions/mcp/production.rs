use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use prost::Message;

use crate::cursor_backend::{
    resolve_sand_ghost_mode_header, send_cursor_unary,
};
use crate::extensions::auth::credential_renewer::RenewalOutcome;
use crate::extensions::auth::extension::HostAuthExtension;

use super::extension::{
    AuthRenewalEvent, AuthRenewalPort, McpHostServicePort, McpPluginSkillsService,
    PollingHandle, PollingPort,
};
use super::mcp_service::{McpHostService, PluginSkillsPort};
use super::plugin_skills::{
    PLUGIN_SKILLS_REFRESH_INTERVAL_MS, SandPluginSkillsService,
};
use super::skill_publish::{
    PublishPluginInput, PublishedSkillResult, SandSkillPublishService,
    SkillPublishClient, SkillPublishTeam,
};

pub const LEGACY_MCP_AUTH_FILENAME: &str = "mcp-auth.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyMcpAuthCleanupOutcome {
    NotFound,
    Error,
    Deleted,
}

pub fn is_legacy_mcp_auth_file(name: &str) -> bool {
    name == LEGACY_MCP_AUTH_FILENAME || name.starts_with(&format!("{LEGACY_MCP_AUTH_FILENAME}."))
}

pub fn cleanup_legacy_mcp_auth_credentials(
    root_dir: &Path,
) -> (LegacyMcpAuthCleanupOutcome, usize) {
    let entries = match fs::read_dir(root_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (LegacyMcpAuthCleanupOutcome::NotFound, 0);
        }
        Err(_) => return (LegacyMcpAuthCleanupOutcome::Error, 0),
    };
    let mut removed = 0usize;
    let mut saw_error = false;
    for entry in entries {
        let Ok(entry) = entry else {
            saw_error = true;
            continue;
        };
        let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
            continue;
        };
        if !is_legacy_mcp_auth_file(&name) {
            continue;
        }
        match fs::remove_file(entry.path()) {
            Ok(()) => removed += 1,
            Err(_) => saw_error = true,
        }
    }
    if saw_error {
        (LegacyMcpAuthCleanupOutcome::Error, removed)
    } else if removed > 0 {
        (LegacyMcpAuthCleanupOutcome::Deleted, removed)
    } else {
        (LegacyMcpAuthCleanupOutcome::NotFound, 0)
    }
}

pub const DASHBOARD_GET_TEAMS_PATH: &str = "/aiserver.v1.DashboardService/GetTeams";
pub const DASHBOARD_PUBLISH_PLUGIN_PATH: &str =
    "/aiserver.v1.DashboardService/PublishPlugin";
pub const DASHBOARD_UNPUBLISH_PLUGIN_PATH: &str =
    "/aiserver.v1.DashboardService/UnpublishPlugin";

#[derive(Clone, PartialEq, Message)]
struct GetTeamsRequestWire {
    #[prost(bool, optional, tag = "1")]
    active_only: Option<bool>,
}
#[derive(Clone, PartialEq, Message)]
struct TeamWire {
    #[prost(string, tag = "1")]
    name: String,
    #[prost(int32, tag = "2")]
    id: i32,
    #[prost(bool, tag = "36")]
    is_direct_member: bool,
}
#[derive(Clone, PartialEq, Message)]
struct GetTeamsResponseWire {
    #[prost(message, repeated, tag = "1")]
    teams: Vec<TeamWire>,
}
#[derive(Clone, PartialEq, Message)]
struct PublishPluginRequestWire {
    #[prost(int32, tag = "1")]
    team_id: i32,
    #[prost(string, tag = "2")]
    name: String,
    #[prost(string, tag = "3")]
    display_name: String,
    #[prost(string, tag = "4")]
    description: String,
    #[prost(bytes = "vec", tag = "5")]
    plugin_tar_gz: Vec<u8>,
}
#[derive(Clone, PartialEq, Message)]
struct PublishPluginResponseWire {
    #[prost(int64, tag = "1")]
    plugin_id: i64,
    #[prost(int64, tag = "2")]
    _marketplace_id: i64,
    #[prost(string, tag = "3")]
    commit_sha: String,
}
#[derive(Clone, PartialEq, Message)]
struct UnpublishPluginRequestWire {
    #[prost(int64, tag = "1")]
    plugin_id: i64,
    #[prost(int32, optional, tag = "2")]
    team_id: Option<i32>,
}
#[derive(Clone, PartialEq, Message)]
struct UnpublishPluginResponseWire {
    #[prost(string, tag = "1")]
    _commit_sha: String,
}

#[derive(Clone)]
pub struct CursorSkillPublishClient {
    backend_url: String,
    auth: Arc<HostAuthExtension>,
}
impl CursorSkillPublishClient {
    pub fn new(backend_url: impl Into<String>, auth: Arc<HostAuthExtension>) -> Self {
        Self { backend_url: backend_url.into(), auth }
    }

    fn unary<Req, Resp>(
        &self,
        path: &str,
        request: &Req,
        timeout_ms: u64,
    ) -> Result<Resp, String>
    where
        Req: Message,
        Resp: Message + Default,
    {
        let access_token = self.auth.get_access_token().map_err(|error| error.to_string())?;
        let machine_id = self.auth.get_machine_id().map_err(|error| error.to_string())?;
        let ghost_mode =
            resolve_sand_ghost_mode_header(&self.backend_url, &access_token, &machine_id);
        let bytes = send_cursor_unary(
            &self.backend_url,
            &access_token,
            &machine_id,
            path,
            &request.encode_to_vec(),
            timeout_ms,
            ghost_mode,
        )
        .map_err(|error| error.to_string())?;
        Resp::decode(bytes.as_slice()).map_err(|error| error.to_string())
    }
}
impl SkillPublishClient for CursorSkillPublishClient {
    fn get_teams(&self, timeout_ms: u64) -> Result<Vec<SkillPublishTeam>, String> {
        let response: GetTeamsResponseWire = self.unary(
            DASHBOARD_GET_TEAMS_PATH,
            &GetTeamsRequestWire { active_only: Some(true) },
            timeout_ms,
        )?;
        Ok(response
            .teams
            .into_iter()
            .map(|team| SkillPublishTeam {
                team_id: team.id,
                name: team.name,
                is_direct_member: team.is_direct_member,
            })
            .collect())
    }

    fn publish_plugin(
        &self,
        request: PublishPluginInput,
        timeout_ms: u64,
    ) -> Result<PublishedSkillResult, String> {
        let response: PublishPluginResponseWire = self.unary(
            DASHBOARD_PUBLISH_PLUGIN_PATH,
            &PublishPluginRequestWire {
                team_id: request.team_id,
                name: request.name,
                display_name: request.display_name,
                description: request.description,
                plugin_tar_gz: request.plugin_tar_gz,
            },
            timeout_ms,
        )?;
        Ok(PublishedSkillResult {
            plugin_id: response.plugin_id.to_string(),
            commit_sha: response.commit_sha,
        })
    }

    fn unpublish_plugin(
        &self,
        plugin_id: &str,
        team_id: i32,
        timeout_ms: u64,
    ) -> Result<(), String> {
        let plugin_id = plugin_id
            .parse::<i64>()
            .map_err(|_| "published plugin id is not an int64".to_string())?;
        let _: UnpublishPluginResponseWire = self.unary(
            DASHBOARD_UNPUBLISH_PLUGIN_PATH,
            &UnpublishPluginRequestWire {
                plugin_id,
                team_id: Some(team_id),
            },
            timeout_ms,
        )?;
        Ok(())
    }
}

impl AuthRenewalPort for HostAuthExtension {
    fn peek_access_token(&self) -> Option<String> {
        HostAuthExtension::peek_access_token(self)
    }

    fn subscribe_to_renewal(
        &self,
        listener: Arc<dyn Fn(AuthRenewalEvent) + Send + Sync>,
    ) -> Box<dyn FnOnce() + Send> {
        let service = Arc::clone(self.service());
        let subscription = service.subscribe_to_renewal(Arc::new(move |event| {
            listener(AuthRenewalEvent {
                outcome: match event.result.outcome {
                    RenewalOutcome::Renewed => "renewed",
                    RenewalOutcome::Failed => "failed",
                }
                .to_string(),
                is_first_credential: event.is_first_credential,
            });
        }));
        Box::new(move || {
            service.unsubscribe_from_renewal(subscription);
        })
    }
}

impl McpPluginSkillsService for SandPluginSkillsService {
    fn sync(&self, trigger: &str) -> Result<(), String> {
        SandPluginSkillsService::sync(self, trigger).map(|_| ())
    }

    fn handle_auth_change(&self) {
        let _ = SandPluginSkillsService::sync(self, "auth");
    }

    fn dispose(&self) {
        SandPluginSkillsService::dispose(self);
    }
}

impl PluginSkillsPort for SandPluginSkillsService {
    fn sync(&self, trigger: &str) -> Result<Vec<serde_json::Value>, String> {
        SandPluginSkillsService::sync(self, trigger).map(|records| {
            records
                .into_iter()
                .map(|record| serde_json::json!({
                    "id": record.id,
                    "pluginId": record.plugin_id,
                    "pluginName": record.plugin_name,
                    "name": record.name,
                    "description": record.description,
                    "filePath": record.file_path,
                    "pluginVersion": record.plugin_version,
                }))
                .collect()
        })
    }

    fn status(&self) -> serde_json::Value {
        serde_json::json!({ "authBlocked": self.current_auth_blocked() })
    }

    fn remove_live_references(&self, source_urls: &[String]) {
        SandPluginSkillsService::remove_live_references(self, source_urls);
    }
}

impl McpHostServicePort for McpHostService {
    fn dispose(&self) -> Result<(), String> {
        McpHostService::dispose(self)
    }
}

pub struct RealPluginSkillsPolling {
    interval: Duration,
}
impl RealPluginSkillsPolling {
    pub fn daily() -> Self {
        Self { interval: Duration::from_millis(PLUGIN_SKILLS_REFRESH_INTERVAL_MS) }
    }

    pub fn with_interval(interval: Duration) -> Self {
        Self { interval }
    }
}
struct RealPollingHandle {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl PollingHandle for RealPollingHandle {
    fn dispose(&mut self) {
        if self.stop.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(handle) = &self.thread {
            handle.thread().unpark();
        }
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}
impl Drop for RealPollingHandle {
    fn drop(&mut self) {
        self.dispose();
    }
}
impl PollingPort for RealPluginSkillsPolling {
    fn start(&self, callback: Arc<dyn Fn() + Send + Sync>) -> Box<dyn PollingHandle> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let interval = self.interval;
        let handle = thread::Builder::new()
            .name("sand-plugin-skills-refresh".into())
            .spawn(move || {
                while !stop_thread.load(Ordering::SeqCst) {
                    callback();
                    if stop_thread.load(Ordering::SeqCst) {
                        break;
                    }
                    thread::park_timeout(interval);
                }
            })
            .expect("failed to spawn plugin-skills polling thread");
        Box::new(RealPollingHandle { stop, thread: Some(handle) })
    }
}

pub fn create_production_skill_publish(
    sand_root_dir: impl Into<std::path::PathBuf>,
    backend_url: impl Into<String>,
    auth: Arc<HostAuthExtension>,
    plugin_skills: Arc<SandPluginSkillsService>,
) -> SandSkillPublishService {
    let client: Arc<dyn SkillPublishClient> =
        Arc::new(CursorSkillPublishClient::new(backend_url, auth));
    SandSkillPublishService::new(sand_root_dir, client, plugin_skills)
}
