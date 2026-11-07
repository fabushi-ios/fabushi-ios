use std::collections::HashMap;
use std::sync::Arc;

use crate::oauth::mcp_oauth_loopback_registry::{
    LoopbackHttpResponse, LoopbackLease, McpOAuthLoopbackHandler, McpOAuthLoopbackRegistry,
    parse_loopback_redirect,
};
use crate::protocol::Failure;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthCallback {
    pub code: String,
    pub state: String,
    pub server_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OAuthCallbackDisposition {
    Unhandled,
    ProviderError {
        state: String,
        server_name: String,
        error: String,
    },
    MissingCode {
        state: String,
        server_name: String,
    },
    Complete(OAuthCallback),
}

impl OAuthCallback {
    pub fn validate(self) -> Result<Self, Failure> {
        if self.code.trim().is_empty()
            || self.state.trim().is_empty()
            || self.server_name.trim().is_empty()
        {
            return Err(Failure::new(
                "MCP_OAUTH_INVALID_CALLBACK",
                "OAuth callback requires code, state, and server identity",
            ));
        }
        Ok(self)
    }
}

pub fn classify_callback(
    request_target: &str,
    expected_path: &str,
    mut resolve: impl FnMut(&str) -> Option<String>,
) -> Result<OAuthCallbackDisposition, Failure> {
    let (path, query) = request_target
        .split_once('?')
        .map_or((request_target, ""), |(path, query)| (path, query));
    if path != expected_path {
        return Ok(OAuthCallbackDisposition::Unhandled);
    }

    let params = parse_query(query)?;
    let Some(state) = params.get("state").filter(|state| !state.is_empty()).cloned() else {
        return Ok(OAuthCallbackDisposition::Unhandled);
    };
    let Some(server_name) = resolve(&state) else {
        return Ok(OAuthCallbackDisposition::Unhandled);
    };

    if let Some(error) = params.get("error") {
        return Ok(OAuthCallbackDisposition::ProviderError {
            state,
            server_name,
            error: error.clone(),
        });
    }

    let Some(code) = params.get("code").filter(|code| !code.is_empty()).cloned() else {
        return Ok(OAuthCallbackDisposition::MissingCode {
            state,
            server_name,
        });
    };

    Ok(OAuthCallbackDisposition::Complete(
        OAuthCallback {
            code,
            state,
            server_name,
        }
        .validate()?,
    ))
}

fn parse_query(query: &str) -> Result<HashMap<String, String>, Failure> {
    let mut params = HashMap::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (raw_key, raw_value) = pair
            .split_once('=')
            .map_or((pair, ""), |(key, value)| (key, value));
        let key = decode_component(raw_key)?;
        let value = decode_component(raw_value)?;
        params.entry(key).or_insert(value);
    }
    Ok(params)
}

fn decode_component(value: &str) -> Result<String, Failure> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                output.push(b' ');
                index += 1;
            }
            b'%' => {
                if index + 2 >= bytes.len() {
                    return Err(Failure::new(
                        "MCP_OAUTH_INVALID_CALLBACK",
                        "OAuth callback query contains malformed percent encoding",
                    ));
                }
                let high = hex(bytes[index + 1]).ok_or_else(|| {
                    Failure::new(
                        "MCP_OAUTH_INVALID_CALLBACK",
                        "OAuth callback query contains malformed percent encoding",
                    )
                })?;
                let low = hex(bytes[index + 2]).ok_or_else(|| {
                    Failure::new(
                        "MCP_OAUTH_INVALID_CALLBACK",
                        "OAuth callback query contains malformed percent encoding",
                    )
                })?;
                output.push((high << 4) | low);
                index += 3;
            }
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(output).map_err(|_| {
        Failure::new(
            "MCP_OAUTH_INVALID_CALLBACK",
            "OAuth callback query is not valid UTF-8",
        )
    })
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}


pub struct McpOAuthCallbackListener {
    lease: LoopbackLease,
}

impl McpOAuthCallbackListener {
    pub fn lease(&self) -> &LoopbackLease {
        &self.lease
    }

    pub fn close(self, registry: &mut McpOAuthLoopbackRegistry) -> bool {
        registry.release(&self.lease)
    }
}

pub fn start_mcp_oauth_callback_listener<Resolve, Complete, Settled>(
    redirect_url: &str,
    registry: &mut McpOAuthLoopbackRegistry,
    resolve: Resolve,
    on_callback: Complete,
    on_settled: Settled,
) -> Result<McpOAuthCallbackListener, Failure>
where
    Resolve: Fn(&str) -> Option<String> + Send + Sync + 'static,
    Complete: Fn(OAuthCallback) -> Result<(), Failure> + Send + Sync + 'static,
    Settled: Fn(&str) + Send + Sync + 'static,
{
    let redirect = parse_loopback_redirect(redirect_url)?;
    let expected_path = redirect.path;
    let resolve = Arc::new(resolve);
    let on_callback = Arc::new(on_callback);
    let on_settled = Arc::new(on_settled);

    let handler: McpOAuthLoopbackHandler = Arc::new(move |request_target| {
        let disposition = match classify_callback(request_target, &expected_path, {
            let resolve = Arc::clone(&resolve);
            move |state| resolve(state)
        }) {
            Ok(disposition) => disposition,
            Err(_) => {
                return Some(LoopbackHttpResponse::html(
                    400,
                    "<!doctype html><title>OAuth error</title><p>Invalid OAuth callback.</p>",
                ));
            }
        };

        match disposition {
            OAuthCallbackDisposition::Unhandled => None,
            OAuthCallbackDisposition::ProviderError { state, .. }
            | OAuthCallbackDisposition::MissingCode { state, .. } => {
                on_settled(&state);
                Some(LoopbackHttpResponse::html(
                    400,
                    "<!doctype html><title>OAuth error</title><p>OAuth authorization failed.</p>",
                ))
            }
            OAuthCallbackDisposition::Complete(callback) => {
                let state = callback.state.clone();
                let completed = on_callback(callback).is_ok();
                on_settled(&state);
                Some(if completed {
                    LoopbackHttpResponse::html(
                        200,
                        "<!doctype html><title>OAuth complete</title><p>Authorization complete. You may close this window.</p>",
                    )
                } else {
                    LoopbackHttpResponse::html(
                        500,
                        "<!doctype html><title>OAuth error</title><p>OAuth authorization could not be completed.</p>",
                    )
                })
            }
        }
    });

    let lease = registry.acquire_with_handler(redirect_url, handler)?;
    Ok(McpOAuthCallbackListener { lease })
}
