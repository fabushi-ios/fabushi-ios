use super::{CrossNodeEventBus, EventBusError, EventSubscription, ServerEvent};
use async_trait::async_trait;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;

const DEFAULT_CHANNEL_CAPACITY: usize = 16384;

#[derive(Clone)]
pub struct InProcessEventBus {
    node_id: String,
    channels: Arc<RwLock<HashMap<String, broadcast::Sender<ServerEvent>>>>,
    wildcard_channels: Arc<RwLock<HashMap<String, broadcast::Sender<ServerEvent>>>>,
    capacity: usize,
}

impl InProcessEventBus {
    pub fn new(node_id: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            channels: Arc::new(RwLock::new(HashMap::new())),
            wildcard_channels: Arc::new(RwLock::new(HashMap::new())),
            capacity: DEFAULT_CHANNEL_CAPACITY,
        }
    }

    pub fn with_capacity(node_id: impl Into<String>, capacity: usize) -> Self {
        Self {
            node_id: node_id.into(),
            channels: Arc::new(RwLock::new(HashMap::new())),
            wildcard_channels: Arc::new(RwLock::new(HashMap::new())),
            capacity: capacity.max(16),
        }
    }

    pub fn active_channel_count(&self) -> usize {
        self.prune_dead_channels();
        self.channels.read().len() + self.wildcard_channels.read().len()
    }

    pub fn prune_dead_channels(&self) {
        {
            let mut map = self.channels.write();
            map.retain(|_, tx| tx.receiver_count() > 0);
        }
        {
            let mut wmap = self.wildcard_channels.write();
            wmap.retain(|_, tx| tx.receiver_count() > 0);
        }
    }

    fn subscribe_in_map(
        &self,
        map_lock: &RwLock<HashMap<String, broadcast::Sender<ServerEvent>>>,
        key: &str,
    ) -> broadcast::Receiver<ServerEvent> {
        let mut map = map_lock.write();
        if map.len() > 256 {
            map.retain(|_, tx| tx.receiver_count() > 0);
        }
        if let Some(existing) = map.get(key) {
            if existing.receiver_count() > 0 {
                return existing.subscribe();
            }
        }
        let (tx, rx) = broadcast::channel(self.capacity);
        map.insert(key.to_string(), tx);
        rx
    }
}

#[async_trait]
impl CrossNodeEventBus for InProcessEventBus {
    async fn publish(&self, channel: &str, event: &ServerEvent) -> Result<(), EventBusError> {
        let mut targets: Vec<broadcast::Sender<ServerEvent>> = Vec::new();
        let mut saw_dead_exact = false;
        let mut saw_dead_wild = false;

        {
            let map = self.channels.read();
            if let Some(tx) = map.get(channel) {
                if tx.receiver_count() > 0 {
                    targets.push(tx.clone());
                } else {
                    saw_dead_exact = true;
                }
            }
        }

        {
            let wmap = self.wildcard_channels.read();
            for (prefix, tx) in wmap.iter() {
                if channel.starts_with(prefix) {
                    if tx.receiver_count() > 0 {
                        targets.push(tx.clone());
                    } else {
                        saw_dead_wild = true;
                    }
                }
            }
        }

        for tx in targets {
            let _ = tx.send(event.clone());
        }

        if saw_dead_exact {
            self.channels.write().retain(|_, tx| tx.receiver_count() > 0);
        }
        if saw_dead_wild {
            self.wildcard_channels
                .write()
                .retain(|_, tx| tx.receiver_count() > 0);
        }

        Ok(())
    }

    async fn subscribe(&self, channel: &str) -> Result<EventSubscription, EventBusError> {
        let rx = if let Some(prefix) = channel.strip_suffix('*') {
            self.subscribe_in_map(&self.wildcard_channels, prefix)
        } else {
            self.subscribe_in_map(&self.channels, channel)
        };
        Ok(EventSubscription::new(channel.to_string(), rx))
    }

    fn node_id(&self) -> &str {
        &self.node_id
    }
}
