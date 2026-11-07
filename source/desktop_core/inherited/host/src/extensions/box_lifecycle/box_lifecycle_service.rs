use std::future::Future;
use std::pin::Pin;

pub type BoxLifecycleFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxRunState {
    pub image_update_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecreateSandBoxRequest {
    pub preserve_data: bool,
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecreateSandBoxResponse {
    pub started: bool,
    pub reason: Option<String>,
}

pub trait BoxLifecycleClient<Signal> {
    type Error;

    fn get_sand_box_run_state<'a>(
        &'a self,
        signal: &'a Signal,
    ) -> BoxLifecycleFuture<'a, Result<BoxRunState, Self::Error>>;

    fn recreate_sand_box<'a>(
        &'a self,
        request: RecreateSandBoxRequest,
    ) -> BoxLifecycleFuture<'a, Result<RecreateSandBoxResponse, Self::Error>>;
}

pub struct BoxLifecycleService<Client> {
    client: Client,
}

impl<Client> BoxLifecycleService<Client> {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    pub fn client(&self) -> &Client {
        &self.client
    }
}

impl<Client> BoxLifecycleService<Client> {
    pub async fn fetch_image_update_available<Signal>(
        &self,
        signal: &Signal,
    ) -> Result<bool, Client::Error>
    where
        Client: BoxLifecycleClient<Signal>,
    {
        Ok(self.client.get_sand_box_run_state(signal).await?.image_update_available)
    }

    pub async fn recreate_in_box<Signal>(
        &self,
        preserve_data: bool,
        force: Option<bool>,
    ) -> Result<RecreateSandBoxResponse, Client::Error>
    where
        Client: BoxLifecycleClient<Signal>,
    {
        self.client
            .recreate_sand_box(RecreateSandBoxRequest {
                preserve_data,
                force: force == Some(true),
            })
            .await
    }
}
