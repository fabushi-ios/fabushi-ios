use std::future::Future;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoOpSingleStep<State> {
    pub state: State,
    pub has_tool_call: bool,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoOpSideChannelActionHandler;

impl NoOpSideChannelActionHandler {
    pub async fn handle<Context, State, Compute, ComputeFuture>(
        &self,
        ctx: Context,
        compute_new_structure: Compute,
    ) -> State
    where
        Compute: FnOnce(Context) -> ComputeFuture,
        ComputeFuture: Future<Output = State>,
    {
        compute_new_structure(ctx).await
    }

    pub async fn handle_single_step<Context, State, Compute, ComputeFuture>(
        &self,
        ctx: Context,
        compute_new_structure: Compute,
    ) -> NoOpSingleStep<State>
    where
        Compute: FnOnce(Context) -> ComputeFuture,
        ComputeFuture: Future<Output = State>,
    {
        NoOpSingleStep {
            state: compute_new_structure(ctx).await,
            has_tool_call: false,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct BackgroundShellActionHandler {
    inner: NoOpSideChannelActionHandler,
}

impl BackgroundShellActionHandler {
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
    async fn side_channel_handler_only_recomputes_structure() {
        let handler = BackgroundShellActionHandler::default();
        let state = handler.handle(41_u32, |ctx| async move { ctx + 1 }).await;
        assert_eq!(state, 42);
        let step = handler.handle_single_step("ctx", |ctx| async move { format!("{ctx}-state") }).await;
        assert_eq!(step.state, "ctx-state");
        assert!(!step.has_tool_call);
    }
}
