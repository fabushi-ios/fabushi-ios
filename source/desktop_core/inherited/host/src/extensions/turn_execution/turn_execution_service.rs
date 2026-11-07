use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;

pub const UNBOUND_EXECUTION_MESSAGE: &str = "Sand turn execution is not bound: the host asked for a runner before the composition root handed the turn-execution extension its executor.";
pub const DOUBLE_BIND_MESSAGE: &str = "Sand turn execution is already bound: a second executor would mint a second runner for the same agent.";

pub trait TurnExecutor: Send + Sync + 'static {
    fn is_inference_ready(&self) -> Pin<Box<dyn Future<Output = bool> + Send + '_>>;
    fn create_runner(&self, session: Value, hooks: Value) -> Value;
    fn create_group_member_runner(&self, session: Value, hooks: Value, overrides: Value) -> Value;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TurnExecutionError {
    #[error("{UNBOUND_EXECUTION_MESSAGE}")]
    Unbound,
    #[error("{DOUBLE_BIND_MESSAGE}")]
    DoubleBind,
}

#[derive(Default)]
pub struct TurnExecutionRegistry {
    executor: Option<Arc<dyn TurnExecutor>>,
}

impl TurnExecutionRegistry {
    pub fn can_execute(&self) -> bool {
        self.executor.is_some()
    }

    pub fn executor_handle(&self) -> Option<Arc<dyn TurnExecutor>> {
        self.executor.clone()
    }

    pub fn bind_executor(&mut self, executor: Box<dyn TurnExecutor>) -> Result<(), TurnExecutionError> {
        if self.executor.is_some() {
            return Err(TurnExecutionError::DoubleBind);
        }
        self.executor = Some(Arc::from(executor));
        Ok(())
    }

    pub async fn is_run_ready(&self) -> bool {
        let Some(executor) = self.executor_handle() else {
            return false;
        };
        executor.is_inference_ready().await
    }

    pub fn create_runner(&self, session: Value, hooks: Value) -> Result<Value, TurnExecutionError> {
        Ok(self.require()?.create_runner(session, hooks))
    }

    pub fn create_group_member_runner(
        &self,
        session: Value,
        hooks: Value,
        overrides: Value,
    ) -> Result<Value, TurnExecutionError> {
        Ok(self.require()?.create_group_member_runner(session, hooks, overrides))
    }

    fn require(&self) -> Result<Arc<dyn TurnExecutor>, TurnExecutionError> {
        self.executor_handle().ok_or(TurnExecutionError::Unbound)
    }
}
