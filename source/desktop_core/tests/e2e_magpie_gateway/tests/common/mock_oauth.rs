#![allow(unused_imports, dead_code)]

use axum::{
    extract::State as AxumState,
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Form, Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc, Mutex,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockOAuthForm {
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub refresh_token: Option<String>,
    pub grant_type: Option<String>,
}

#[derive(Default)]
pub struct MockOAuthState {
    pub call_count: AtomicU32,
    pub captured_forms: Mutex<Vec<HashMap<String, String>>>,
    pub force_error: AtomicBool,
    pub custom_access_token: Mutex<Option<String>>,
    pub custom_expires_in: Mutex<Option<i64>>,
}

impl MockOAuthState {
    pub fn reset(&self) {
        self.call_count.store(0, Ordering::SeqCst);
        self.captured_forms.lock().unwrap().clear();
        self.force_error.store(false, Ordering::SeqCst);
        *self.custom_access_token.lock().unwrap() = None;
        *self.custom_expires_in.lock().unwrap() = None;
    }
}

pub async fn spawn_mock_oauth() -> (String, Arc<MockOAuthState>) {
    let state = Arc::new(MockOAuthState::default());
    let s_clone = state.clone();

    let router = Router::new().route(
        "/token",
        post({
            let st = s_clone.clone();
            move |Form(form): Form<HashMap<String, String>>| async move {
                st.call_count.fetch_add(1, Ordering::SeqCst);
                st.captured_forms.lock().unwrap().push(form.clone());

                if st.force_error.load(Ordering::SeqCst) {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "error": "invalid_grant",
                            "error_description": "Token has been expired or revoked"
                        })),
                    )
                        .into_response();
                }

                let token = st
                    .custom_access_token
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_else(|| format!("ya29.mock_token_{}", uuid::Uuid::new_v4().simple()));
                let expires_in = st
                    .custom_expires_in
                    .lock()
                    .unwrap()
                    .unwrap_or(3600);

                (
                    StatusCode::OK,
                    Json(json!({
                        "access_token": token,
                        "expires_in": expires_in,
                        "token_type": "Bearer",
                        "scope": "https://www.googleapis.com/auth/cloud-platform"
                    })),
                )
                    .into_response()
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock oauth server");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    (format!("http://127.0.0.1:{port}/token"), state)
}
