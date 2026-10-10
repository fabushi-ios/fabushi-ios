use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostExtensionDeclaration {
    pub id: String,
    pub dependencies: Vec<String>,
}

impl HostExtensionDeclaration {
    pub fn new<I, S>(id: impl Into<String>, dependencies: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            id: id.into(),
            dependencies: dependencies.into_iter().map(Into::into).collect(),
        }
    }
}

pub type HostApi = Arc<dyn Any + Send + Sync>;
type HostEventPayload = Arc<dyn Any + Send + Sync>;
type HostEventFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'static>>;
type HostEventHandler = Arc<dyn Fn(HostEventPayload) -> HostEventFuture + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostEventFailureMode {
    ReportOnly,
    Reject,
}

#[derive(Default)]
struct HostEventHandlers {
    next_id: u64,
    topics: BTreeMap<String, BTreeMap<u64, HostEventHandler>>,
}

#[derive(Clone)]
pub struct HostEvents {
    inner: Arc<Mutex<HostEventHandlers>>,
    on_handler_failure: Arc<dyn Fn(&str, &str) + Send + Sync>,
}

pub struct HostEventSubscription {
    inner: Weak<Mutex<HostEventHandlers>>,
    topic: String,
    id: u64,
}

impl HostEventSubscription {
    pub fn unsubscribe(&self) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let mut state = inner.lock().unwrap();
        if let Some(handlers) = state.topics.get_mut(&self.topic) {
            handlers.remove(&self.id);
            if handlers.is_empty() {
                state.topics.remove(&self.topic);
            }
        }
    }
}

impl HostEvents {
    pub fn new<Failure>(on_handler_failure: Failure) -> Self
    where
        Failure: Fn(&str, &str) + Send + Sync + 'static,
    {
        Self {
            inner: Arc::new(Mutex::new(HostEventHandlers::default())),
            on_handler_failure: Arc::new(on_handler_failure),
        }
    }

    pub fn on<T, Handler, Fut>(
        &self,
        topic: impl Into<String>,
        handler: Handler,
    ) -> HostEventSubscription
    where
        T: Any + Send + Sync + 'static,
        Handler: Fn(Arc<T>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        let topic = topic.into();
        let handler = Arc::new(handler);
        let erased: HostEventHandler = Arc::new(move |payload| {
            let handler = handler.clone();
            Box::pin(async move {
                let typed = Arc::downcast::<T>(payload)
                    .map_err(|_| "host event payload type mismatch".to_owned())?;
                handler(typed).await
            })
        });

        let mut state = self.inner.lock().unwrap();
        let id = state.next_id;
        state.next_id = state.next_id.wrapping_add(1);
        state
            .topics
            .entry(topic.clone())
            .or_default()
            .insert(id, erased);
        HostEventSubscription {
            inner: Arc::downgrade(&self.inner),
            topic,
            id,
        }
    }

    pub async fn emit<T>(
        &self,
        topic: &str,
        payload: T,
        failure_mode: HostEventFailureMode,
    ) -> Result<(), String>
    where
        T: Any + Send + Sync + 'static,
    {
        self.emit_payload(topic, Arc::new(payload), failure_mode)
            .await
    }

    async fn emit_payload(
        &self,
        topic: &str,
        payload: HostEventPayload,
        failure_mode: HostEventFailureMode,
    ) -> Result<(), String> {
        let handlers = {
            let state = self.inner.lock().unwrap();
            state
                .topics
                .get(topic)
                .map(|handlers| handlers.values().cloned().collect::<Vec<_>>())
                .unwrap_or_default()
        };
        if handlers.is_empty() {
            return Ok(());
        }

        // Start every handler before awaiting any result, matching
        // Promise.allSettled rather than serial handler execution.
        let handles = handlers
            .into_iter()
            .map(|handler| {
                let payload = payload.clone();
                tokio::spawn(async move { handler(payload).await })
            })
            .collect::<Vec<_>>();

        let mut first_failure = None;
        for handle in handles {
            let outcome = match handle.await {
                Ok(result) => result,
                Err(error) => Err(format!("host event handler task failed: {error}")),
            };
            if let Err(error) = outcome {
                (self.on_handler_failure)(topic, &error);
                if failure_mode == HostEventFailureMode::Reject && first_failure.is_none() {
                    first_failure = Some(error);
                }
            }
        }

        match first_failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

#[derive(Clone)]
pub struct HostExtensionRuntimeDeclaration<Host> {
    pub declaration: HostExtensionDeclaration,
    start: Arc<
        dyn Fn(HostExtensionRuntimeContext<Host>) -> HostExtensionStartFuture
            + Send
            + Sync,
    >,
}

type HostExtensionStartFuture =
    Pin<Box<dyn Future<Output = Result<HostApi, String>> + Send + 'static>>;
type HostTeardownFuture =
    Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'static>>;
type HostTeardown = Box<dyn FnOnce() -> HostTeardownFuture + Send + 'static>;

struct TeardownEntry {
    extension_id: String,
    run: HostTeardown,
}

#[derive(Clone)]
pub struct HostExtensionRuntimeContext<Host> {
    deps: BTreeMap<String, HostApi>,
    host: Arc<Host>,
    extension_id: String,
    teardowns: Arc<Mutex<Vec<TeardownEntry>>>,
}

impl<Host> HostExtensionRuntimeContext<Host>
where
    Host: Send + Sync + 'static,
{
    pub fn host(&self) -> &Host {
        self.host.as_ref()
    }

    pub fn dependency<Api>(&self, id: &str) -> Option<Arc<Api>>
    where
        Api: Any + Send + Sync + 'static,
    {
        self.deps.get(id)?.clone().downcast::<Api>().ok()
    }

    pub fn on_stop<Teardown, Fut>(&self, teardown: Teardown)
    where
        Teardown: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        self.teardowns.lock().unwrap().push(TeardownEntry {
            extension_id: self.extension_id.clone(),
            run: Box::new(move || Box::pin(teardown())),
        });
    }
}

pub fn define_host_extension<Host, Api, Start, Fut, Dependencies, Dependency>(
    id: impl Into<String>,
    dependencies: Dependencies,
    start: Start,
) -> HostExtensionRuntimeDeclaration<Host>
where
    Host: Send + Sync + 'static,
    Api: Any + Send + Sync + 'static,
    Start: Fn(HostExtensionRuntimeContext<Host>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Api, String>> + Send + 'static,
    Dependencies: IntoIterator<Item = Dependency>,
    Dependency: Into<String>,
{
    let declaration = HostExtensionDeclaration::new(id, dependencies);
    let start = Arc::new(start);
    HostExtensionRuntimeDeclaration {
        declaration,
        start: Arc::new(move |context| {
            let start = start.clone();
            Box::pin(async move {
                start(context)
                    .await
                    .map(|api| Arc::new(api) as HostApi)
            })
        }),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostExtensionGraphError {
    DuplicateId(String),
    SelfDependency(String),
    MissingDependency {
        extension: String,
        dependency: String,
    },
    Cycle(Vec<String>),
    UnknownResolvedId(String),
}

impl fmt::Display for HostExtensionGraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateId(id) => {
                write!(formatter, "two host extensions declare the id \"{id}\"")
            }
            Self::SelfDependency(id) => {
                write!(
                    formatter,
                    "host extension \"{id}\" declares itself as a peer"
                )
            }
            Self::MissingDependency {
                extension,
                dependency,
            } => write!(
                formatter,
                "host extension \"{extension}\" requires the peer \"{dependency}\", which is not in this build"
            ),
            Self::Cycle(path) => {
                write!(formatter, "host extension peer cycle: {}", path.join(" → "))
            }
            Self::UnknownResolvedId(id) => write!(
                formatter,
                "the resolved boot order names an unknown host extension \"{id}\""
            ),
        }
    }
}

impl std::error::Error for HostExtensionGraphError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostExtensionStartError {
    pub extension_id: String,
    pub cause: String,
}

impl fmt::Display for HostExtensionStartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "host extension \"{}\" failed to start",
            self.extension_id
        )
    }
}

impl std::error::Error for HostExtensionStartError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StartHostExtensionsError {
    Graph(HostExtensionGraphError),
    Start(HostExtensionStartError),
}

impl fmt::Display for StartHostExtensionsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Graph(error) => error.fmt(formatter),
            Self::Start(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for StartHostExtensionsError {}

pub fn resolve_host_extension_boot_order(
    extensions: &[HostExtensionDeclaration],
) -> Result<Vec<String>, HostExtensionGraphError> {
    let mut peers = BTreeMap::<String, BTreeSet<String>>::new();
    for extension in extensions {
        if peers.contains_key(&extension.id) {
            return Err(HostExtensionGraphError::DuplicateId(
                extension.id.clone(),
            ));
        }
        peers.insert(
            extension.id.clone(),
            extension.dependencies.iter().cloned().collect(),
        );
    }

    for extension in extensions {
        for dependency in &extension.dependencies {
            if dependency == &extension.id {
                return Err(HostExtensionGraphError::SelfDependency(
                    extension.id.clone(),
                ));
            }
            if !peers.contains_key(dependency) {
                return Err(HostExtensionGraphError::MissingDependency {
                    extension: extension.id.clone(),
                    dependency: dependency.clone(),
                });
            }
        }
    }

    let mut remaining = peers.keys().cloned().collect::<Vec<_>>();
    let mut started = BTreeSet::<String>::new();
    let mut order = Vec::with_capacity(remaining.len());
    while !remaining.is_empty() {
        let index = remaining.iter().position(|id| {
            peers
                .get(id)
                .into_iter()
                .flatten()
                .all(|dependency| started.contains(dependency))
        });
        let Some(index) = index else {
            return Err(HostExtensionGraphError::Cycle(describe_cycle(
                &peers, &started,
            )));
        };
        let id = remaining.remove(index);
        started.insert(id.clone());
        order.push(id);
    }
    Ok(order)
}

fn describe_cycle(
    peers: &BTreeMap<String, BTreeSet<String>>,
    started: &BTreeSet<String>,
) -> Vec<String> {
    fn walk(
        id: &str,
        peers: &BTreeMap<String, BTreeSet<String>>,
        started: &BTreeSet<String>,
        path: &mut Vec<String>,
    ) -> Option<Vec<String>> {
        if let Some(index) = path.iter().position(|item| item == id) {
            let mut cycle = path[index..].to_vec();
            cycle.push(id.to_string());
            return Some(cycle);
        }
        path.push(id.to_string());
        if let Some(dependencies) = peers.get(id) {
            for dependency in dependencies {
                if started.contains(dependency) {
                    continue;
                }
                if let Some(cycle) = walk(dependency, peers, started, path) {
                    return Some(cycle);
                }
            }
        }
        path.pop();
        None
    }

    for id in peers.keys().filter(|id| !started.contains(*id)) {
        if let Some(cycle) = walk(id, peers, started, &mut Vec::new()) {
            return cycle;
        }
    }
    peers
        .keys()
        .filter(|id| !started.contains(*id))
        .cloned()
        .collect()
}

pub struct StartedHostExtensions {
    pub order: Vec<String>,
    apis: BTreeMap<String, HostApi>,
    teardowns: Arc<Mutex<Vec<TeardownEntry>>>,
    on_stop_failure: Arc<dyn Fn(&str, &str) + Send + Sync>,
}

impl StartedHostExtensions {
    pub fn api<Api>(&self, id: &str) -> Option<Arc<Api>>
    where
        Api: Any + Send + Sync + 'static,
    {
        self.apis.get(id)?.clone().downcast::<Api>().ok()
    }

    pub async fn stop(&mut self) {
        stop_teardowns(&self.teardowns, &self.on_stop_failure).await;
    }
}

async fn stop_teardowns(
    teardowns: &Arc<Mutex<Vec<TeardownEntry>>>,
    on_stop_failure: &Arc<dyn Fn(&str, &str) + Send + Sync>,
) {
    let mut pending = {
        let mut entries = teardowns.lock().unwrap();
        std::mem::take(&mut *entries)
    };
    while let Some(entry) = pending.pop() {
        if let Err(error) = (entry.run)().await {
            on_stop_failure(&entry.extension_id, &error);
        }
    }
}

pub async fn start_host_extensions<Host, Failure>(
    extensions: &[HostExtensionRuntimeDeclaration<Host>],
    host: Arc<Host>,
    on_stop_failure: Failure,
) -> Result<StartedHostExtensions, StartHostExtensionsError>
where
    Host: Send + Sync + 'static,
    Failure: Fn(&str, &str) + Send + Sync + 'static,
{
    let declarations = extensions
        .iter()
        .map(|extension| extension.declaration.clone())
        .collect::<Vec<_>>();
    let order = resolve_host_extension_boot_order(&declarations)
        .map_err(StartHostExtensionsError::Graph)?;

    let by_id = extensions
        .iter()
        .map(|extension| (extension.declaration.id.clone(), extension))
        .collect::<BTreeMap<_, _>>();
    let teardowns = Arc::new(Mutex::new(Vec::new()));
    let on_stop_failure: Arc<dyn Fn(&str, &str) + Send + Sync> =
        Arc::new(on_stop_failure);
    let mut apis = BTreeMap::<String, HostApi>::new();

    for id in &order {
        let extension = by_id.get(id).ok_or_else(|| {
            StartHostExtensionsError::Graph(
                HostExtensionGraphError::UnknownResolvedId(id.clone()),
            )
        })?;
        let deps = extension
            .declaration
            .dependencies
            .iter()
            .filter_map(|dependency| {
                apis.get(dependency)
                    .cloned()
                    .map(|api| (dependency.clone(), api))
            })
            .collect::<BTreeMap<_, _>>();
        let context = HostExtensionRuntimeContext {
            deps,
            host: host.clone(),
            extension_id: id.clone(),
            teardowns: teardowns.clone(),
        };
        match (extension.start)(context).await {
            Ok(api) => {
                apis.insert(id.clone(), api);
            }
            Err(cause) => {
                stop_teardowns(&teardowns, &on_stop_failure).await;
                return Err(StartHostExtensionsError::Start(
                    HostExtensionStartError {
                        extension_id: id.clone(),
                        cause,
                    },
                ));
            }
        }
    }

    Ok(StartedHostExtensions {
        order,
        apis,
        teardowns,
        on_stop_failure,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_dependencies_before_dependents() {
        let extensions = vec![
            HostExtensionDeclaration::new("gateway", ["auth"]),
            HostExtensionDeclaration::new("auth", std::iter::empty::<&str>()),
            HostExtensionDeclaration::new("tools", ["gateway"]),
        ];
        assert_eq!(
            resolve_host_extension_boot_order(&extensions).unwrap(),
            vec!["auth", "gateway", "tools"]
        );
    }

    #[test]
    fn rejects_missing_dependency_and_cycles() {
        let missing = vec![HostExtensionDeclaration::new("gateway", ["auth"])];
        assert!(matches!(
            resolve_host_extension_boot_order(&missing),
            Err(HostExtensionGraphError::MissingDependency { .. })
        ));

        let cycle = vec![
            HostExtensionDeclaration::new("a", ["b"]),
            HostExtensionDeclaration::new("b", ["a"]),
        ];
        let error = resolve_host_extension_boot_order(&cycle).unwrap_err();
        assert!(matches!(error, HostExtensionGraphError::Cycle(_)));
        assert!(error.to_string().contains("a → b → a"));
    }

    #[tokio::test]
    async fn host_events_start_all_handlers_and_report_failures_in_registration_order() {
        let failures = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = failures.clone();
        let events = HostEvents::new(move |topic, error| {
            sink.lock()
                .unwrap()
                .push(format!("{topic}:{error}"));
        });

        let _first = events.on::<String, _, _>("updates", |_payload| async {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            Err("first".into())
        });
        let _second = events.on::<String, _, _>("updates", |_payload| async {
            Err("second".into())
        });

        let error = events
            .emit(
                "updates",
                "payload".to_owned(),
                HostEventFailureMode::Reject,
            )
            .await
            .unwrap_err();
        assert_eq!(error, "first");
        assert_eq!(
            failures.lock().unwrap().as_slice(),
            &["updates:first", "updates:second"]
        );
    }

    #[tokio::test]
    async fn host_event_unsubscribe_and_report_only_match_reference() {
        let failures = Arc::new(Mutex::new(0usize));
        let sink = failures.clone();
        let events = HostEvents::new(move |_topic, _error| {
            *sink.lock().unwrap() += 1;
        });
        let subscription = events.on::<u32, _, _>("topic", |_payload| async {
            Err("ignored".into())
        });

        events
            .emit("topic", 1u32, HostEventFailureMode::ReportOnly)
            .await
            .unwrap();
        assert_eq!(*failures.lock().unwrap(), 1);

        subscription.unsubscribe();
        events
            .emit("topic", 2u32, HostEventFailureMode::Reject)
            .await
            .unwrap();
        assert_eq!(*failures.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn starts_extensions_with_typed_dependencies_and_stops_in_reverse_order() {
        let lifecycle = Arc::new(Mutex::new(Vec::<String>::new()));

        let auth_lifecycle = lifecycle.clone();
        let auth = define_host_extension(
            "auth",
            std::iter::empty::<&str>(),
            move |context: HostExtensionRuntimeContext<()>| {
                let lifecycle = auth_lifecycle.clone();
                context.on_stop(move || {
                    let lifecycle = lifecycle.clone();
                    async move {
                        lifecycle.lock().unwrap().push("stop-auth".into());
                        Ok(())
                    }
                });
                async { Ok::<_, String>("token".to_owned()) }
            },
        );

        let gateway_lifecycle = lifecycle.clone();
        let gateway = define_host_extension(
            "gateway",
            ["auth"],
            move |context: HostExtensionRuntimeContext<()>| {
                let token = context
                    .dependency::<String>("auth")
                    .expect("auth dependency");
                let lifecycle = gateway_lifecycle.clone();
                context.on_stop(move || {
                    let lifecycle = lifecycle.clone();
                    async move {
                        lifecycle.lock().unwrap().push("stop-gateway".into());
                        Ok(())
                    }
                });
                async move { Ok::<_, String>(format!("gateway:{}", token.as_str())) }
            },
        );

        let mut started = start_host_extensions(
            &[gateway, auth],
            Arc::new(()),
            |_extension_id, _error| {},
        )
        .await
        .unwrap();
        assert_eq!(started.order, vec!["auth", "gateway"]);
        assert_eq!(
            started.api::<String>("gateway").unwrap().as_str(),
            "gateway:token"
        );

        started.stop().await;
        assert_eq!(
            lifecycle.lock().unwrap().as_slice(),
            &["stop-gateway", "stop-auth"]
        );
    }

    #[tokio::test]
    async fn start_failure_rolls_back_current_and_prior_teardowns() {
        let lifecycle = Arc::new(Mutex::new(Vec::<String>::new()));
        let first_lifecycle = lifecycle.clone();
        let first = define_host_extension(
            "first",
            std::iter::empty::<&str>(),
            move |context: HostExtensionRuntimeContext<()>| {
                let lifecycle = first_lifecycle.clone();
                context.on_stop(move || {
                    let lifecycle = lifecycle.clone();
                    async move {
                        lifecycle.lock().unwrap().push("stop-first".into());
                        Ok(())
                    }
                });
                async { Ok::<_, String>(()) }
            },
        );

        let failing_lifecycle = lifecycle.clone();
        let failing = define_host_extension(
            "failing",
            ["first"],
            move |context: HostExtensionRuntimeContext<()>| {
                let lifecycle = failing_lifecycle.clone();
                context.on_stop(move || {
                    let lifecycle = lifecycle.clone();
                    async move {
                        lifecycle.lock().unwrap().push("stop-failing".into());
                        Ok(())
                    }
                });
                async { Err::<(), _>("boom".to_owned()) }
            },
        );

        let error = match start_host_extensions(
            &[failing, first],
            Arc::new(()),
            |_extension_id, _error| {},
        )
        .await
        {
            Ok(_) => panic!("start unexpectedly succeeded"),
            Err(error) => error,
        };
        let StartHostExtensionsError::Start(error) = error else {
            panic!("expected extension start error");
        };
        assert_eq!(error.extension_id, "failing");
        assert_eq!(error.cause, "boom");
        assert_eq!(
            lifecycle.lock().unwrap().as_slice(),
            &["stop-failing", "stop-first"]
        );
    }
}
