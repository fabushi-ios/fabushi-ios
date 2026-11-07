use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxEnvironmentUpdate {
    pub env: BTreeMap<String, String>,
    pub replace: bool,
}

pub trait BoxEnvironmentControlClient<Ctx> {
    type Error;

    fn update_environment_variables(
        &mut self,
        ctx: &Ctx,
        request: BoxEnvironmentUpdate,
    ) -> Result<(), Self::Error>;
}

pub fn apply_box_environment_via_transport<Ctx, Transport, Client, Factory>(
    ctx: &Ctx,
    transport: &Transport,
    update: &BoxEnvironmentUpdate,
    create_client: Factory,
) -> Result<(), Client::Error>
where
    Client: BoxEnvironmentControlClient<Ctx>,
    Factory: FnOnce(&Transport) -> Client,
{
    let mut control = create_client(transport);
    control.update_environment_variables(
        ctx,
        BoxEnvironmentUpdate {
            env: update.env.clone(),
            replace: update.replace,
        },
    )
}
