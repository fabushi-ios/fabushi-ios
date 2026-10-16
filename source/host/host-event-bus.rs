use crate::host_extensions::{
    HostEventFailureMode, HostEventSubscription, HostEvents,
};
use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, Weak};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEventBusFailure {
    SubscriberFailed {
        topic: String,
        error_class: String,
    },
    ListenerFailed {
        error_class: String,
    },
}

type LegacyListener<Event> =
    Arc<dyn Fn(&Event) -> Result<(), String> + Send + Sync + 'static>;

struct LegacyListeners<Event> {
    next_id: u64,
    listeners: BTreeMap<u64, LegacyListener<Event>>,
}

impl<Event> Default for LegacyListeners<Event> {
    fn default() -> Self {
        Self {
            next_id: 0,
            listeners: BTreeMap::new(),
        }
    }
}

pub struct HostEventListenerSubscription<Event> {
    listeners: Weak<Mutex<LegacyListeners<Event>>>,
    id: u64,
}

impl<Event> HostEventListenerSubscription<Event> {
    pub fn unsubscribe(&self) {
        let Some(listeners) = self.listeners.upgrade() else {
            return;
        };
        listeners.lock().unwrap().listeners.remove(&self.id);
    }
}

pub struct SandHostEventBus<Event> {
    listeners: Arc<Mutex<LegacyListeners<Event>>>,
    capability_events: HostEvents,
    report_failure: Arc<dyn Fn(HostEventBusFailure) + Send + Sync>,
}

impl<Event> SandHostEventBus<Event>
where
    Event: Send + Sync + 'static,
{
    pub fn new<Reporter>(report_failure: Reporter) -> Self
    where
        Reporter: Fn(HostEventBusFailure) + Send + Sync + 'static,
    {
        let report_failure: Arc<dyn Fn(HostEventBusFailure) + Send + Sync> =
            Arc::new(report_failure);
        let capability_reporter = report_failure.clone();
        let capability_events = HostEvents::new(move |topic, error| {
            capability_reporter(HostEventBusFailure::SubscriberFailed {
                topic: topic.to_owned(),
                error_class: error.to_owned(),
            });
        });
        Self {
            listeners: Arc::new(Mutex::new(LegacyListeners::default())),
            capability_events,
            report_failure,
        }
    }

    pub fn subscribe<Listener>(
        &self,
        listener: Listener,
    ) -> HostEventListenerSubscription<Event>
    where
        Listener: Fn(&Event) -> Result<(), String> + Send + Sync + 'static,
    {
        let mut listeners = self.listeners.lock().unwrap();
        let id = listeners.next_id;
        listeners.next_id = listeners.next_id.wrapping_add(1);
        listeners.listeners.insert(id, Arc::new(listener));
        HostEventListenerSubscription {
            listeners: Arc::downgrade(&self.listeners),
            id,
        }
    }

    pub fn emit_event(&self, event: &Event) {
        let listeners = self
            .listeners
            .lock()
            .unwrap()
            .listeners
            .values()
            .cloned()
            .collect::<Vec<_>>();

        for listener in listeners {
            let result = catch_unwind(AssertUnwindSafe(|| listener(event)));
            let failure = match result {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(error),
                Err(payload) => Some(
                    payload
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| {
                            payload
                                .downcast_ref::<&'static str>()
                                .map(|value| (*value).to_owned())
                        })
                        .unwrap_or_else(|| "listener panicked".to_owned()),
                ),
            };
            if let Some(error_class) = failure {
                (self.report_failure)(HostEventBusFailure::ListenerFailed {
                    error_class,
                });
            }
        }
    }

    pub fn on<T, Handler, Fut>(
        &self,
        topic: impl Into<String>,
        handler: Handler,
    ) -> HostEventSubscription
    where
        T: Send + Sync + 'static,
        Handler: Fn(Arc<T>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
    {
        self.capability_events.on(topic, handler)
    }

    pub async fn emit_topic<T>(
        &self,
        topic: &str,
        payload: T,
        failure_mode: HostEventFailureMode,
    ) -> Result<(), String>
    where
        T: Send + Sync + 'static,
    {
        self.capability_events
            .emit(topic, payload, failure_mode)
            .await
    }
}

impl<Event> Default for SandHostEventBus<Event>
where
    Event: Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new(|_| {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug)]
    enum Event {
        Value(u32),
    }

    #[test]
    fn legacy_listeners_are_isolated_and_unsubscribable() {
        let failures = Arc::new(Mutex::new(Vec::<HostEventBusFailure>::new()));
        let sink = failures.clone();
        let bus = SandHostEventBus::new(move |failure| {
            sink.lock().unwrap().push(failure);
        });
        let observed = Arc::new(AtomicUsize::new(0));
        let count = observed.clone();
        let kept = bus.subscribe(move |event| {
            let Event::Value(value) = event;
            count.fetch_add(*value as usize, Ordering::SeqCst);
            Ok(())
        });
        let failing = bus.subscribe(|_event| Err("listener_error".into()));
        let panicking = bus.subscribe(|_event| -> Result<(), String> {
            panic!("listener_panic")
        });

        bus.emit_event(&Event::Value(2));
        assert_eq!(observed.load(Ordering::SeqCst), 2);
        assert_eq!(failures.lock().unwrap().len(), 2);

        failing.unsubscribe();
        panicking.unsubscribe();
        bus.emit_event(&Event::Value(3));
        assert_eq!(observed.load(Ordering::SeqCst), 5);
        assert_eq!(failures.lock().unwrap().len(), 2);
        kept.unsubscribe();
    }

    #[tokio::test]
    async fn capability_handlers_report_and_can_reject() {
        let failures = Arc::new(Mutex::new(Vec::<HostEventBusFailure>::new()));
        let sink = failures.clone();
        let bus = SandHostEventBus::<Event>::new(move |failure| {
            sink.lock().unwrap().push(failure);
        });
        let _subscription =
            bus.on::<String, _, _>("topic", |_payload| async {
                Err("subscriber_error".into())
            });

        bus.emit_topic(
            "topic",
            "payload".to_owned(),
            HostEventFailureMode::ReportOnly,
        )
        .await
        .unwrap();
        assert!(matches!(
            failures.lock().unwrap().first(),
            Some(HostEventBusFailure::SubscriberFailed { topic, error_class })
                if topic == "topic" && error_class == "subscriber_error"
        ));

        let error = bus
            .emit_topic(
                "topic",
                "payload".to_owned(),
                HostEventFailureMode::Reject,
            )
            .await
            .unwrap_err();
        assert_eq!(error, "subscriber_error");
    }
}
