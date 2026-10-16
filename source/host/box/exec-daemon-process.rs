pub const BOX_EXEC_DAEMON_START_TIMEOUT_MS: u64 = 20_000;
pub const BOX_EXEC_DAEMON_STOP_TIMEOUT_MS: u64 = 5_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxExecDaemonEndpoint {
    pub runner_id: String,
    pub endpoint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedBoxExecDaemon {
    pub endpoint: BoxExecDaemonEndpoint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxExecDaemonStartError<RemoteError> {
    LocalProcessSpawnForbidden,
    ContaminatedEndpoint,
    Remote(RemoteError),
    ReadinessTimeout,
}

pub trait RemoteBoxExecDaemonLifecycle {
    type Error;

    async fn endpoint_is_bound(
        &self,
        endpoint: &BoxExecDaemonEndpoint,
    ) -> Result<bool, Self::Error>;

    async fn start(
        &self,
        endpoint: &BoxExecDaemonEndpoint,
    ) -> Result<(), Self::Error>;

    async fn ping_authenticated(
        &self,
        endpoint: &BoxExecDaemonEndpoint,
    ) -> Result<bool, Self::Error>;

    async fn stop(
        &self,
        endpoint: &BoxExecDaemonEndpoint,
    ) -> Result<(), Self::Error>;
}

/// iOS never spawns the desktop Node exec-daemon. The same lifecycle contract
/// is fulfilled by an explicitly selected Remote Runner endpoint.
pub async fn start_box_exec_daemon_process<Lifecycle>(
    lifecycle: &Lifecycle,
    endpoint: BoxExecDaemonEndpoint,
    readiness_attempts: usize,
) -> Result<OwnedBoxExecDaemon, BoxExecDaemonStartError<Lifecycle::Error>>
where
    Lifecycle: RemoteBoxExecDaemonLifecycle,
{
    if lifecycle
        .endpoint_is_bound(&endpoint)
        .await
        .map_err(BoxExecDaemonStartError::Remote)?
    {
        return Err(BoxExecDaemonStartError::ContaminatedEndpoint);
    }

    lifecycle
        .start(&endpoint)
        .await
        .map_err(BoxExecDaemonStartError::Remote)?;

    for _ in 0..readiness_attempts.max(1) {
        if lifecycle
            .ping_authenticated(&endpoint)
            .await
            .map_err(BoxExecDaemonStartError::Remote)?
        {
            return Ok(OwnedBoxExecDaemon { endpoint });
        }
    }

    let _ = lifecycle.stop(&endpoint).await;
    Err(BoxExecDaemonStartError::ReadinessTimeout)
}

pub async fn close_box_exec_daemon<Lifecycle>(
    lifecycle: &Lifecycle,
    daemon: &OwnedBoxExecDaemon,
) -> Result<(), Lifecycle::Error>
where
    Lifecycle: RemoteBoxExecDaemonLifecycle,
{
    lifecycle.stop(&daemon.endpoint).await
}

pub fn refuse_local_box_exec_daemon_spawn<Error>()
    -> Result<(), BoxExecDaemonStartError<Error>>
{
    Err(BoxExecDaemonStartError::LocalProcessSpawnForbidden)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct Lifecycle {
        bound: bool,
        started: AtomicBool,
        stopped: AtomicBool,
        pings_before_ready: usize,
        pings: AtomicUsize,
    }

    impl RemoteBoxExecDaemonLifecycle for Lifecycle {
        type Error = Infallible;

        async fn endpoint_is_bound(
            &self,
            _endpoint: &BoxExecDaemonEndpoint,
        ) -> Result<bool, Self::Error> {
            Ok(self.bound)
        }

        async fn start(
            &self,
            _endpoint: &BoxExecDaemonEndpoint,
        ) -> Result<(), Self::Error> {
            self.started.store(true, Ordering::SeqCst);
            Ok(())
        }

        async fn ping_authenticated(
            &self,
            _endpoint: &BoxExecDaemonEndpoint,
        ) -> Result<bool, Self::Error> {
            let current = self.pings.fetch_add(1, Ordering::SeqCst);
            Ok(current >= self.pings_before_ready)
        }

        async fn stop(
            &self,
            _endpoint: &BoxExecDaemonEndpoint,
        ) -> Result<(), Self::Error> {
            self.stopped.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    fn endpoint() -> BoxExecDaemonEndpoint {
        BoxExecDaemonEndpoint {
            runner_id: "runner-1".into(),
            endpoint: "wss://runner.example.test/exec".into(),
        }
    }

    #[tokio::test]
    async fn starts_only_after_authenticated_remote_readiness() {
        let lifecycle = Lifecycle {
            bound: false,
            started: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            pings_before_ready: 1,
            pings: AtomicUsize::new(0),
        };
        let daemon = start_box_exec_daemon_process(
            &lifecycle,
            endpoint(),
            3,
        )
        .await
        .unwrap();
        assert!(lifecycle.started.load(Ordering::SeqCst));
        assert_eq!(daemon.endpoint.runner_id, "runner-1");
        close_box_exec_daemon(&lifecycle, &daemon).await.unwrap();
        assert!(lifecycle.stopped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn refuses_contaminated_endpoint_and_local_spawn() {
        let lifecycle = Lifecycle {
            bound: true,
            started: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            pings_before_ready: 0,
            pings: AtomicUsize::new(0),
        };
        assert!(matches!(
            start_box_exec_daemon_process(&lifecycle, endpoint(), 1).await,
            Err(BoxExecDaemonStartError::ContaminatedEndpoint)
        ));
        assert!(!lifecycle.started.load(Ordering::SeqCst));
        assert!(matches!(
            refuse_local_box_exec_daemon_spawn::<Infallible>(),
            Err(BoxExecDaemonStartError::LocalProcessSpawnForbidden)
        ));
    }
}
