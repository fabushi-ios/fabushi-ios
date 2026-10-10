use crate::box_env::BoxEnvironmentUpdate;
use crate::box_remote_accessor::BoxEndpoint;
use crate::box_windows::{
    SAND_BOX_DISPLAY_HEADER, SAND_BOX_FORK_ROUTER_PORT,
    SAND_BOX_PRIMARY_WINDOW_INDEX, SAND_BOX_WINDOW_OWNER_HEADER,
    is_primary_window_index, sand_box_display_token,
};
use std::collections::BTreeMap;

pub const EXEC_DAEMON_PORT: u16 = 1337;
pub const DEFAULT_AUTH_TOKEN: &str = "local";
pub const BOX_TERMINALS_FOLDER: &str =
    "/root/.cursor/projects/workspace/terminals";
pub const DAEMON_READY_TIMEOUT_MS: u64 = 90_000;
pub const DAEMON_WATCHDOG_INTERVAL_MS: u64 = 30_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonPingOutcome {
    Ok,
    Refused,
    Timeout,
    Dns,
    Disconnected,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PingResult {
    pub outcome: DaemonPingOutcome,
    pub cause_summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonPingReport {
    pub outcome: DaemonPingOutcome,
    pub attempts: usize,
    pub duration_ms: u64,
    pub readiness_state: &'static str,
    pub target: String,
    pub cause_summary: Option<String>,
}

pub fn daemon_ping_readiness_state(
    outcome: &DaemonPingOutcome,
) -> &'static str {
    match outcome {
        DaemonPingOutcome::Refused => "up_but_exec_refused",
        DaemonPingOutcome::Timeout => "up_but_exec_unresponsive",
        _ => "up_but_exec_disconnected",
    }
}

pub trait RemoteRunnerBoxOperations<Context> {
    type Error;
    type Accessor;

    async fn ping(
        &self,
        context: &Context,
        endpoint: &BoxEndpoint,
    ) -> Result<PingResult, Self::Error>;

    fn create_remote_accessor(
        &self,
        endpoint: &BoxEndpoint,
    ) -> Self::Accessor;

    async fn apply_environment(
        &self,
        context: &Context,
        endpoint: &BoxEndpoint,
        update: &BoxEnvironmentUpdate,
    ) -> Result<(), Self::Error>;

    async fn load_mcp_servers(
        &self,
        context: &Context,
        endpoint: &BoxEndpoint,
        config_json: &str,
    ) -> Result<Vec<String>, Self::Error>;

    async fn upload_file(
        &self,
        context: &Context,
        endpoint: &BoxEndpoint,
        path: &str,
        data: &[u8],
    ) -> Result<(), Self::Error>;

    async fn download_file(
        &self,
        context: &Context,
        endpoint: &BoxEndpoint,
        path: &str,
    ) -> Result<Vec<u8>, Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandBoxReadyError<RemoteError> {
    Remote(RemoteError),
    NotReady(DaemonPingReport),
}

/// iOS counterpart of Grok's loopback box.
///
/// The reference name is retained for one-to-one ledger correspondence, but
/// this implementation never spawns or reads from a local daemon. Every
/// operation is delegated to an explicitly supplied Remote Runner endpoint.
pub struct LoopbackSandBox<Operations> {
    operations: Operations,
    primary_endpoint: BoxEndpoint,
    ready_attempt_limit: usize,
    max_windows: u32,
}

impl<Operations> LoopbackSandBox<Operations> {
    pub fn new_remote(
        operations: Operations,
        primary_endpoint: BoxEndpoint,
        max_windows: u32,
    ) -> Self {
        Self {
            operations,
            primary_endpoint,
            ready_attempt_limit: 6,
            max_windows: max_windows.max(1),
        }
    }

    pub fn with_ready_attempt_limit(mut self, attempts: usize) -> Self {
        self.ready_attempt_limit = attempts.max(1);
        self
    }

    pub fn describe(&self) -> &'static str {
        "remote-runner"
    }

    pub fn terminals_folder(&self) -> &'static str {
        BOX_TERMINALS_FOLDER
    }

    pub fn max_windows(&self) -> u32 {
        self.max_windows
    }

    pub fn primary_endpoint(&self) -> &BoxEndpoint {
        &self.primary_endpoint
    }

    pub fn endpoint_for_window(
        &self,
        window_index: u32,
        owner_token: Option<&str>,
    ) -> BoxEndpoint {
        if is_primary_window_index(window_index) {
            return self.primary_endpoint.clone();
        }
        let mut headers = BTreeMap::from([(
            SAND_BOX_DISPLAY_HEADER.to_owned(),
            sand_box_display_token(window_index),
        )]);
        if let Some(token) = owner_token {
            headers.insert(
                SAND_BOX_WINDOW_OWNER_HEADER.to_owned(),
                token.to_owned(),
            );
        }
        BoxEndpoint {
            host: self.primary_endpoint.host.clone(),
            port: SAND_BOX_FORK_ROUTER_PORT,
            auth_token: self.primary_endpoint.auth_token.clone(),
            headers,
        }
    }
}

impl<Operations> LoopbackSandBox<Operations> {
    pub async fn wait_until_ready<Context>(
        &self,
        context: &Context,
        endpoint: &BoxEndpoint,
        mut now: impl FnMut() -> u64,
        mut report: impl FnMut(DaemonPingReport),
    ) -> Result<(), SandBoxReadyError<Operations::Error>>
    where
        Operations: RemoteRunnerBoxOperations<Context>,
    {
        let started = now();
        let target = format!("{}:{}", endpoint.host, endpoint.port);
        let mut last = PingResult {
            outcome: DaemonPingOutcome::Refused,
            cause_summary: None,
        };

        for attempt in 1..=self.ready_attempt_limit {
            last = self
                .operations
                .ping(context, endpoint)
                .await
                .map_err(SandBoxReadyError::Remote)?;
            if last.outcome == DaemonPingOutcome::Ok {
                if attempt > 1 {
                    report(DaemonPingReport {
                        outcome: DaemonPingOutcome::Ok,
                        attempts: attempt,
                        duration_ms: now().saturating_sub(started),
                        readiness_state: "ready_after_retry",
                        target,
                        cause_summary: last.cause_summary,
                    });
                }
                return Ok(());
            }
        }

        let final_report = DaemonPingReport {
            readiness_state: daemon_ping_readiness_state(&last.outcome),
            outcome: last.outcome,
            attempts: self.ready_attempt_limit,
            duration_ms: now().saturating_sub(started),
            target,
            cause_summary: last.cause_summary,
        };
        report(final_report.clone());
        Err(SandBoxReadyError::NotReady(final_report))
    }

    pub async fn ensure_ready<Context>(
        &self,
        context: &Context,
        now: impl FnMut() -> u64,
        report: impl FnMut(DaemonPingReport),
    ) -> Result<Operations::Accessor, SandBoxReadyError<Operations::Error>>
    where
        Operations: RemoteRunnerBoxOperations<Context>,
    {
        self.wait_until_ready(
            context,
            &self.primary_endpoint,
            now,
            report,
        )
        .await?;
        Ok(self
            .operations
            .create_remote_accessor(&self.primary_endpoint))
    }

    pub async fn apply_environment<Context>(
        &self,
        context: &Context,
        update: &BoxEnvironmentUpdate,
    ) -> Result<(), Operations::Error>
    where
        Operations: RemoteRunnerBoxOperations<Context>,
    {
        self.operations
            .apply_environment(context, &self.primary_endpoint, update)
            .await
    }

    pub async fn load_mcp_servers<Context>(
        &self,
        context: &Context,
        config_json: &str,
    ) -> Result<Vec<String>, Operations::Error>
    where
        Operations: RemoteRunnerBoxOperations<Context>,
    {
        self.operations
            .load_mcp_servers(
                context,
                &self.primary_endpoint,
                config_json,
            )
            .await
    }

    pub async fn upload_file<Context>(
        &self,
        context: &Context,
        path: &str,
        data: &[u8],
    ) -> Result<(), Operations::Error>
    where
        Operations: RemoteRunnerBoxOperations<Context>,
    {
        self.operations
            .upload_file(
                context,
                &self.primary_endpoint,
                path,
                data,
            )
            .await
    }

    pub async fn download_file<Context>(
        &self,
        context: &Context,
        path: &str,
    ) -> Result<Vec<u8>, Operations::Error>
    where
        Operations: RemoteRunnerBoxOperations<Context>,
    {
        self.operations
            .download_file(
                context,
                &self.primary_endpoint,
                path,
            )
            .await
    }

    pub fn primary_window_index(&self) -> u32 {
        SAND_BOX_PRIMARY_WINDOW_INDEX
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::convert::Infallible;
    use std::sync::Mutex;

    struct Operations {
        pings: Mutex<VecDeque<PingResult>>,
    }

    impl RemoteRunnerBoxOperations<()> for Operations {
        type Error = Infallible;
        type Accessor = String;

        async fn ping(
            &self,
            _context: &(),
            _endpoint: &BoxEndpoint,
        ) -> Result<PingResult, Self::Error> {
            Ok(self.pings.lock().unwrap().pop_front().unwrap_or(
                PingResult {
                    outcome: DaemonPingOutcome::Ok,
                    cause_summary: None,
                },
            ))
        }

        fn create_remote_accessor(
            &self,
            endpoint: &BoxEndpoint,
        ) -> Self::Accessor {
            endpoint.base_url()
        }

        async fn apply_environment(
            &self,
            _context: &(),
            _endpoint: &BoxEndpoint,
            _update: &BoxEnvironmentUpdate,
        ) -> Result<(), Self::Error> {
            Ok(())
        }

        async fn load_mcp_servers(
            &self,
            _context: &(),
            _endpoint: &BoxEndpoint,
            _config_json: &str,
        ) -> Result<Vec<String>, Self::Error> {
            Ok(vec!["github".into()])
        }

        async fn upload_file(
            &self,
            _context: &(),
            _endpoint: &BoxEndpoint,
            _path: &str,
            _data: &[u8],
        ) -> Result<(), Self::Error> {
            Ok(())
        }

        async fn download_file(
            &self,
            _context: &(),
            _endpoint: &BoxEndpoint,
            _path: &str,
        ) -> Result<Vec<u8>, Self::Error> {
            Ok(b"remote".to_vec())
        }
    }

    fn endpoint() -> BoxEndpoint {
        BoxEndpoint {
            host: "runner.example.test".into(),
            port: EXEC_DAEMON_PORT,
            auth_token: "token".into(),
            headers: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn readiness_retries_and_reports_recovery() {
        let box_ = LoopbackSandBox::new_remote(
            Operations {
                pings: Mutex::new(VecDeque::from([
                    PingResult {
                        outcome: DaemonPingOutcome::Refused,
                        cause_summary: Some("Unavailable/ECONNREFUSED".into()),
                    },
                    PingResult {
                        outcome: DaemonPingOutcome::Ok,
                        cause_summary: None,
                    },
                ])),
            },
            endpoint(),
            8,
        )
        .with_ready_attempt_limit(3);
        let reports = std::sync::Arc::new(Mutex::new(Vec::new()));
        let capture = reports.clone();
        let mut clock = vec![100, 150].into_iter();
        let accessor = box_
            .ensure_ready(
                &(),
                || clock.next().unwrap_or(150),
                move |report| capture.lock().unwrap().push(report),
            )
            .await
            .unwrap();
        assert_eq!(accessor, "http://runner.example.test:1337");
        assert_eq!(reports.lock().unwrap()[0].readiness_state, "ready_after_retry");
    }

    #[test]
    fn fork_endpoint_carries_display_and_owner_headers() {
        let box_ = LoopbackSandBox::new_remote(
            Operations {
                pings: Mutex::new(VecDeque::new()),
            },
            endpoint(),
            6,
        );
        let fork = box_.endpoint_for_window(3, Some("owner-3"));
        assert_eq!(fork.port, SAND_BOX_FORK_ROUTER_PORT);
        assert_eq!(
            fork.headers.get(SAND_BOX_DISPLAY_HEADER).map(String::as_str),
            Some("3")
        );
        assert_eq!(
            fork.headers
                .get(SAND_BOX_WINDOW_OWNER_HEADER)
                .map(String::as_str),
            Some("owner-3")
        );
    }
}
