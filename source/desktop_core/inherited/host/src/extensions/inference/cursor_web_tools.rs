use std::sync::Arc;

use prost::Message;
use uuid::Uuid;

use crate::cursor_backend::{
    resolve_sand_ghost_mode_header, send_cursor_unary_with_request_id,
};
use crate::extensions::auth::credential_renewer::get_configured_backend_url;
use crate::extensions::auth::extension::HostAuthExtension;

pub const RUN_WEB_SEARCH_PATH: &str = "/aiserver.v1.AiService/RunWebSearch";
pub const RUN_WEB_FETCH_PATH: &str = "/aiserver.v1.AiService/RunWebFetch";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebDocument {
    pub url: String,
    pub title: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSearchRequest {
    pub search_term: String,
    pub explanation: Option<String>,
    pub model_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSearchResponse {
    pub answer: Option<String>,
    pub documents: Vec<WebDocument>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebFetchResponse {
    Success { content: String },
    Error { error: String, is_timeout: Option<bool> },
    MissingResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebFetchResult {
    pub content: Option<String>,
    pub error: Option<String>,
    pub is_timeout: Option<bool>,
}

pub trait CursorWebBackend: Send + Sync {
    fn run_web_search(&self, request: WebSearchRequest) -> Result<WebSearchResponse, String>;
    fn run_web_fetch(&self, url: &str) -> Result<WebFetchResponse, String>;
}

pub trait CursorWebAuth: Send + Sync {
    fn get_access_token(&self) -> Result<String, String>;
    fn get_machine_id(&self) -> Result<String, String>;
}

impl CursorWebAuth for HostAuthExtension {
    fn get_access_token(&self) -> Result<String, String> {
        HostAuthExtension::get_access_token(self).map_err(|error| error.to_string())
    }

    fn get_machine_id(&self) -> Result<String, String> {
        HostAuthExtension::get_machine_id(self).map_err(|error| error.to_string())
    }
}

pub type CursorWebRequestIdObserver = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone)]
pub struct CursorWebBackendOptions {
    pub auth: Arc<dyn CursorWebAuth>,
    pub backend_url: String,
    pub on_request_id: Option<CursorWebRequestIdObserver>,
}

impl CursorWebBackendOptions {
    pub fn production(
        auth: Arc<dyn CursorWebAuth>,
        on_request_id: Option<CursorWebRequestIdObserver>,
    ) -> Result<Self, String> {
        Ok(Self {
            auth,
            backend_url: get_configured_backend_url().map_err(|error| error.to_string())?,
            on_request_id,
        })
    }
}

#[derive(Clone, PartialEq, Message)]
struct ProtoRunWebSearchRequest {
    #[prost(string, tag = "1")]
    search_term: String,
    #[prost(string, optional, tag = "2")]
    explanation: Option<String>,
    #[prost(string, tag = "3")]
    model_id: String,
}

#[derive(Clone, PartialEq, Message)]
struct ProtoWebSearchDocument {
    #[prost(string, tag = "1")]
    url: String,
    #[prost(string, tag = "2")]
    title: String,
    #[prost(string, tag = "3")]
    text: String,
}

#[derive(Clone, PartialEq, Message)]
struct ProtoRunWebSearchResponse {
    #[prost(string, optional, tag = "1")]
    answer: Option<String>,
    #[prost(message, repeated, tag = "2")]
    documents: Vec<ProtoWebSearchDocument>,
}

#[derive(Clone, PartialEq, Message)]
struct ProtoRunWebFetchRequest {
    #[prost(string, tag = "1")]
    url: String,
}

#[derive(Clone, PartialEq, Message)]
struct ProtoRunWebFetchSuccess {
    #[prost(string, tag = "1")]
    content: String,
}

#[derive(Clone, PartialEq, Message)]
struct ProtoRunWebFetchError {
    #[prost(string, tag = "1")]
    error: String,
    #[prost(bool, tag = "2")]
    is_timeout: bool,
}

#[derive(Clone, PartialEq, Message)]
struct ProtoRunWebFetchResponse {
    #[prost(oneof = "proto_run_web_fetch_response::Result", tags = "1, 2")]
    result: Option<proto_run_web_fetch_response::Result>,
}

mod proto_run_web_fetch_response {
    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum Result {
        #[prost(message, tag = "1")]
        Success(super::ProtoRunWebFetchSuccess),
        #[prost(message, tag = "2")]
        Error(super::ProtoRunWebFetchError),
    }
}

#[derive(Clone)]
pub struct ProductionCursorWebBackend {
    options: CursorWebBackendOptions,
}

impl ProductionCursorWebBackend {
    pub fn new(options: CursorWebBackendOptions) -> Self {
        Self { options }
    }

    fn credentials(&self) -> Result<(String, String, &'static str), String> {
        let access_token = self.options.auth.get_access_token()?;
        let machine_id = self.options.auth.get_machine_id()?;
        let ghost_mode = resolve_sand_ghost_mode_header(
            &self.options.backend_url,
            &access_token,
            &machine_id,
        );
        Ok((access_token, machine_id, ghost_mode))
    }

    fn request_id(&self) -> String {
        let request_id = Uuid::new_v4().to_string();
        if let Some(observer) = self.options.on_request_id.as_ref() {
            observer(&request_id);
        }
        request_id
    }
}

impl CursorWebBackend for ProductionCursorWebBackend {
    fn run_web_search(&self, request: WebSearchRequest) -> Result<WebSearchResponse, String> {
        let (access_token, machine_id, ghost_mode) = self.credentials()?;
        let request_id = self.request_id();
        let proto = ProtoRunWebSearchRequest {
            search_term: request.search_term,
            explanation: request.explanation,
            model_id: request.model_id,
        };
        let bytes = send_cursor_unary_with_request_id(
            &self.options.backend_url,
            &access_token,
            &machine_id,
            RUN_WEB_SEARCH_PATH,
            &proto.encode_to_vec(),
            None,
            ghost_mode,
            &request_id,
        )
        .map_err(|error| error.to_string())?;
        let response = ProtoRunWebSearchResponse::decode(bytes.as_slice())
            .map_err(|error| format!("Web search returned an invalid protobuf response: {error}"))?;
        Ok(WebSearchResponse {
            answer: response.answer,
            documents: response
                .documents
                .into_iter()
                .map(|document| WebDocument {
                    url: document.url,
                    title: document.title,
                    text: document.text,
                })
                .collect(),
        })
    }

    fn run_web_fetch(&self, url: &str) -> Result<WebFetchResponse, String> {
        let (access_token, machine_id, ghost_mode) = self.credentials()?;
        let request_id = self.request_id();
        let request = ProtoRunWebFetchRequest {
            url: url.to_string(),
        };
        let bytes = send_cursor_unary_with_request_id(
            &self.options.backend_url,
            &access_token,
            &machine_id,
            RUN_WEB_FETCH_PATH,
            &request.encode_to_vec(),
            None,
            ghost_mode,
            &request_id,
        )
        .map_err(|error| error.to_string())?;
        let response = ProtoRunWebFetchResponse::decode(bytes.as_slice())
            .map_err(|error| format!("Web fetch returned an invalid protobuf response: {error}"))?;
        Ok(match response.result {
            Some(proto_run_web_fetch_response::Result::Success(success)) => {
                WebFetchResponse::Success {
                    content: success.content,
                }
            }
            Some(proto_run_web_fetch_response::Result::Error(error)) => {
                WebFetchResponse::Error {
                    error: error.error,
                    is_timeout: Some(error.is_timeout),
                }
            }
            None => WebFetchResponse::MissingResult,
        })
    }
}

pub fn create_cursor_web_backend(options: CursorWebBackendOptions) -> Arc<dyn CursorWebBackend> {
    Arc::new(ProductionCursorWebBackend::new(options))
}

#[derive(Clone)]
pub struct CursorWebSearchService {
    backend: Arc<dyn CursorWebBackend>,
    model_id: String,
}

impl CursorWebSearchService {
    pub fn new(backend: Arc<dyn CursorWebBackend>, model_id: impl Into<String>) -> Self {
        Self {
            backend,
            model_id: model_id.into(),
        }
    }

    pub fn search(
        &self,
        search_term: impl Into<String>,
        explanation: Option<String>,
    ) -> Result<WebSearchResponse, String> {
        self.backend.run_web_search(WebSearchRequest {
            search_term: search_term.into(),
            explanation,
            model_id: self.model_id.clone(),
        })
    }
}

#[derive(Clone)]
pub struct CursorWebFetchService {
    backend: Arc<dyn CursorWebBackend>,
}

impl CursorWebFetchService {
    pub fn new(backend: Arc<dyn CursorWebBackend>) -> Self {
        Self { backend }
    }

    pub fn fetch(&self, url: &str) -> Result<WebFetchResult, String> {
        match self.backend.run_web_fetch(url)? {
            WebFetchResponse::Success { content } => Ok(WebFetchResult {
                content: Some(content),
                error: None,
                is_timeout: None,
            }),
            WebFetchResponse::Error { error, is_timeout } => Ok(WebFetchResult {
                content: None,
                error: Some(error),
                is_timeout,
            }),
            WebFetchResponse::MissingResult => Ok(WebFetchResult {
                content: None,
                error: Some("Web fetch returned no result.".to_string()),
                is_timeout: None,
            }),
        }
    }
}
