use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::inference::cursor_web_tools::{
    CursorWebBackend, WebDocument, WebFetchResponse, WebSearchRequest, WebSearchResponse,
};
use mahayana_host_runtime::extensions::inference::extension::{
    AgentInferenceOwner, InferenceExtensionRuntime,
};
use mahayana_host_runtime::extensions::inference::inference_service::{
    InferenceRoute, InferenceSettings, InferenceUsage, authorize_routed_provider_request,
};
use mahayana_host_runtime::extensions::inference::production::{
    CursorWebBackendFactory, InferenceAuth, InferencePortFactory, InferenceProductionExtras,
    ModelExperimentApplied, ProductionInferenceSettings, RequestIdObserver,
    summarization_session_options,
};
use mahayana_host_runtime::extensions::inference::provider_session::RoutedProvider;
use mahayana_host_runtime::extensions::inference::cursor_session::SAND_SUMMARIZATION_MODEL_ID;
use mahayana_host_runtime::extensions::settings::settings_service::SettingsService;

struct FakeAuth;
impl InferenceAuth for FakeAuth {
    fn access_token(&self) -> Result<String, String> { Ok("token-a".into()) }
    fn machine_id(&self) -> String { "machine-a".into() }
}

struct FakeOwner;
impl AgentInferenceOwner for FakeOwner {}

#[derive(Default)]
struct FakePortFactory {
    auth_seen: Mutex<Vec<(String, String)>>,
}
impl InferencePortFactory for FakePortFactory {
    fn create_host_inference(
        &self,
        auth: Arc<dyn InferenceAuth>,
        on_model_experiment_applied: ModelExperimentApplied,
    ) -> Arc<dyn AgentInferenceOwner> {
        self.auth_seen.lock().unwrap().push((auth.access_token().unwrap(), auth.machine_id()));
        on_model_experiment_applied();
        Arc::new(FakeOwner)
    }
}

#[derive(Default)]
struct FakeBackendFactory {
    auth_seen: Mutex<Vec<(String, String)>>,
}
impl CursorWebBackendFactory for FakeBackendFactory {
    fn create_backend(
        &self,
        auth: Arc<dyn InferenceAuth>,
        on_request_id: Option<RequestIdObserver>,
    ) -> Arc<dyn CursorWebBackend> {
        self.auth_seen.lock().unwrap().push((auth.access_token().unwrap(), auth.machine_id()));
        if let Some(observer) = on_request_id { observer("req-42"); }
        Arc::new(FakeBackend)
    }
}

struct FakeBackend;
impl CursorWebBackend for FakeBackend {
    fn run_web_search(&self, request: WebSearchRequest) -> Result<WebSearchResponse, String> {
        assert_eq!(request.search_term, "Grok architecture");
        assert_eq!(request.explanation.as_deref(), Some("parity"));
        assert_eq!(request.model_id, "grok-4.5");
        Ok(WebSearchResponse {
            answer: Some("answer".into()),
            documents: vec![WebDocument {
                url: "https://example.invalid".into(),
                title: "doc".into(),
                text: "body".into(),
            }],
        })
    }

    fn run_web_fetch(&self, url: &str) -> Result<WebFetchResponse, String> {
        match url {
            "https://ok.invalid" => Ok(WebFetchResponse::Success { content: "page".into() }),
            "https://timeout.invalid" => Ok(WebFetchResponse::Error {
                error: "timeout".into(),
                is_timeout: Some(true),
            }),
            _ => Ok(WebFetchResponse::MissingResult),
        }
    }
}

#[test]
fn inference_extension_readiness_and_experiment_notifications_match_frozen_boundary() {
    let runtime = InferenceExtensionRuntime::new(FakeOwner, Arc::new(|| Some("token".to_string())));
    assert!(runtime.is_ready(false));

    let offline = InferenceExtensionRuntime::new(FakeOwner, Arc::new(|| None));
    assert!(!offline.is_ready(false));
    assert!(offline.is_ready(true));

    let applied = Arc::new(Mutex::new(0usize));
    let counter = Arc::clone(&applied);
    runtime.on_model_experiment_applied(Arc::new(move || *counter.lock().unwrap() += 1));
    runtime.notify_model_experiment_applied();
    assert_eq!(*applied.lock().unwrap(), 1);
}

#[test]
fn production_extras_share_auth_and_forward_request_identity_to_web_services() {
    let port_factory = Arc::new(FakePortFactory::default());
    let backend_factory = Arc::new(FakeBackendFactory::default());
    let extras = InferenceProductionExtras::new(
        Arc::new(FakeAuth),
        port_factory.clone(),
        backend_factory.clone(),
    );

    let applied = Arc::new(Mutex::new(0usize));
    let applied_counter = Arc::clone(&applied);
    let _port = extras.create_port(Arc::new(move || *applied_counter.lock().unwrap() += 1));
    assert_eq!(*applied.lock().unwrap(), 1);

    let request_ids = Arc::new(Mutex::new(Vec::<String>::new()));
    let request_ids_sink = Arc::clone(&request_ids);
    let search = extras.create_web_search(
        "grok-4.5",
        Some(Arc::new(move |request_id| {
            request_ids_sink.lock().unwrap().push(request_id.to_string())
        })),
    );
    let response = search.search("Grok architecture", Some("parity".into())).expect("search");
    assert_eq!(response.answer.as_deref(), Some("answer"));
    assert_eq!(response.documents.len(), 1);

    let fetch = extras.create_web_fetch(None);
    assert_eq!(fetch.fetch("https://ok.invalid").unwrap().content.as_deref(), Some("page"));
    let timeout = fetch.fetch("https://timeout.invalid").unwrap();
    assert_eq!(timeout.error.as_deref(), Some("timeout"));
    assert_eq!(timeout.is_timeout, Some(true));
    assert_eq!(
        fetch.fetch("https://missing.invalid").unwrap().error.as_deref(),
        Some("Web fetch returned no result.")
    );

    assert_eq!(request_ids.lock().unwrap().as_slice(), ["req-42"]);
    assert_eq!(port_factory.auth_seen.lock().unwrap().len(), 1);
    assert_eq!(backend_factory.auth_seen.lock().unwrap().len(), 2);
}


#[test]
fn production_inference_settings_owns_live_route_and_usage_persistence() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "fabushi-production-inference-settings-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&root).expect("create temp settings dir");
    let settings = Arc::new(SettingsService::new(root.join("settings.json")));
    settings
        .set_inference_provider("openrouter")
        .expect("persist provider");
    let adapter = ProductionInferenceSettings::new(Arc::clone(&settings));

    assert_eq!(adapter.inference_provider(), RoutedProvider::OpenRouter);
    adapter.record_inference_usage(
        RoutedProvider::OpenRouter,
        InferenceUsage {
            input_tokens: Some(11),
            output_tokens: Some(7),
            cache_read_tokens: Some(3),
            cache_write_tokens: Some(2),
        },
    );

    let usage = settings.get_inference_router_usage();
    assert_eq!(usage["providers"]["openrouter"]["requests"].as_u64(), Some(1));
    assert_eq!(usage["providers"]["openrouter"]["inputTokens"].as_u64(), Some(11));
    assert_eq!(usage["providers"]["openrouter"]["outputTokens"].as_u64(), Some(7));
    assert_eq!(usage["providers"]["openrouter"]["cacheReadTokens"].as_u64(), Some(3));
    assert_eq!(usage["providers"]["openrouter"]["cacheWriteTokens"].as_u64(), Some(2));
    let _ = fs::remove_dir_all(root);
}


#[test]
fn cursor_route_is_authorized_through_the_same_shipping_runner_boundary() {
    assert_eq!(
        authorize_routed_provider_request(InferenceRoute::Cursor, RoutedProvider::Cursor),
        Ok(RoutedProvider::Cursor)
    );
    assert!(authorize_routed_provider_request(
        InferenceRoute::Cursor,
        RoutedProvider::ClaudeCode,
    )
    .is_err());

    let shipping_main = include_str!("../app/src/main.rs");
    assert!(shipping_main.contains(
        "let requested_provider = RoutedProvider::parse(provider_name)"
    ));
    assert!(!shipping_main.contains(
        ".filter(|provider| *provider != RoutedProvider::Cursor)"
    ));
    assert!(shipping_main.contains(
        "let cursor_auth = inference.cursor_auth_for_session(Some(&worker_session_options));"
    ));
    assert!(shipping_main.contains(
        "cursor_auth: Some(cursor_auth)"
    ));
    assert!(shipping_main.contains(
        "run_local_background_revival_turn("
    ));
    assert!(!shipping_main.contains(
        "Cursor background revival remains on the compatibility path"
    ));
    assert!(shipping_main.contains(
        "configured_routed_provider(&gateway.data_dir.join(\"settings.json\"))"
    ));
}


#[test]
fn summarization_session_matches_frozen_model_and_labeling_fence() {
    let options = summarization_session_options();
    assert_eq!(
        options.model_id.as_deref(),
        Some(SAND_SUMMARIZATION_MODEL_ID)
    );
    assert_eq!(SAND_SUMMARIZATION_MODEL_ID, "gemini-2.5-flash");
    assert!(options.is_summarization_session);
    assert!(options.skip_labeling);
    assert!(!options.is_computer_use_subagent);
    assert!(!options.is_browser_use_subagent);
}
