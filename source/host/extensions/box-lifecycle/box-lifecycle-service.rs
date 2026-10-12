#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

pub trait BoxLifecycleClient {
    type Error;
    type AbortSignal;

    async fn get_sand_box_run_state(
        &self,
        signal: &Self::AbortSignal,
    ) -> Result<BoxRunState, Self::Error>;

    async fn recreate_sand_box(
        &self,
        request: RecreateSandBoxRequest,
    ) -> Result<RecreateSandBoxResponse, Self::Error>;
}

pub struct BoxLifecycleService<Client> {
    client: Client,
}

impl<Client> BoxLifecycleService<Client>
where
    Client: BoxLifecycleClient,
{
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    pub async fn fetch_image_update_available(
        &self,
        signal: &Client::AbortSignal,
    ) -> Result<bool, Client::Error> {
        Ok(self
            .client
            .get_sand_box_run_state(signal)
            .await?
            .image_update_available)
    }

    pub async fn recreate_in_box(
        &self,
        preserve_data: bool,
        force: Option<bool>,
    ) -> Result<RecreateSandBoxResponse, Client::Error> {
        self.client
            .recreate_sand_box(RecreateSandBoxRequest {
                preserve_data,
                force: force == Some(true),
            })
            .await
    }

    pub fn client(&self) -> &Client {
        &self.client
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Client {
        requests: Mutex<Vec<RecreateSandBoxRequest>>,
    }

    impl BoxLifecycleClient for Client {
        type Error = Infallible;
        type AbortSignal = bool;

        async fn get_sand_box_run_state(
            &self,
            signal: &Self::AbortSignal,
        ) -> Result<BoxRunState, Self::Error> {
            Ok(BoxRunState {
                image_update_available: *signal,
            })
        }

        async fn recreate_sand_box(
            &self,
            request: RecreateSandBoxRequest,
        ) -> Result<RecreateSandBoxResponse, Self::Error> {
            self.requests.lock().unwrap().push(request);
            Ok(RecreateSandBoxResponse {
                started: true,
                reason: Some("queued".into()),
            })
        }
    }

    #[tokio::test]
    async fn forwards_run_state_signal_and_boolean_result() {
        let service = BoxLifecycleService::new(Client::default());
        assert!(service.fetch_image_update_available(&true).await.unwrap());
        assert!(!service.fetch_image_update_available(&false).await.unwrap());
    }

    #[tokio::test]
    async fn recreates_with_reference_force_defaulting_and_reason_projection() {
        let service = BoxLifecycleService::new(Client::default());
        let response = service.recreate_in_box(true, None).await.unwrap();
        assert_eq!(
            response,
            RecreateSandBoxResponse {
                started: true,
                reason: Some("queued".into()),
            }
        );
        service.recreate_in_box(false, Some(true)).await.unwrap();

        assert_eq!(
            service.client().requests.lock().unwrap().as_slice(),
            &[
                RecreateSandBoxRequest {
                    preserve_data: true,
                    force: false,
                },
                RecreateSandBoxRequest {
                    preserve_data: false,
                    force: true,
                },
            ]
        );
    }
}
