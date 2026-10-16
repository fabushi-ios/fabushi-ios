use std::any::Any;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub const UNBOUND_EXECUTION_MESSAGE: &str = "Sand turn execution is not bound: the host asked for a runner before the composition root handed the turn-execution extension its executor.";
pub const DOUBLE_BIND_MESSAGE: &str = "Sand turn execution is already bound: a second executor would mint a second runner for the same agent.";

pub type TurnExecutionValue = Arc<dyn Any + Send + Sync>;

pub trait TurnExecutor: Send + Sync {
    fn is_inference_ready(
        &self,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + '_>>;

    fn create_runner(
        &self,
        session: TurnExecutionValue,
        hooks: TurnExecutionValue,
    ) -> TurnExecutionValue;

    fn create_group_member_runner(
        &self,
        session: TurnExecutionValue,
        hooks: TurnExecutionValue,
        overrides: TurnExecutionValue,
    ) -> TurnExecutionValue;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnExecutionRegistryError {
    Unbound,
    AlreadyBound,
}

impl fmt::Display for TurnExecutionRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unbound => UNBOUND_EXECUTION_MESSAGE,
            Self::AlreadyBound => DOUBLE_BIND_MESSAGE,
        })
    }
}

impl std::error::Error for TurnExecutionRegistryError {}

#[derive(Default)]
pub struct TurnExecutionRegistry {
    executor: Option<Arc<dyn TurnExecutor>>,
}

impl TurnExecutionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn can_execute(&self) -> bool {
        self.executor.is_some()
    }

    pub fn bind_executor(
        &mut self,
        executor: Arc<dyn TurnExecutor>,
    ) -> Result<(), TurnExecutionRegistryError> {
        if self.executor.is_some() {
            return Err(TurnExecutionRegistryError::AlreadyBound);
        }
        self.executor = Some(executor);
        Ok(())
    }

    pub async fn is_run_ready(&self) -> bool {
        match self.executor.as_ref() {
            Some(executor) => executor.is_inference_ready().await,
            None => false,
        }
    }

    pub fn create_runner(
        &self,
        session: TurnExecutionValue,
        hooks: TurnExecutionValue,
    ) -> Result<TurnExecutionValue, TurnExecutionRegistryError> {
        Ok(self.require()?.create_runner(session, hooks))
    }

    pub fn create_group_member_runner(
        &self,
        session: TurnExecutionValue,
        hooks: TurnExecutionValue,
        overrides: TurnExecutionValue,
    ) -> Result<TurnExecutionValue, TurnExecutionRegistryError> {
        Ok(self
            .require()?
            .create_group_member_runner(session, hooks, overrides))
    }

    fn require(&self) -> Result<&dyn TurnExecutor, TurnExecutionRegistryError> {
        self.executor
            .as_deref()
            .ok_or(TurnExecutionRegistryError::Unbound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Executor {
        ready: bool,
        normal_calls: AtomicUsize,
        group_calls: AtomicUsize,
    }

    impl TurnExecutor for Executor {
        fn is_inference_ready(
            &self,
        ) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
            Box::pin(async move { self.ready })
        }

        fn create_runner(
            &self,
            _session: TurnExecutionValue,
            _hooks: TurnExecutionValue,
        ) -> TurnExecutionValue {
            self.normal_calls.fetch_add(1, Ordering::SeqCst);
            Arc::new("runner".to_owned())
        }

        fn create_group_member_runner(
            &self,
            _session: TurnExecutionValue,
            _hooks: TurnExecutionValue,
            _overrides: TurnExecutionValue,
        ) -> TurnExecutionValue {
            self.group_calls.fetch_add(1, Ordering::SeqCst);
            Arc::new("group-runner".to_owned())
        }
    }

    fn value<T: Any + Send + Sync>(value: T) -> TurnExecutionValue {
        Arc::new(value)
    }

    #[tokio::test]
    async fn unbound_registry_is_not_ready_and_fails_runner_creation_closed() {
        let registry = TurnExecutionRegistry::new();
        assert!(!registry.can_execute());
        assert!(!registry.is_run_ready().await);
        let error = match registry.create_runner(value("session"), value("hooks")) {
            Ok(_) => panic!("unbound registry unexpectedly created a runner"),
            Err(error) => error,
        };
        assert_eq!(error, TurnExecutionRegistryError::Unbound);
        assert_eq!(error.to_string(), UNBOUND_EXECUTION_MESSAGE);
    }

    #[tokio::test]
    async fn binds_once_and_forwards_all_executor_paths() {
        let executor = Arc::new(Executor {
            ready: true,
            normal_calls: AtomicUsize::new(0),
            group_calls: AtomicUsize::new(0),
        });
        let mut registry = TurnExecutionRegistry::new();
        registry.bind_executor(executor.clone()).unwrap();

        assert!(registry.can_execute());
        assert!(registry.is_run_ready().await);

        let runner = registry
            .create_runner(value("session"), value("hooks"))
            .unwrap();
        assert_eq!(
            runner.downcast::<String>().unwrap().as_str(),
            "runner"
        );
        let group = registry
            .create_group_member_runner(
                value("session"),
                value("hooks"),
                value("overrides"),
            )
            .unwrap();
        assert_eq!(
            group.downcast::<String>().unwrap().as_str(),
            "group-runner"
        );
        assert_eq!(executor.normal_calls.load(Ordering::SeqCst), 1);
        assert_eq!(executor.group_calls.load(Ordering::SeqCst), 1);

        let second = registry.bind_executor(executor).unwrap_err();
        assert_eq!(second, TurnExecutionRegistryError::AlreadyBound);
        assert_eq!(second.to_string(), DOUBLE_BIND_MESSAGE);
    }
}
