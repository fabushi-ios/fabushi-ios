#![allow(unused_imports, dead_code)]

use axum::{
    body::Body,
    extract::{Path as AxumPath, State as AxumState},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

#[derive(Default)]
pub struct MockProviderState {
    pub call_count: AtomicU32,
    pub captured_bodies: Mutex<Vec<Value>>,
    pub captured_headers: Mutex<Vec<HashMap<String, String>>>,
    pub captured_paths: Mutex<Vec<String>>,
    pub force_status: Mutex<Option<StatusCode>>,
    pub force_retry_after: Mutex<Option<u64>>,
    pub delay_ms: AtomicU64,
    pub fail_count: AtomicU32,
    pub custom_reply_text: Mutex<Option<String>>,
}

impl MockProviderState {
    pub fn reset(&self) {
        self.call_count.store(0, Ordering::SeqCst);
        self.captured_bodies.lock().unwrap().clear();
        self.captured_headers.lock().unwrap().clear();
        self.captured_paths.lock().unwrap().clear();
        *self.force_status.lock().unwrap() = None;
        *self.force_retry_after.lock().unwrap() = None;
        self.delay_ms.store(0, Ordering::SeqCst);
        self.fail_count.store(0, Ordering::SeqCst);
        *self.custom_reply_text.lock().unwrap() = None;
    }

    pub fn set_force_status(&self, status: StatusCode, retry_after: Option<u64>) {
        *self.force_status.lock().unwrap() = Some(status);
        *self.force_retry_after.lock().unwrap() = retry_after;
    }

    pub fn set_fail_times(&self, times: u32, status: StatusCode) {
        self.fail_count.store(times, Ordering::SeqCst);
        *self.force_status.lock().unwrap() = Some(status);
    }
}

pub async fn spawn_mock_providers() -> (String, String, Arc<MockProviderState>) {
    let state = Arc::new(MockProviderState::default());

    // 1. OpenAI router
    let s_openai = state.clone();
    let openai_router = Router::new()
        .route(
            "/v1/chat/completions",
            post({
                let st = s_openai.clone();
                move |headers: HeaderMap, Json(body): Json<Value>| async move {
                    handle_openai_completion(st, headers, body).await
                }
            }),
        )
        .route(
            "/v1/models",
            get(|| async {
                Json(json!({
                    "object": "list",
                    "data": [
                        { "id": "gpt-4o", "object": "model", "created": 1700000000, "owned_by": "mock" },
                        { "id": "deepseek-chat", "object": "model", "created": 1700000000, "owned_by": "mock" }
                    ]
                }))
            }),
        );

    let openai_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock openai");
    let openai_port = openai_listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(openai_listener, openai_router).await;
    });

    // 2. Gemini router
    let s_gemini = state.clone();
    let gemini_router = Router::new()
        .route(
            "/v1beta/models/{model_action}",
            post({
                let st = s_gemini.clone();
                move |AxumPath(model_action): AxumPath<String>,
                      headers: HeaderMap,
                      Json(body): Json<Value>| async move {
                    handle_gemini_completion(st, model_action, headers, body).await
                }
            }),
        )
        .route(
            "/v1/models/{model_action}",
            post({
                let st = s_gemini.clone();
                move |AxumPath(model_action): AxumPath<String>,
                      headers: HeaderMap,
                      Json(body): Json<Value>| async move {
                    handle_gemini_completion(st, model_action, headers, body).await
                }
            }),
        );

    let gemini_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock gemini");
    let gemini_port = gemini_listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(gemini_listener, gemini_router).await;
    });

    (
        format!("http://127.0.0.1:{openai_port}/v1"),
        format!("http://127.0.0.1:{gemini_port}/v1beta"),
        state,
    )
}

async fn handle_openai_completion(
    st: Arc<MockProviderState>,
    headers: HeaderMap,
    body: Value,
) -> Response {
    st.call_count.fetch_add(1, Ordering::SeqCst);
    st.captured_paths
        .lock()
        .unwrap()
        .push("/v1/chat/completions".to_string());
    st.captured_bodies.lock().unwrap().push(body.clone());

    let mut hdr_map = HashMap::new();
    for (k, v) in headers.iter() {
        if let Ok(s) = v.to_str() {
            hdr_map.insert(k.as_str().to_string(), s.to_string());
        }
    }
    st.captured_headers.lock().unwrap().push(hdr_map);

    let delay = st.delay_ms.load(Ordering::SeqCst);
    if delay > 0 {
        tokio::time::sleep(Duration::from_millis(delay)).await;
    }

    // Check fail count or force status
    let remaining_fails = st.fail_count.load(Ordering::SeqCst);
    if remaining_fails > 0 {
        st.fail_count.fetch_sub(1, Ordering::SeqCst);
        let status = st.force_status.lock().unwrap().unwrap_or(StatusCode::TOO_MANY_REQUESTS);
        let retry = *st.force_retry_after.lock().unwrap();
        let mut resp = (status, Json(json!({ "error": { "message": "Simulated error", "type": "error" } }))).into_response();
        if let Some(sec) = retry {
            resp.headers_mut().insert("retry-after", sec.to_string().parse().unwrap());
        }
        return resp;
    }

    if let Some(status) = *st.force_status.lock().unwrap() {
        let retry = *st.force_retry_after.lock().unwrap();
        let mut resp = (status, Json(json!({ "error": { "message": "Forced error", "type": "error" } }))).into_response();
        if let Some(sec) = retry {
            resp.headers_mut().insert("retry-after", sec.to_string().parse().unwrap());
        }
        return resp;
    }

    let is_stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let reply_text = st
        .custom_reply_text
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| "MOCK_OPENAI_REPLY".to_string());

    if is_stream {
        let chunk1 = json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion.chunk",
            "created": 1700000000,
            "model": body.get("model").and_then(Value::as_str).unwrap_or("gpt-4o"),
            "choices": [{
                "index": 0,
                "delta": { "role": "assistant", "content": reply_text },
                "finish_reason": null
            }]
        });
        let chunk_done = json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion.chunk",
            "created": 1700000000,
            "model": body.get("model").and_then(Value::as_str).unwrap_or("gpt-4o"),
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "stop"
            }]
        });
        let sse_body = format!("data: {chunk1}\n\ndata: {chunk_done}\n\ndata: [DONE]\n\n");
        (
            StatusCode::OK,
            [("content-type", "text/event-stream")],
            sse_body,
        )
            .into_response()
    } else {
        (
            StatusCode::OK,
            Json(json!({
                "id": "chatcmpl-mock",
                "object": "chat.completion",
                "created": 1700000000,
                "model": body.get("model").and_then(Value::as_str).unwrap_or("gpt-4o"),
                "choices": [{
                    "index": 0,
                    "message": { "role": "assistant", "content": reply_text },
                    "finish_reason": "stop"
                }],
                "usage": { "prompt_tokens": 12, "completion_tokens": 8, "total_tokens": 20 }
            })),
        )
            .into_response()
    }
}

async fn handle_gemini_completion(
    st: Arc<MockProviderState>,
    model_action: String,
    headers: HeaderMap,
    body: Value,
) -> Response {
    st.call_count.fetch_add(1, Ordering::SeqCst);
    st.captured_paths
        .lock()
        .unwrap()
        .push(format!("/v1beta/models/{model_action}"));
    st.captured_bodies.lock().unwrap().push(body);

    let mut hdr_map = HashMap::new();
    for (k, v) in headers.iter() {
        if let Ok(s) = v.to_str() {
            hdr_map.insert(k.as_str().to_string(), s.to_string());
        }
    }
    st.captured_headers.lock().unwrap().push(hdr_map);

    let delay = st.delay_ms.load(Ordering::SeqCst);
    if delay > 0 {
        tokio::time::sleep(Duration::from_millis(delay)).await;
    }

    if let Some(status) = *st.force_status.lock().unwrap() {
        return (status, Json(json!({ "error": { "message": "Forced error" } }))).into_response();
    }

    let reply_text = st
        .custom_reply_text
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| "MOCK_GEMINI_STREAM".to_string());

    if model_action.contains("streamGenerateContent") {
        let evt = json!({
            "candidates": [{
                "content": { "role": "model", "parts": [{ "text": reply_text }] },
                "finishReason": "STOP"
            }],
            "usageMetadata": { "promptTokenCount": 5, "candidatesTokenCount": 3, "totalTokenCount": 8 }
        });
        (
            StatusCode::OK,
            [("content-type", "text/event-stream")],
            format!("data: {evt}\n\n"),
        )
            .into_response()
    } else {
        (
            StatusCode::OK,
            Json(json!({
                "candidates": [{
                    "content": { "role": "model", "parts": [{ "text": reply_text }] },
                    "finishReason": "STOP"
                }],
                "usageMetadata": { "promptTokenCount": 5, "candidatesTokenCount": 3, "totalTokenCount": 8 }
            })),
        )
            .into_response()
    }
}
