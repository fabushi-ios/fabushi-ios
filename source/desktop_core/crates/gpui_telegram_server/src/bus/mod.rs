pub mod event;
pub mod in_process;
pub mod redis_stub;

pub use event::{EventPayload, ServerEvent};
pub use in_process::InProcessEventBus;
pub use redis_stub::RedisEventBusStub;

use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::broadcast;

#[derive(Debug, Error)]
pub enum EventBusError {
    #[error("Serialization failed: {0}")]
    Serialization(String),

    #[error("Channel lagged: {0}")]
    ChannelLagged(u64),

    #[error("Channel closed")]
    ChannelClosed,

    #[error("Publish error: {0}")]
    PublishError(String),

    #[error("Connection error: {0}")]
    ConnectionError(String),
}

pub struct EventSubscription {
    receiver: broadcast::Receiver<ServerEvent>,
    channel: String,
}

impl EventSubscription {
    pub fn new(channel: String, receiver: broadcast::Receiver<ServerEvent>) -> Self {
        Self { receiver, channel }
    }

    pub fn channel(&self) -> &str {
        &self.channel
    }

    pub async fn recv(&mut self) -> Result<ServerEvent, EventBusError> {
        loop {
            match self.receiver.recv().await {
                Ok(evt) => return Ok(evt),
                Err(broadcast::error::RecvError::Lagged(count)) => {
                    // Log or return lagged error if strictly required, or return error
                    return Err(EventBusError::ChannelLagged(count));
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(EventBusError::ChannelClosed);
                }
            }
        }
    }
}

#[async_trait]
pub trait CrossNodeEventBus: Send + Sync {
    async fn publish(&self, channel: &str, event: &ServerEvent) -> Result<(), EventBusError>;
    async fn subscribe(&self, channel: &str) -> Result<EventSubscription, EventBusError>;
    fn node_id(&self) -> &str;
}
