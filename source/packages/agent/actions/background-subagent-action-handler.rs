use super::package_agent_actions_background_shell_action_handler::{
    NoOpSideChannelActionHandler, NoOpSingleStep,
};
use std::future::Future;

#[derive(Debug, Default, Clone, Copy)]
pub struct BackgroundSubagentActionHandler {
    inner: NoOpSideChannelActionHandler,
}

impl BackgroundSubagentActionHandler {
    pub async fn handle<Context, State, Compute, ComputeFuture>(&self, ctx: Context, compute: Compute) -> State
    where
        Compute: FnOnce(Context) -> ComputeFuture,
        ComputeFuture: Future<Output = State>,
    {
        self.inner.handle(ctx, compute).await
    }

    pub async fn handle_single_step<Context, State, Compute, ComputeFuture>(
        &self,
        ctx: Context,
        compute: Compute,
    ) -> NoOpSingleStep<State>
    where
        Compute: FnOnce(Context) -> ComputeFuture,
        ComputeFuture: Future<Output = State>,
    {
        self.inner.handle_single_step(ctx, compute).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn subagent_handler_inherits_noop_side_channel_contract() {
        let handler = BackgroundSubagentActionHandler::default();
        let step = handler.handle_single_step(7, |ctx| async move { ctx * 3 }).await;
        assert_eq!(step.state, 21);
        assert!(!step.has_tool_call);
    }
}
