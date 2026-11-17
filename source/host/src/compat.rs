use crate::error::ok_envelope;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct LegacyTestHostState {
    logged_in: bool,
    attempt_id: Option<String>,
    has_seen_onboarding: bool,
    user_time_zone: Option<String>,
    feature_flag_overrides: BTreeMap<String, Value>,
}

impl Default for LegacyTestHostState {
    fn default() -> Self {
        Self {
            logged_in: false,
            attempt_id: None,
            has_seen_onboarding: false,
            user_time_zone: None,
            feature_flag_overrides: BTreeMap::new(),
        }
    }
}

impl LegacyTestHostState {
    fn settings_json(&self) -> Value {
        json!({
            "hasSeenOnboarding": self.has_seen_onboarding,
            "userTimeZone": self.user_time_zone,
            "featureFlagOverrides": self.feature_flag_overrides,
        })
    }

    fn auth_session_json(&self) -> Value {
        if self.logged_in {
            json!({
                "@type": "mahayana.auth.session",
                "loggedIn": true,
                "provider": "browser",
                "user": {
                    "id": "fast-e2e-browser-user",
                    "email": "browser@example.test",
                    "nickname": "Browser 测试用户"
                }
            })
        } else {
            json!({
                "@type": "mahayana.auth.session",
                "loggedIn": false
            })
        }
    }
}

pub fn dispatch_legacy_test_method(
    state_lock: &Mutex<LegacyTestHostState>,
    method: &str,
    params: &Value,
) -> Option<String> {
    let mut state = state_lock.lock();
    match method {
        "feature.auth.status" => Some(ok_envelope(state.auth_session_json())),
        "feature.auth.browserStart" => {
            let attempt = "test-browser-login".to_string();
            state.attempt_id = Some(attempt.clone());
            Some(ok_envelope(json!({
                "attemptId": attempt,
                "loginUrl": "about:blank#fabushi-test-browser-login",
                "expiresAt": chrono::Utc::now().timestamp_millis() + 300_000,
                "pollAfterMs": 250
            })))
        }
        "feature.auth.browserPoll" => {
            let attempt = params
                .get("attemptId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !attempt.is_empty()
                && (state.attempt_id.as_deref() == Some(attempt) || attempt == "test-browser-login")
            {
                state.logged_in = true;
                state.attempt_id = None;
                let auth = state.auth_session_json();
                Some(ok_envelope(json!({
                    "status": "completed",
                    "provider": "browser",
                    "auth": auth
                })))
            } else {
                Some(ok_envelope(json!({
                    "status": "expired"
                })))
            }
        }
        "feature.auth.logout" => {
            state.logged_in = false;
            state.attempt_id = None;
            Some(ok_envelope(json!({
                "@type": "mahayana.auth.session",
                "loggedIn": false,
                "revoked": true
            })))
        }
        "getHostSettings" => Some(ok_envelope(state.settings_json())),
        "setHostSettings" => {
            let patch = params.get("settings").unwrap_or(params);
            if let Some(seen) = patch.get("hasSeenOnboarding").and_then(Value::as_bool) {
                state.has_seen_onboarding = seen;
            }
            if let Some(tz_val) = patch.get("userTimeZone") {
                if tz_val.is_null() {
                    state.user_time_zone = None;
                } else if let Some(tz) = tz_val.as_str() {
                    let trimmed = tz.trim();
                    state.user_time_zone = if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_string())
                    };
                }
            }
            if let Some(flags) = patch.get("featureFlagOverrides").and_then(Value::as_object) {
                for (k, v) in flags {
                    state.feature_flag_overrides.insert(k.clone(), v.clone());
                }
            }
            Some(ok_envelope(state.settings_json()))
        }
        _ => None,
    }
}
