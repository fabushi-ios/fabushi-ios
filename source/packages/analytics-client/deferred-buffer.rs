use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct DeferredAnalyticsEvent {
    pub event_name: String,
    pub props: Option<Map<String, Value>>,
    pub timestamp: Option<f64>,
}

#[derive(Debug, Default)]
pub struct DeferredAnalyticsBuffer {
    events: Vec<DeferredAnalyticsEvent>,
}

impl DeferredAnalyticsBuffer {
    pub fn track(
        &mut self,
        event_name: impl Into<String>,
        props: Option<Map<String, Value>>,
        timestamp: Option<f64>,
    ) {
        self.events.push(DeferredAnalyticsEvent {
            event_name: event_name.into(),
            props,
            timestamp,
        });
    }

    pub async fn flush(&mut self, _timeout_ms: u64) {}

    pub fn get_events(&self) -> &[DeferredAnalyticsEvent] {
        &self.events
    }

    pub fn clear(&mut self) {
        self.events.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn queues_in_order_flushes_as_noop_and_clears() {
        let mut buffer = DeferredAnalyticsBuffer::default();
        let mut props = Map::new();
        props.insert("screen".to_owned(), json!("home"));

        buffer.track("opened", Some(props.clone()), Some(10.5));
        buffer.track("closed", None, None);

        assert_eq!(
            buffer.get_events(),
            &[
                DeferredAnalyticsEvent {
                    event_name: "opened".to_owned(),
                    props: Some(props),
                    timestamp: Some(10.5),
                },
                DeferredAnalyticsEvent {
                    event_name: "closed".to_owned(),
                    props: None,
                    timestamp: None,
                },
            ]
        );

        buffer.flush(1).await;
        assert_eq!(buffer.get_events().len(), 2);

        buffer.clear();
        assert!(buffer.get_events().is_empty());
    }
}
