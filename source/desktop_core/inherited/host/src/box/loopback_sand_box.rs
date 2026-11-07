use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::extensions::telemetry::lifecycle_telemetry::DaemonPingReport;
use crate::ports::r#box::{SandBoxDaemonUnreachableError, is_primary_window_index};

use super::box_env::{BoxEnvironmentUpdate, apply_box_environment_via_transport};
use super::box_file_transfer::upload_file_via_exec_daemon;
use super::box_mcp::load_box_mcp_servers_via_transport;
use super::box_remote_accessor::{
    BoxEndpoint, DEFAULT_BOX_PING_TIMEOUT_MS, ping_box_transport_classified,
};
use super::box_transfer::{BoxFileUnreadableError, resolve_box_workspace_path};
use super::box_windows::{
    BoxConnection, SAND_BOX_DISPLAY_HEADER, SAND_BOX_FORK_ROUTER_PORT,
    SAND_BOX_WINDOW_OWNER_HEADER, SandBoxWindow, primary_sand_box_window,
    run_start_window, run_stop_window, sand_box_display_token, sand_box_max_windows,
};
use super::generated_production::{
    ProductionBoxResourceAccessor, ProductionBoxTransport,
    create_production_box_control_client, create_production_box_resource_accessor,
};
use super::protected_path_guard::{SandProtectedPathError, assert_path_outside_protected_roots};

pub const EXEC_DAEMON_PORT: u16 = 1337;
pub const VNC_PORT: u16 = 6080;
pub const FORK_VNC_PORT: u16 = 6081;
pub const DEFAULT_AUTH_TOKEN: &str = "local";
pub const BOX_TERMINALS_FOLDER: &str = "/root/.cursor/projects/workspace/terminals";
pub const DAEMON_READY_TIMEOUT_MS: u64 = 90_000;
pub const DAEMON_READY_POLL_INTERVAL_MS: u64 = 500;
pub const DAEMON_WATCHDOG_INTERVAL_MS: u64 = 30_000;

pub type DaemonPingReporter = Arc<dyn Fn(&DaemonPingReport) + Send + Sync + 'static>;

#[derive(Debug)]
struct DaemonWatchdogEpisode {
    readiness: &'static str,
    unready_since: Option<Instant>,
    unready_attempts: u64,
}

impl Default for DaemonWatchdogEpisode {
    fn default() -> Self {
        Self {
            readiness: "unknown",
            unready_since: None,
            unready_attempts: 0,
        }
    }
}

struct LoopbackSharedState {
    telemetry: Mutex<Option<DaemonPingReporter>>,
    watchdog_started: AtomicBool,
    foreground_ready_waits: AtomicUsize,
    poll_gate: Mutex<()>,
    episode: Mutex<DaemonWatchdogEpisode>,
}

impl Default for LoopbackSharedState {
    fn default() -> Self {
        Self {
            telemetry: Mutex::new(None),
            watchdog_started: AtomicBool::new(false),
            foreground_ready_waits: AtomicUsize::new(0),
            poll_gate: Mutex::new(()),
            episode: Mutex::new(DaemonWatchdogEpisode::default()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopbackSandBoxOptions {
    pub host: String,
    pub auth_token: String,
    pub exec_daemon_port: u16,
    pub ready_timeout_ms: u64,
    pub poll_interval_ms: u64,
    pub watchdog_interval_ms: u64,
    pub protected_box_paths: Vec<PathBuf>,
}

impl Default for LoopbackSandBoxOptions {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            auth_token: DEFAULT_AUTH_TOKEN.into(),
            exec_daemon_port: EXEC_DAEMON_PORT,
            ready_timeout_ms: DAEMON_READY_TIMEOUT_MS,
            poll_interval_ms: DAEMON_READY_POLL_INTERVAL_MS,
            watchdog_interval_ms: DAEMON_WATCHDOG_INTERVAL_MS,
            protected_box_paths: Vec::new(),
        }
    }
}

#[derive(Debug, Error)]
pub enum LoopbackSandBoxError {
    #[error(transparent)]
    Daemon(#[from] SandBoxDaemonUnreachableError),
    #[error(transparent)]
    ProtectedPath(#[from] SandProtectedPathError),
    #[error(transparent)]
    Unreadable(#[from] BoxFileUnreadableError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Transport(String),
}

pub struct LoopbackReady {
    pub remote_accessor: ProductionBoxResourceAccessor,
    pub vnc_url: String,
    pub terminals_folder: String,
}

#[derive(Clone)]
pub struct LoopbackSandBox {
    options: LoopbackSandBoxOptions,
    shared: Arc<LoopbackSharedState>,
}

impl std::fmt::Debug for LoopbackSandBox {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LoopbackSandBox")
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl LoopbackSandBox {
    pub fn new(options: LoopbackSandBoxOptions) -> Self {
        Self {
            options,
            shared: Arc::new(LoopbackSharedState::default()),
        }
    }

    pub fn set_telemetry(&self, reporter: DaemonPingReporter) {
        *self
            .shared
            .telemetry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(reporter);
    }

    fn report_daemon_ping(&self, report: DaemonPingReport) {
        let reporter = self
            .shared
            .telemetry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(reporter) = reporter {
            reporter(&report);
        }
    }

    pub fn options(&self) -> &LoopbackSandBoxOptions {
        &self.options
    }

    pub fn describe(&self) -> &'static str {
        "loopback"
    }

    pub fn terminals_folder(&self) -> &'static str {
        BOX_TERMINALS_FOLDER
    }

    pub fn is_available(&self) -> bool {
        true
    }

    pub fn max_windows(&self) -> u32 {
        sand_box_max_windows()
    }

    pub fn primary_endpoint(&self) -> BoxEndpoint {
        BoxEndpoint::new(
            self.options.host.clone(),
            self.options.exec_daemon_port,
            self.options.auth_token.clone(),
        )
    }

    pub fn assert_file_read_allowed(&self, box_path: &str) -> Result<(), LoopbackSandBoxError> {
        assert_path_outside_protected_roots(
            &self.options.protected_box_paths,
            Path::new(box_path),
            Path::new("/workspace"),
        )?;
        Ok(())
    }

    pub fn wait_until_ready<Ctx>(
        &self,
        ctx: &Ctx,
        endpoint: &BoxEndpoint,
        timeout_ms: u64,
    ) -> Result<(), LoopbackSandBoxError> {
        let primary = endpoint.port == self.options.exec_daemon_port;
        if !primary {
            return self.wait_until_ready_uncoordinated(ctx, endpoint, timeout_ms);
        }
        self.shared
            .foreground_ready_waits
            .fetch_add(1, Ordering::AcqRel);
        let result = {
            let _poll = self
                .shared
                .poll_gate
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            self.wait_until_ready_uncoordinated(ctx, endpoint, timeout_ms)
        };
        self.shared
            .foreground_ready_waits
            .fetch_sub(1, Ordering::AcqRel);
        result
    }

    fn wait_until_ready_uncoordinated<Ctx>(
        &self,
        ctx: &Ctx,
        endpoint: &BoxEndpoint,
        timeout_ms: u64,
    ) -> Result<(), LoopbackSandBoxError> {
        let started = Instant::now();
        let target = format!("{}:{}", endpoint.host, endpoint.port);
        let primary = endpoint.port == self.options.exec_daemon_port;
        let mut attempts = 0u64;
        let mut last_outcome = "refused".to_string();
        let mut last_cause = None::<String>;
        let mut unready_since = None::<Instant>;

        while started.elapsed() < Duration::from_millis(timeout_ms) {
            attempts = attempts.saturating_add(1);
            let transport = ProductionBoxTransport::from_endpoint(endpoint);
            let result = ping_box_transport_classified(
                ctx,
                &transport,
                create_production_box_control_client,
                DEFAULT_BOX_PING_TIMEOUT_MS,
            );
            last_outcome = result.outcome.to_string();
            last_cause = result.cause_summary;
            if result.outcome == "ok" {
                let now = Instant::now();
                let mut recovery = None;
                if primary {
                    let mut episode = self
                        .shared
                        .episode
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let watchdog_outage = episode.readiness == "unready";
                    if attempts > 1 || watchdog_outage {
                        let episode_start = if watchdog_outage {
                            episode.unready_since.unwrap_or(started)
                        } else {
                            unready_since.unwrap_or(started)
                        };
                        recovery = Some(DaemonPingReport {
                            outcome: "ok".into(),
                            attempts: if watchdog_outage {
                                episode.unready_attempts.saturating_add(attempts)
                            } else {
                                attempts
                            },
                            duration_ms: millis(now.duration_since(if watchdog_outage {
                                episode_start
                            } else {
                                started
                            })),
                            unready_duration_ms: millis(now.duration_since(episode_start)),
                            readiness_state: "ready_after_retry".into(),
                            target: target.clone(),
                            cause_summary: None,
                        });
                    }
                    episode.readiness = "ready";
                    episode.unready_since = None;
                    episode.unready_attempts = 0;
                } else if attempts > 1 {
                    let episode_start = unready_since.unwrap_or(started);
                    recovery = Some(DaemonPingReport {
                        outcome: "ok".into(),
                        attempts,
                        duration_ms: millis(now.duration_since(started)),
                        unready_duration_ms: millis(now.duration_since(episode_start)),
                        readiness_state: "ready_after_retry".into(),
                        target: target.clone(),
                        cause_summary: None,
                    });
                }
                if let Some(report) = recovery {
                    self.report_daemon_ping(report);
                }
                if primary {
                    self.start_daemon_watchdog(endpoint.clone());
                }
                return Ok(());
            }
            unready_since.get_or_insert_with(Instant::now);
            if self.options.poll_interval_ms > 0 {
                thread::sleep(Duration::from_millis(self.options.poll_interval_ms));
            }
        }

        let now = Instant::now();
        let episode_start = unready_since.unwrap_or(started);
        self.report_daemon_ping(DaemonPingReport {
            outcome: last_outcome.clone(),
            attempts,
            duration_ms: millis(now.duration_since(started)),
            unready_duration_ms: millis(now.duration_since(episode_start)),
            readiness_state: daemon_ping_readiness_state(&last_outcome).into(),
            target: target.clone(),
            cause_summary: last_cause.clone(),
        });
        if primary && self.shared.watchdog_started.load(Ordering::Acquire) {
            let mut episode = self
                .shared
                .episode
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if episode.readiness == "ready" {
                episode.unready_since = Some(episode_start);
                episode.unready_attempts = attempts;
            } else {
                episode.unready_attempts = episode.unready_attempts.saturating_add(attempts);
            }
            episode.readiness = "unready";
        }

        let cause = last_cause
            .as_deref()
            .map(|cause| format!(" [{cause}]"))
            .unwrap_or_default();
        Err(SandBoxDaemonUnreachableError::new(
            last_outcome.clone(),
            format!(
                "loopback sand box exec-daemon at {target} not ready within {timeout_ms}ms (attempts: {attempts}; last ping: {last_outcome}{cause})"
            ),
        )
        .into())
    }

    fn start_daemon_watchdog(&self, endpoint: BoxEndpoint) {
        let has_telemetry = self
            .shared
            .telemetry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some();
        if !has_telemetry
            || self.options.watchdog_interval_ms == 0
            || self
                .shared
                .watchdog_started
                .swap(true, Ordering::AcqRel)
        {
            return;
        }
        {
            let mut episode = self
                .shared
                .episode
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            episode.readiness = "ready";
        }
        let weak = Arc::downgrade(&self.shared);
        let interval = Duration::from_millis(self.options.watchdog_interval_ms);
        let _ = thread::Builder::new()
            .name("mahayana-loopback-daemon-watchdog".into())
            .spawn(move || daemon_watchdog_loop(weak, endpoint, interval));
    }

    pub fn ensure_ready<Ctx>(
        &self,
        ctx: &Ctx,
        _agent_id: &str,
    ) -> Result<LoopbackReady, LoopbackSandBoxError> {
        let endpoint = self.primary_endpoint();
        self.wait_until_ready(ctx, &endpoint, self.options.ready_timeout_ms)?;
        let transport = ProductionBoxTransport::from_endpoint(&endpoint);
        Ok(LoopbackReady {
            remote_accessor: create_production_box_resource_accessor(&transport)
                .with_file_read_guard(self.options.protected_box_paths.clone()),
            vnc_url: format!("http://{}:{VNC_PORT}/vnc.html", self.options.host),
            terminals_folder: BOX_TERMINALS_FOLDER.into(),
        })
    }

    pub fn apply_environment<Ctx>(
        &self,
        ctx: &Ctx,
        update: &BoxEnvironmentUpdate,
    ) -> Result<(), LoopbackSandBoxError> {
        let endpoint = self.primary_endpoint();
        self.wait_until_ready(ctx, &endpoint, self.options.ready_timeout_ms)?;
        let transport = ProductionBoxTransport::from_endpoint(&endpoint);
        apply_box_environment_via_transport(
            ctx,
            &transport,
            update,
            create_production_box_control_client,
        )
        .map_err(|error| LoopbackSandBoxError::Transport(error.to_string()))
    }

    pub fn load_mcp_servers<Ctx>(
        &self,
        ctx: &Ctx,
        config_json: &str,
    ) -> Result<Vec<String>, LoopbackSandBoxError> {
        let endpoint = self.primary_endpoint();
        self.wait_until_ready(ctx, &endpoint, self.options.ready_timeout_ms)?;
        let transport = ProductionBoxTransport::from_endpoint(&endpoint);
        load_box_mcp_servers_via_transport(
            ctx,
            &transport,
            config_json,
            create_production_box_control_client,
        )
        .map_err(|error| LoopbackSandBoxError::Transport(error.to_string()))
    }

    pub fn mcp_resource_accessor<Ctx>(
        &self,
        ctx: &Ctx,
    ) -> Result<ProductionBoxResourceAccessor, LoopbackSandBoxError> {
        let endpoint = self.primary_endpoint();
        self.wait_until_ready(ctx, &endpoint, self.options.ready_timeout_ms)?;
        let transport = ProductionBoxTransport::from_endpoint(&endpoint);
        Ok(create_production_box_resource_accessor(&transport)
            .with_file_read_guard(self.options.protected_box_paths.clone()))
    }

    pub fn ensure_window<Ctx>(
        &self,
        ctx: &Ctx,
        agent_id: &str,
        window_index: u32,
        owner_token: Option<&str>,
    ) -> Result<SandBoxWindow<ProductionBoxResourceAccessor>, LoopbackSandBoxError> {
        if is_primary_window_index(window_index) {
            let ready = self.ensure_ready(ctx, agent_id)?;
            return Ok(primary_sand_box_window(BoxConnection {
                remote_accessor: ready.remote_accessor,
                vnc_url: ready.vnc_url,
            }));
        }

        let mut primary = self.ensure_ready(ctx, agent_id)?;
        run_start_window(
            ctx,
            &mut primary.remote_accessor,
            window_index,
            owner_token,
        )
        .map_err(|error| LoopbackSandBoxError::Transport(error.to_string()))?;

        let display_token = sand_box_display_token(window_index);
        let mut endpoint = BoxEndpoint::new(
            self.options.host.clone(),
            SAND_BOX_FORK_ROUTER_PORT,
            self.options.auth_token.clone(),
        );
        endpoint
            .headers
            .insert(SAND_BOX_DISPLAY_HEADER.into(), display_token.clone());
        if let Some(owner_token) = owner_token {
            endpoint
                .headers
                .insert(SAND_BOX_WINDOW_OWNER_HEADER.into(), owner_token.into());
        }
        self.wait_until_ready(ctx, &endpoint, self.options.ready_timeout_ms)?;
        let transport = ProductionBoxTransport::from_endpoint(&endpoint);
        Ok(SandBoxWindow {
            window_index,
            computer_use: create_production_box_resource_accessor(&transport)
                .with_file_read_guard(self.options.protected_box_paths.clone()),
            vnc_url: format!(
                "http://{}:{FORK_VNC_PORT}/vnc.html?path=websockify%3Ftoken%3D{display_token}",
                self.options.host
            ),
        })
    }

    pub fn release_window<Ctx>(
        &self,
        ctx: &Ctx,
        agent_id: &str,
        window_index: u32,
    ) -> Result<(), LoopbackSandBoxError> {
        if is_primary_window_index(window_index) {
            return Ok(());
        }
        let mut primary = self.ensure_ready(ctx, agent_id)?;
        let _ = run_stop_window(ctx, &mut primary.remote_accessor, window_index);
        Ok(())
    }

    pub fn upload_file<Ctx>(
        &self,
        ctx: &Ctx,
        agent_id: &str,
        box_path: &str,
        data: &[u8],
    ) -> Result<(), LoopbackSandBoxError> {
        let mut ready = self.ensure_ready(ctx, agent_id)?;
        let resolved = resolve_box_workspace_path(box_path);
        upload_file_via_exec_daemon(ctx, &mut ready.remote_accessor, &resolved, data)
            .map_err(|error| LoopbackSandBoxError::Transport(error.to_string()))
    }

    pub fn download_file<Ctx>(
        &self,
        _ctx: &Ctx,
        _agent_id: &str,
        box_path: &str,
    ) -> Result<Vec<u8>, LoopbackSandBoxError> {
        let resolved = resolve_box_workspace_path(box_path);
        self.assert_file_read_allowed(&resolved)?;
        match fs::read(&resolved) {
            Ok(bytes) => Ok(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(
                BoxFileUnreadableError(format!(
                    "download from box {resolved} failed (file missing)"
                ))
                .into(),
            ),
            Err(error) => Err(error.into()),
        }
    }

    pub fn run_state(&self) -> &'static str {
        "running"
    }

    pub fn list_boxes(&self) -> Vec<(String, bool)> {
        vec![(String::new(), true)]
    }

    pub fn hibernate(&self, _agent_id: &str) {}
}

pub fn daemon_ping_readiness_state(outcome: &str) -> &'static str {
    match outcome {
        "refused" => "up_but_exec_refused",
        "timeout" => "up_but_exec_unresponsive",
        _ => "up_but_exec_disconnected",
    }
}

fn millis(duration: Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}

fn report_from_shared(shared: &LoopbackSharedState, report: &DaemonPingReport) {
    let reporter = shared
        .telemetry
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    if let Some(reporter) = reporter {
        reporter(report);
    }
}

fn daemon_watchdog_loop(
    weak: Weak<LoopbackSharedState>,
    endpoint: BoxEndpoint,
    interval: Duration,
) {
    loop {
        thread::sleep(interval);
        let Some(shared) = weak.upgrade() else {
            return;
        };
        if shared.foreground_ready_waits.load(Ordering::Acquire) > 0 {
            continue;
        }
        let _poll = shared
            .poll_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if shared.foreground_ready_waits.load(Ordering::Acquire) > 0 {
            continue;
        }
        let started = Instant::now();
        let transport = ProductionBoxTransport::from_endpoint(&endpoint);
        let result = ping_box_transport_classified(
            &(),
            &transport,
            create_production_box_control_client,
            DEFAULT_BOX_PING_TIMEOUT_MS,
        );
        let now = Instant::now();
        let target = format!("{}:{}", endpoint.host, endpoint.port);
        let mut report = None;
        {
            let mut episode = shared
                .episode
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if result.outcome == "ok" {
                if episode.readiness == "unready" {
                    let since = episode.unready_since.unwrap_or(started);
                    report = Some(DaemonPingReport {
                        outcome: "ok".into(),
                        attempts: episode.unready_attempts.saturating_add(1),
                        duration_ms: millis(now.duration_since(since)),
                        unready_duration_ms: millis(now.duration_since(since)),
                        readiness_state: "ready_after_retry".into(),
                        target,
                        cause_summary: None,
                    });
                }
                episode.readiness = "ready";
                episode.unready_since = None;
                episode.unready_attempts = 0;
            } else if episode.readiness != "unready" {
                episode.readiness = "unready";
                episode.unready_since = Some(started);
                episode.unready_attempts = 1;
                report = Some(DaemonPingReport {
                    outcome: result.outcome.to_string(),
                    attempts: 1,
                    duration_ms: millis(now.duration_since(started)),
                    unready_duration_ms: millis(now.duration_since(started)),
                    readiness_state: daemon_ping_readiness_state(result.outcome.as_ref()).into(),
                    target,
                    cause_summary: result.cause_summary,
                });
            } else {
                episode.unready_attempts = episode.unready_attempts.saturating_add(1);
            }
        }
        if let Some(report) = report {
            report_from_shared(&shared, &report);
        }
    }
}
