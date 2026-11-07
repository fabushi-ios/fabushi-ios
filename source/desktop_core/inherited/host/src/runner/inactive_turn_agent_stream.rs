use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;

use super::{
    OuterStreamPersistence, persist_outer_stream_checkpoint,
    persist_outer_stream_final_state, release_outer_stream_persistence,
};

/// Generated Agent streams execute inside the already-owned Runner worker.
/// They intentionally keep scoped borrows (for example the live renderer
/// delta callback), so the future itself does not claim a second cross-thread
/// ownership boundary.
pub type InactiveTurnStreamFuture<'a, T> =
    Pin<Box<dyn Future<Output = T> + 'a>>;

pub trait InactiveTurnAgentOutputSink {
    fn on_text_delta(&mut self, delta: &str, accumulated: &str);
}

#[derive(Default)]
pub struct NoopInactiveTurnAgentOutputSink;

impl InactiveTurnAgentOutputSink for NoopInactiveTurnAgentOutputSink {
    fn on_text_delta(&mut self, _delta: &str, _accumulated: &str) {}
}

/// Checkpoint callback boundary used by the generated Agent stream.
///
/// The frozen Grok owner exposes an async persist callback. Keep that shape in
/// Rust instead of nesting an executor: a checkpoint is not eligible for
/// resume until this future has settled successfully.
pub trait InactiveTurnCheckpointSink<Context, State> {
    fn persist<'a>(
        &'a mut self,
        context: &'a Context,
        checkpoint: &'a mut State,
    ) -> InactiveTurnStreamFuture<'a, Result<(), String>>;
}

pub trait InactiveTurnAgentStreamSource<Context, State>: Send + Sync {
    type Error: From<String>;

    fn start_stream<'a>(
        &'a self,
        context: &'a Context,
        resume_from: Option<&'a State>,
        persist_checkpoint: &'a mut dyn InactiveTurnCheckpointSink<Context, State>,
        output: &'a mut dyn InactiveTurnAgentOutputSink,
    ) -> InactiveTurnStreamFuture<'a, Result<State, Self::Error>>;
}

pub trait InactiveTurnAgentLifecycleHooks<State>: Send + Sync {
    fn on_completed<'a>(
        &'a self,
        _state: &'a State,
    ) -> InactiveTurnStreamFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }

    fn cleanup<'a>(&'a self) -> InactiveTurnStreamFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Debug, Default)]
pub struct NoopInactiveTurnAgentLifecycleHooks;

impl<State> InactiveTurnAgentLifecycleHooks<State> for NoopInactiveTurnAgentLifecycleHooks {}

pub struct InactiveTurnAgentStreamPath<Context, State, Error = String>
where
    Error: From<String>,
{
    source: Arc<
        dyn InactiveTurnAgentStreamSource<Context, State, Error = Error>,
    >,
    _marker: PhantomData<fn(Context, State, Error)>,
}

impl<Context, State, Error> Clone
    for InactiveTurnAgentStreamPath<Context, State, Error>
where
    Error: From<String>,
{
    fn clone(&self) -> Self {
        Self {
            source: Arc::clone(&self.source),
            _marker: PhantomData,
        }
    }
}

impl<Context, State, Error> InactiveTurnAgentStreamPath<Context, State, Error>
where
    Error: From<String>,
{
    pub fn new(
        source: Arc<
            dyn InactiveTurnAgentStreamSource<Context, State, Error = Error>,
        >,
    ) -> Self {
        Self {
            source,
            _marker: PhantomData,
        }
    }

    pub async fn start_stream(
        &self,
        context: &Context,
        resume_from: Option<&State>,
        persist_checkpoint: &mut dyn InactiveTurnCheckpointSink<Context, State>,
        output: &mut dyn InactiveTurnAgentOutputSink,
    ) -> Result<State, Error> {
        self.source
            .start_stream(context, resume_from, persist_checkpoint, output)
            .await
    }

    pub async fn run_lifecycle<P, H>(
        &self,
        context: &Context,
        resume_from: Option<&State>,
        persistence: &P,
        hooks: &H,
    ) -> Result<State, Error>
    where
        P: OuterStreamPersistence<Context, State>,
        H: InactiveTurnAgentLifecycleHooks<State>,
    {
        let mut output = NoopInactiveTurnAgentOutputSink;
        self.run_lifecycle_with_output(
            context,
            resume_from,
            persistence,
            hooks,
            &mut output,
        )
        .await
    }

    /// Own one generated-Agent stream lifecycle on the shipping Runner worker.
    ///
    /// This mirrors createOuterStreamLifecycle: generation-gated step
    /// checkpoints are drained synchronously, completion hooks run before
    /// cleanup, final state is persisted only for a completed stream, and the
    /// disk-pressure claim is released exactly once on every exit path.
    pub async fn run_lifecycle_with_output<P, H>(
        &self,
        context: &Context,
        resume_from: Option<&State>,
        persistence: &P,
        hooks: &H,
        output: &mut dyn InactiveTurnAgentOutputSink,
    ) -> Result<State, Error>
    where
        P: OuterStreamPersistence<Context, State>,
        H: InactiveTurnAgentLifecycleHooks<State>,
    {
        struct OuterCheckpointSink<'a, P> {
            persistence: &'a P,
        }

        impl<P, Context, State> InactiveTurnCheckpointSink<Context, State>
            for OuterCheckpointSink<'_, P>
        where
            P: OuterStreamPersistence<Context, State>,
        {
            fn persist<'a>(
                &'a mut self,
                checkpoint_context: &'a Context,
                checkpoint: &'a mut State,
            ) -> InactiveTurnStreamFuture<'a, Result<(), String>> {
                Box::pin(async move {
                    persist_outer_stream_checkpoint(
                        self.persistence,
                        checkpoint_context,
                        checkpoint,
                    )
                    .await
                    .map(|_| ())
                })
            }
        }

        let mut persist = OuterCheckpointSink { persistence };
        let stream_result = self
            .start_stream(context, resume_from, &mut persist, output)
            .await;

        let result = match stream_result {
            Ok(final_state) => {
                let completion = hooks.on_completed(&final_state).await;
                let cleanup = hooks.cleanup().await;
                let final_persist = if completion.is_ok() && cleanup.is_ok() {
                    persist_outer_stream_final_state(
                        persistence,
                        context,
                        &final_state,
                    )
                    .await
                    .map(|_| ())
                } else {
                    Ok(())
                };

                if let Err(error) = completion {
                    Err(Error::from(error))
                } else if let Err(error) = cleanup {
                    Err(Error::from(error))
                } else if let Err(error) = final_persist {
                    Err(Error::from(error))
                } else {
                    Ok(final_state)
                }
            }
            Err(error) => {
                let cleanup = hooks.cleanup().await;
                match cleanup {
                    Ok(()) => Err(error),
                    Err(cleanup_error) => Err(Error::from(cleanup_error)),
                }
            }
        };

        release_outer_stream_persistence(persistence);
        result
    }
}
