use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxEnvironmentUpdate {
    pub env: BTreeMap<String, String>,
    pub replace: bool,
}

impl BoxEnvironmentUpdate {
    pub fn new(
        env: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
        replace: bool,
    ) -> Self {
        Self {
            env: env
                .into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
            replace,
        }
    }
}

pub trait BoxEnvironmentControlClient<Context> {
    type Error;

    async fn update_environment_variables(
        &mut self,
        context: &Context,
        request: BoxEnvironmentUpdate,
    ) -> Result<(), Self::Error>;
}

/// iOS adaptation of Grok's box environment transport helper.
///
/// The helper only forwards an explicit environment capability request to the
/// provided Remote Runner transport. It never mutates the iOS process
/// environment and never starts a local process.
pub async fn apply_box_environment_via_transport<Context, Transport, Client, CreateClient>(
    context: &Context,
    transport: Transport,
    update: &BoxEnvironmentUpdate,
    create_client: CreateClient,
) -> Result<(), Client::Error>
where
    Client: BoxEnvironmentControlClient<Context>,
    CreateClient: FnOnce(Transport) -> Client,
{
    let mut client = create_client(transport);
    client
        .update_environment_variables(context, update.clone())
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder {
        received: Option<BoxEnvironmentUpdate>,
    }

    impl BoxEnvironmentControlClient<&'static str> for Recorder {
        type Error = ();

        async fn update_environment_variables(
            &mut self,
            _context: &&'static str,
            request: BoxEnvironmentUpdate,
        ) -> Result<(), Self::Error> {
            self.received = Some(request);
            Ok(())
        }
    }

    #[tokio::test]
    async fn forwards_a_copy_of_the_update_to_transport() {
        let update = BoxEnvironmentUpdate::new(
            [("TOKEN", "redacted"), ("MODE", "safe")],
            true,
        );
        let captured = std::sync::Arc::new(std::sync::Mutex::new(None));
        let captured_for_client = captured.clone();

        struct SharedRecorder(
            std::sync::Arc<std::sync::Mutex<Option<BoxEnvironmentUpdate>>>,
        );
        impl BoxEnvironmentControlClient<()> for SharedRecorder {
            type Error = ();
            async fn update_environment_variables(
                &mut self,
                _context: &(),
                request: BoxEnvironmentUpdate,
            ) -> Result<(), Self::Error> {
                *self.0.lock().expect("recorder lock") = Some(request);
                Ok(())
            }
        }

        apply_box_environment_via_transport(
            &(),
            "remote-runner",
            &update,
            |_| SharedRecorder(captured_for_client),
        )
        .await
        .unwrap();

        assert_eq!(
            captured.lock().expect("recorder lock").as_ref(),
            Some(&update)
        );
    }

    #[test]
    fn update_has_deterministic_key_order_for_wire_encoding() {
        let update = BoxEnvironmentUpdate::new(
            [("Z", "last"), ("A", "first")],
            false,
        );
        assert_eq!(
            update.env.keys().cloned().collect::<Vec<_>>(),
            vec!["A".to_owned(), "Z".to_owned()]
        );
    }
}
