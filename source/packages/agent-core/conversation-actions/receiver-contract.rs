#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationActionReceiverEntry<Action> {
    pub action: Action,
}

#[allow(async_fn_in_trait)]
pub trait ConversationActionReceiver<Context, Action> {
    type ContextInjectionToolSignal;

    async fn peek(
        &mut self,
        context: &Context,
    ) -> Option<ConversationActionReceiverEntry<Action>>;

    async fn pop(&mut self, context: &Context);

    fn peek_is_claimed_injection(&self) -> Option<bool> {
        None
    }

    fn fail_consumed_injection_delivery(&mut self) {}

    fn get_context_injection_tool_signal(
        &self,
    ) -> Option<&Self::ContextInjectionToolSignal> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ProbeReceiver {
        queued: Option<&'static str>,
        failed_delivery: bool,
        signal: Option<&'static str>,
    }

    impl ConversationActionReceiver<(), &'static str> for ProbeReceiver {
        type ContextInjectionToolSignal = &'static str;

        async fn peek(
            &mut self,
            _context: &(),
        ) -> Option<ConversationActionReceiverEntry<&'static str>> {
            self.queued
                .map(|action| ConversationActionReceiverEntry { action })
        }

        async fn pop(&mut self, _context: &()) {
            self.queued = None;
        }

        fn peek_is_claimed_injection(&self) -> Option<bool> {
            Some(true)
        }

        fn fail_consumed_injection_delivery(&mut self) {
            self.failed_delivery = true;
        }

        fn get_context_injection_tool_signal(
            &self,
        ) -> Option<&Self::ContextInjectionToolSignal> {
            self.signal.as_ref()
        }
    }

    #[tokio::test]
    async fn preserves_receiver_entry_and_optional_injection_hooks() {
        let mut receiver = ProbeReceiver {
            queued: Some("steer"),
            failed_delivery: false,
            signal: Some("tool-signal"),
        };

        assert_eq!(
            receiver.peek(&()).await,
            Some(ConversationActionReceiverEntry { action: "steer" })
        );
        assert_eq!(receiver.peek_is_claimed_injection(), Some(true));
        assert_eq!(
            receiver.get_context_injection_tool_signal().copied(),
            Some("tool-signal")
        );

        receiver.fail_consumed_injection_delivery();
        assert!(receiver.failed_delivery);

        receiver.pop(&()).await;
        assert_eq!(receiver.peek(&()).await, None);
    }
}
