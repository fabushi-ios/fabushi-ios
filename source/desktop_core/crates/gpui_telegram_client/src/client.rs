use crate::cache::LocalStore;
use crate::config::ClientConfig;
use crate::net::{ConnectionStatus, HttpClient, WsChannel, WsTransport};
use crate::sync::ClientSyncCoordinator;
use gpui_telegram_core::error::Result;
use gpui_telegram_core::types::ChatId;
use gpui_telegram_proto::message::MessageItem;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;

pub struct TelegramClient {
    pub config: ClientConfig,
    pub local_store: LocalStore,
    pub sync_coordinator: ClientSyncCoordinator,
    pub http_client: HttpClient,
    pub ws_channel: WsChannel,
    pub ws_transport: WsTransport,
    runtime: Arc<Runtime>,
}

impl TelegramClient {
    pub fn new(config: ClientConfig) -> Self {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("gpui-tokio-client")
                .enable_all()
                .build()
                .expect("Failed to initialize client Tokio runtime"),
        );

        let handle = runtime.handle().clone();
        let http_client = HttpClient::new_with_runtime(
            config.server_url.clone(),
            Duration::from_secs(config.http_timeout_secs),
            Some(handle.clone()),
        );
        let ws_channel = WsChannel::new();
        let ws_transport = WsTransport::new_with_runtime(
            config.server_url.clone(),
            ws_channel.clone(),
            Some(handle),
        );

        Self {
            config,
            local_store: LocalStore::new(),
            sync_coordinator: ClientSyncCoordinator::new(),
            http_client,
            ws_channel,
            ws_transport,
            runtime,
        }
    }

    pub fn new_with_url(server_url: String) -> Self {
        let config = ClientConfig {
            server_url,
            ..Default::default()
        };
        Self::new(config)
    }

    pub fn runtime(&self) -> &Arc<Runtime> {
        &self.runtime
    }

    pub fn status(&self) -> ConnectionStatus {
        self.ws_transport.status()
    }

    pub fn is_connected(&self) -> bool {
        self.ws_transport.is_connected()
    }

    pub fn get_messages(&self, chat_id: ChatId) -> Result<Vec<MessageItem>> {
        self.local_store.get_messages(chat_id)
    }
}
