use crate::package_agent_core_conversation_actions_receiver_contract::{
    ConversationActionReceiver, ConversationActionReceiverEntry,
};

// The pinned module also retains creation of this default manager logger.
#[allow(dead_code)]
const REMOTE_MANAGER_LOGGER_NAME: &str = "RemoteConversationActionManager";

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopConversationActionReceiver;

impl<Context, Action> ConversationActionReceiver<Context, Action>
    for NoopConversationActionReceiver
{
    type ContextInjectionToolSignal = ();

    async fn peek(
        &mut self,
        _context: &Context,
    ) -> Option<ConversationActionReceiverEntry<Action>> {
        None
    }

    async fn pop(&mut self, _context: &Context) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_retained_remote_manager_logger_identity() {
        assert_eq!(REMOTE_MANAGER_LOGGER_NAME, "RemoteConversationActionManager");
    }

    #[tokio::test]
    async fn noop_receiver_never_surfaces_or_consumes_an_action() {
        let mut receiver = NoopConversationActionReceiver;

        let peek =
            <NoopConversationActionReceiver as ConversationActionReceiver<(), String>>::peek(
                &mut receiver,
                &(),
            )
            .await;
        assert_eq!(peek, None);

        <NoopConversationActionReceiver as ConversationActionReceiver<(), String>>::pop(
            &mut receiver,
            &(),
        )
        .await;

        assert_eq!(
            <NoopConversationActionReceiver as ConversationActionReceiver<(), String>>::peek_is_claimed_injection(
                &receiver,
            ),
            None
        );
        assert!(
            <NoopConversationActionReceiver as ConversationActionReceiver<(), String>>::get_context_injection_tool_signal(
                &receiver,
            )
            .is_none()
        );
    }
}
