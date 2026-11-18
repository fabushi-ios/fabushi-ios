pub mod ai_miniapps_infra;
pub mod calls_settings_wallet;
pub mod chats_messages;
pub mod config_auth;
pub mod crypto_media_social;

use crate::compat::dispatch_legacy_test_method;
use crate::error::BridgeError;
use crate::host::TelegramAppHost;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub struct RpcEnvelope {
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub id: Option<Value>,
    #[serde(default)]
    pub params: Value,
}

impl TelegramAppHost {
    pub fn dispatch(&self, request_json: &str) -> String {
        let env: RpcEnvelope = match serde_json::from_str(request_json) {
            Ok(parsed) => parsed,
            Err(e) => {
                return BridgeError::invalid_json(format!("Malformed JSON: {e}"))
                    .to_envelope_string()
            }
        };

        let Some(cmd) = env
            .command
            .as_deref()
            .or(env.method.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return BridgeError::new(
                "missing_command",
                "Missing non-empty 'command' or 'method' field",
            )
            .to_envelope_string();
        };

        if self.is_test_mode && cmd == "debug.trigger_panic" {
            panic!("Intentional test panic for FFI catch_unwind verification");
        }

        if self.is_test_mode {
            if let Some(legacy_reply) =
                dispatch_legacy_test_method(&self.legacy_test_state, cmd, &env.params)
            {
                return legacy_reply;
            }
        }

        if let Some(resp) = config_auth::handle(self, cmd, &env.params) {
            return resp;
        }
        if let Some(resp) = chats_messages::handle(self, cmd, &env.params) {
            return resp;
        }
        if let Some(resp) = crypto_media_social::handle(self, cmd, &env.params) {
            return resp;
        }
        if let Some(resp) = calls_settings_wallet::handle(self, cmd, &env.params) {
            return resp;
        }
        if let Some(resp) = ai_miniapps_infra::handle(self, cmd, &env.params) {
            return resp;
        }

        // Strict fail-closed rejection: unimplemented or unknown commands return error.code == "unknown_command"
        BridgeError::unknown_command(cmd).to_envelope_string()
    }
}
