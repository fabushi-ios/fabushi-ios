use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use url::Url;

use crate::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use crate::runner::routed_provider_runtime::{
    RoutedProviderCancellation, RoutedToolBridge,
};

use super::send_message_tool::SendMessageSink;

pub const SAND_REQUEST_BOX_HELP_TOOL_NAME: &str = "request_box_help";
pub const WAITING_USER_CANCELLATION_PREFIX: &str = "waiting-user:box-help:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxHelpRequest {
    pub instruction: String,
    pub reason: Option<String>,
    pub domain: Option<String>,
    pub idp_domain: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxHelpOutcome {
    Started { request_id: String },
    AlreadyPending { request_id: String, instruction: String },
}

impl BoxHelpOutcome {
    pub fn request_id(&self) -> &str {
        match self {
            Self::Started { request_id } | Self::AlreadyPending { request_id, .. } => request_id,
        }
    }
}

pub fn normalize_box_help_domain(raw: &str) -> Option<String> {
    let value = raw.trim().to_ascii_lowercase();
    if value.is_empty() {
        return None;
    }
    let candidate = if value.contains("://") { value } else { format!("https://{value}") };
    let url = Url::parse(&candidate).ok()?;
    let host = url.host_str()?;
    let host = host.strip_prefix("www.").unwrap_or(host);
    (!host.is_empty()).then(|| host.to_string())
}

pub fn connector_card_emission_to_message(connector: &str, server_id: &str, variant: &str) -> Value {
    json!({"type":"connector","connector":connector,"serverId":server_id,"variant":variant})
}

pub fn box_help_tool_definition() -> RoutedToolDefinition {
    RoutedToolDefinition {
        name: SAND_REQUEST_BOX_HELP_TOOL_NAME.to_string(),
        provider_identifier: "fabushi-runner".to_string(),
        tool_name: SAND_REQUEST_BOX_HELP_TOOL_NAME.to_string(),
        description: Some(
            "Hand the box desktop to the user for a step only they can do, such as login, SSO, passkey, 2FA, captcha, or payment confirmation. The turn ends and resumes after the user hands the box back.".to_string(),
        ),
        input_schema: json!({
            "type":"object",
            "required":["instruction"],
            "additionalProperties":false,
            "properties":{
                "instruction":{"type":"string","minLength":1},
                "reason":{"type":"string","enum":["auth","captcha","payment","other"]},
                "domain":{"type":"string"},
                "idp_domain":{"type":"string"}
            }
        }),
    }
}

pub struct BoxHelpToolBridge {
    delegate: Arc<dyn RoutedToolBridge>,
    sink: Arc<dyn SendMessageSink>,
    cancellation: RoutedProviderCancellation,
}

impl BoxHelpToolBridge {
    pub fn new(
        delegate: Arc<dyn RoutedToolBridge>,
        sink: Arc<dyn SendMessageSink>,
        cancellation: RoutedProviderCancellation,
    ) -> Self {
        Self { delegate, sink, cancellation }
    }
}

impl RoutedToolBridge for BoxHelpToolBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        let mut tools = self.delegate.list_tools()?;
        tools.retain(|tool| {
            tool.name != SAND_REQUEST_BOX_HELP_TOOL_NAME
                && tool.tool_name != SAND_REQUEST_BOX_HELP_TOOL_NAME
        });
        tools.insert(0, box_help_tool_definition());
        Ok(tools)
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: Value,
        tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        if tool.name != SAND_REQUEST_BOX_HELP_TOOL_NAME
            && tool.tool_name != SAND_REQUEST_BOX_HELP_TOOL_NAME
        {
            return self.delegate.call_tool(tool, args, tool_call_id);
        }
        let object = args.as_object().ok_or_else(|| {
            ProviderSessionError::Tool("request_box_help arguments must be an object".into())
        })?;
        let instruction = object
            .get("instruction")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ProviderSessionError::Tool(
                "instruction is required for request_box_help".into()
            ))?
            .to_string();
        let reason = object.get("reason").and_then(Value::as_str)
            .filter(|value| matches!(*value, "auth" | "captcha" | "payment" | "other"))
            .map(ToOwned::to_owned);
        let domain = object.get("domain").and_then(Value::as_str).and_then(normalize_box_help_domain);
        let idp_domain = object.get("idp_domain").and_then(Value::as_str).and_then(normalize_box_help_domain);
        let timestamp_ms = SystemTime::now().duration_since(UNIX_EPOCH)
            .map(|value| value.as_millis().min(u64::MAX as u128) as u64)
            .unwrap_or_default();
        let outcome = self.sink.request_box_help(
            BoxHelpRequest { instruction, reason, domain, idp_domain },
            timestamp_ms,
            tool_call_id,
        )?;
        self.cancellation.cancel(format!(
            "{WAITING_USER_CANCELLATION_PREFIX}{}",
            outcome.request_id()
        ));
        Ok(Value::String(match outcome {
            BoxHelpOutcome::Started { .. } =>
                "Handed the box to the user. They have control now; wait for them to hand it back, and you'll be resumed automatically.".to_string(),
            BoxHelpOutcome::AlreadyPending { instruction, .. } =>
                format!("The user still has the box: you handed it to them for \"{instruction}\" and they haven't handed it back, so this request was NOT sent. Do not ask again; wait for the user to hand the box back."),
        }))
    }
}
