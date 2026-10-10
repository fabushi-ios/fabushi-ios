use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelActionRoutingError;

impl fmt::Display for CancelActionRoutingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Cancel Conversation action should never be routed directly to runStream!")
    }
}

impl std::error::Error for CancelActionRoutingError {}

#[derive(Debug)]
pub struct CancelActionHandler<Config, ResourceAccessor, InteractionListener, SummarizationHandler, ConversationActionReceiver> {
    pub config: Config,
    pub resource_accessor: ResourceAccessor,
    pub interaction_listener: InteractionListener,
    pub summarization_handler: SummarizationHandler,
    pub conversation_action_receiver: ConversationActionReceiver,
}

impl<Config, ResourceAccessor, InteractionListener, SummarizationHandler, ConversationActionReceiver>
    CancelActionHandler<Config, ResourceAccessor, InteractionListener, SummarizationHandler, ConversationActionReceiver>
{
    pub fn new(
        config: Config,
        resource_accessor: ResourceAccessor,
        interaction_listener: InteractionListener,
        summarization_handler: SummarizationHandler,
        conversation_action_receiver: ConversationActionReceiver,
    ) -> Self {
        Self {
            config,
            resource_accessor,
            interaction_listener,
            summarization_handler,
            conversation_action_receiver,
        }
    }

    pub async fn handle<Output>(&self) -> Result<Output, CancelActionRoutingError> {
        Err(CancelActionRoutingError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn direct_cancel_route_is_always_rejected() {
        let handler = CancelActionHandler::new(1, 2, 3, 4, 5);
        let error = handler.handle::<()>().await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Cancel Conversation action should never be routed directly to runStream!"
        );
    }
}
