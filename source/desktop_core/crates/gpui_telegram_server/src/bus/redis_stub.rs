use super::{CrossNodeEventBus, EventBusError, EventSubscription, InProcessEventBus, ServerEvent};
use async_trait::async_trait;
use std::sync::Arc;

#[derive(Clone)]
pub struct RedisEventBusStub {
    node_id: String,
    redis_url: String,
    broker: Arc<InProcessEventBus>,
}

impl RedisEventBusStub {
    pub fn new(node_id: impl Into<String>, redis_url: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            redis_url: redis_url.into(),
            broker: Arc::new(InProcessEventBus::new("redis-sim-broker")),
        }
    }

    pub fn with_shared_broker(
        node_id: impl Into<String>,
        redis_url: impl Into<String>,
        broker: Arc<InProcessEventBus>,
    ) -> Self {
        Self {
            node_id: node_id.into(),
            redis_url: redis_url.into(),
            broker,
        }
    }

    pub fn redis_url(&self) -> &str {
        &self.redis_url
    }
}

#[async_trait]
impl CrossNodeEventBus for RedisEventBusStub {
    async fn publish(&self, channel: &str, event: &ServerEvent) -> Result<(), EventBusError> {
        // Enforce genuine wire serialization to test Redis JSON wire contract
        let json_bytes = serde_json::to_vec(event)
            .map_err(|e| EventBusError::Serialization(e.to_string()))?;

        let deserialized: ServerEvent = serde_json::from_slice(&json_bytes)
            .map_err(|e| EventBusError::Serialization(e.to_string()))?;

        self.broker.publish(channel, &deserialized).await
    }

    async fn subscribe(&self, channel: &str) -> Result<EventSubscription, EventBusError> {
        self.broker.subscribe(channel).await
    }

    fn node_id(&self) -> &str {
        &self.node_id
    }
}
