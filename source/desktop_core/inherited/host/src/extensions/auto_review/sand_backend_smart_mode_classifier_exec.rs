use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use prost::{Message, Oneof};
use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

use crate::cursor_backend::{
    CursorBackendError, CursorRequestCancellation, resolve_sand_ghost_mode_header,
    send_cursor_unary_cancellable,
};
use crate::extensions::auth::credential_renewer::get_configured_backend_url;
use crate::extensions::auth::extension::HostAuthExtension;
use crate::runner::sand_auto_review_classifier_run::{
    AutoReviewClassifierError, AutoReviewClassifierRequest,
    SandAutoReviewClassifierExecutor, SmartModeClassifierDecision,
    SmartModeClassifierMeasurement, SmartModeClassifierResult, SmartModeClassifierSuccess,
};

pub const DASHBOARD_CLASSIFY_SAND_AUTO_REVIEW_PATH: &str =
    "/aiserver.v1.DashboardService/ClassifySandAutoReview";
pub const SAND_AUTO_REVIEW_CLASSIFIER_TIMEOUT_MS: u64 = 10_000;

pub type ClassifierAccessToken =
    Arc<dyn Fn() -> Result<String, String> + Send + Sync + 'static>;
pub type ClassifierMachineId =
    Arc<dyn Fn() -> Result<String, String> + Send + Sync + 'static>;
pub type SmartModeClassifierMeasurementReporter =
    Arc<dyn Fn(SmartModeClassifierMeasurement) + Send + Sync + 'static>;

static CLASSIFIER_MEASUREMENT_REPORTER: OnceLock<
    Mutex<Option<SmartModeClassifierMeasurementReporter>>,
> = OnceLock::new();

fn classifier_measurement_reporter(
) -> &'static Mutex<Option<SmartModeClassifierMeasurementReporter>> {
    CLASSIFIER_MEASUREMENT_REPORTER.get_or_init(|| Mutex::new(None))
}

pub fn pin_sand_auto_review_classifier_measurement_reporter(
    reporter: Option<SmartModeClassifierMeasurementReporter>,
) {
    *classifier_measurement_reporter()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = reporter;
}

fn report_classifier_measurement(measurement: SmartModeClassifierMeasurement) {
    let reporter = classifier_measurement_reporter()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    if let Some(reporter) = reporter {
        reporter(measurement);
    }
}

#[derive(Clone)]
pub struct SandBackendSmartModeClassifierOptions {
    pub backend_url: String,
    pub get_access_token: ClassifierAccessToken,
    pub get_machine_id: ClassifierMachineId,
}

#[derive(Debug, Error)]
pub enum SandSmartModeClassifierError {
    #[error("could not configure Sand Auto-review classifier backend: {0}")]
    Configuration(String),
    #[error("could not acquire Sand Auto-review classifier credential: {0}")]
    Authentication(String),
    #[error("could not project Sand Auto-review classifier request: {0}")]
    Projection(String),
    #[error("Sand Auto-review classifier backend request failed: {0}")]
    Backend(#[from] CursorBackendError),
    #[error("{0}")]
    Protocol(String),
}

pub struct SandBackendSmartModeClassifierExecutor {
    options: SandBackendSmartModeClassifierOptions,
    cancellation: CursorRequestCancellation,
}

impl SandBackendSmartModeClassifierExecutor {
    pub fn new(options: SandBackendSmartModeClassifierOptions) -> Self {
        Self::with_cancellation(options, Arc::new(|| false))
    }

    pub fn with_cancellation(
        options: SandBackendSmartModeClassifierOptions,
        cancellation: CursorRequestCancellation,
    ) -> Self {
        Self {
            options,
            cancellation,
        }
    }

    pub fn production(
        auth: Arc<HostAuthExtension>,
    ) -> Result<Self, SandSmartModeClassifierError> {
        let backend_url = get_configured_backend_url()
            .map_err(|error| SandSmartModeClassifierError::Configuration(error.to_string()))?;
        let token_auth = Arc::clone(&auth);
        let machine_auth = Arc::clone(&auth);
        Ok(Self::new(SandBackendSmartModeClassifierOptions {
            backend_url,
            get_access_token: Arc::new(move || {
                token_auth
                    .get_access_token()
                    .map_err(|error| error.to_string())
            }),
            get_machine_id: Arc::new(move || {
                machine_auth
                    .get_machine_id()
                    .map_err(|error| error.to_string())
            }),
        }))
    }

    pub fn production_with_cancellation(
        auth: Arc<HostAuthExtension>,
        cancellation: CursorRequestCancellation,
    ) -> Result<Self, SandSmartModeClassifierError> {
        let backend_url = get_configured_backend_url()
            .map_err(|error| SandSmartModeClassifierError::Configuration(error.to_string()))?;
        let token_auth = Arc::clone(&auth);
        let machine_auth = Arc::clone(&auth);
        Ok(Self::with_cancellation(
            SandBackendSmartModeClassifierOptions {
                backend_url,
                get_access_token: Arc::new(move || {
                    token_auth
                        .get_access_token()
                        .map_err(|error| error.to_string())
                }),
                get_machine_id: Arc::new(move || {
                    machine_auth
                        .get_machine_id()
                        .map_err(|error| error.to_string())
                }),
            },
            cancellation,
        ))
    }

    pub fn execute_projected<Target, ConversationMessage>(
        &self,
        request: AutoReviewClassifierRequest<'_, Target, ConversationMessage>,
    ) -> Result<SmartModeClassifierResult, SandSmartModeClassifierError>
    where
        Target: Serialize,
        ConversationMessage: Serialize,
    {
        let access_token = (self.options.get_access_token)()
            .map_err(SandSmartModeClassifierError::Authentication)?;
        let machine_id = (self.options.get_machine_id)()
            .map_err(SandSmartModeClassifierError::Authentication)?;

        let target = project_target(&request.target)?;
        let conversation_context = request
            .conversation_context
            .iter()
            .map(project_conversation_message)
            .collect::<Result<Vec<_>, _>>()?;

        // The Grok Host intentionally keeps toolCallId local to measurement
        // telemetry. It is never serialized into ClassifySandAutoReview.
        let args = SmartModeClassifierArgsWire {
            tool_call_id: String::new(),
            parent_conversation_id: Some(request.parent_conversation_id.to_string()),
            target: Some(target),
            conversation_context,
        };
        let mode = match request.mode.trim() {
            "" => "enforce",
            value => value,
        };
        let envelope = ClassifySandAutoReviewRequestWire {
            args: Some(args),
            // The current Runner contract executes one measured classifier
            // attempt. Its first (and only) backend attempt therefore has
            // the frozen Grok attempt index 0.
            attempt_index: Some(0),
            mode: Some(mode.to_string()),
        };
        let mut body = Vec::new();
        envelope
            .encode(&mut body)
            .map_err(|error| SandSmartModeClassifierError::Projection(error.to_string()))?;

        let ghost_mode = resolve_sand_ghost_mode_header(
            &self.options.backend_url,
            &access_token,
            &machine_id,
        );
        let response = send_cursor_unary_cancellable(
            &self.options.backend_url,
            &access_token,
            &machine_id,
            DASHBOARD_CLASSIFY_SAND_AUTO_REVIEW_PATH,
            &body,
            SAND_AUTO_REVIEW_CLASSIFIER_TIMEOUT_MS,
            ghost_mode,
            Arc::clone(&self.cancellation),
        )?;
        let response = ClassifySandAutoReviewResponseWire::decode(response.as_slice())
            .map_err(|error| {
                SandSmartModeClassifierError::Protocol(format!(
                    "ClassifySandAutoReview returned invalid protobuf: {error}"
                ))
            })?;
        let Some(result) = response.result else {
            return Ok(SmartModeClassifierResult::Missing);
        };
        Ok(project_result(result))
    }
}

pub fn create_sand_backend_smart_mode_classifier_executor(
    auth: Arc<HostAuthExtension>,
) -> Result<SandBackendSmartModeClassifierExecutor, SandSmartModeClassifierError> {
    SandBackendSmartModeClassifierExecutor::production(auth)
}

pub fn create_sand_backend_smart_mode_classifier_executor_with_cancellation(
    auth: Arc<HostAuthExtension>,
    cancellation: CursorRequestCancellation,
) -> Result<SandBackendSmartModeClassifierExecutor, SandSmartModeClassifierError> {
    SandBackendSmartModeClassifierExecutor::production_with_cancellation(
        auth,
        cancellation,
    )
}

impl<Target, ConversationMessage>
    SandAutoReviewClassifierExecutor<Target, ConversationMessage>
    for SandBackendSmartModeClassifierExecutor
where
    Target: Serialize,
    ConversationMessage: Serialize,
{
    fn execute(
        &mut self,
        request: AutoReviewClassifierRequest<'_, Target, ConversationMessage>,
    ) -> Result<SmartModeClassifierResult, AutoReviewClassifierError> {
        match self.execute_projected(request) {
            Ok(result) => Ok(result),
            Err(SandSmartModeClassifierError::Backend(
                CursorBackendError::Cancelled(reason),
            )) => Err(AutoReviewClassifierError::Aborted(reason)),
            Err(SandSmartModeClassifierError::Backend(
                CursorBackendError::Timeout(reason),
            )) => Err(AutoReviewClassifierError::Timeout(reason)),
            Err(error) => Err(AutoReviewClassifierError::Failed(error.to_string())),
        }
    }

    fn record_measurement(&mut self, measurement: SmartModeClassifierMeasurement) {
        report_classifier_measurement(measurement);
    }
}

fn project_target<Target: Serialize>(
    target: &Target,
) -> Result<SmartModeRiskTargetWire, SandSmartModeClassifierError> {
    let value = serde_json::to_value(target)
        .map_err(|error| SandSmartModeClassifierError::Projection(error.to_string()))?;
    let object = value.as_object().ok_or_else(|| {
        SandSmartModeClassifierError::Projection(
            "classifier target must serialize to an object".into(),
        )
    })?;
    let action = object
        .get("action")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            SandSmartModeClassifierError::Projection(
                "classifier target requires a non-empty action".into(),
            )
        })?
        .to_string();
    let arguments = object
        .get("arguments")
        .filter(|value| !value.is_null())
        .map(json_object_to_proto_struct)
        .transpose()?;
    Ok(SmartModeRiskTargetWire { action, arguments })
}

fn project_conversation_message<MessageValue: Serialize>(
    message: &MessageValue,
) -> Result<SmartModeClassifierConversationMessageWire, SandSmartModeClassifierError> {
    let value = serde_json::to_value(message)
        .map_err(|error| SandSmartModeClassifierError::Projection(error.to_string()))?;
    let object = value.as_object().ok_or_else(|| {
        SandSmartModeClassifierError::Projection(
            "classifier conversation message must serialize to an object".into(),
        )
    })?;
    let role = object
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let content = object
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Ok(SmartModeClassifierConversationMessageWire { role, content })
}

fn json_object_to_proto_struct(
    value: &Value,
) -> Result<ProtoStruct, SandSmartModeClassifierError> {
    let object = value.as_object().ok_or_else(|| {
        SandSmartModeClassifierError::Projection(
            "classifier target arguments must be an object".into(),
        )
    })?;
    let fields = object
        .iter()
        .map(|(key, value)| Ok((key.clone(), json_to_proto_value(value)?)))
        .collect::<Result<HashMap<_, _>, SandSmartModeClassifierError>>()?;
    Ok(ProtoStruct { fields })
}

fn json_to_proto_value(
    value: &Value,
) -> Result<ProtoValue, SandSmartModeClassifierError> {
    use proto_value::Kind;
    let kind = match value {
        Value::Null => Kind::NullValue(0),
        Value::Bool(value) => Kind::BoolValue(*value),
        Value::Number(value) => Kind::NumberValue(value.as_f64().ok_or_else(|| {
            SandSmartModeClassifierError::Projection(
                "classifier numeric argument cannot be represented as f64".into(),
            )
        })?),
        Value::String(value) => Kind::StringValue(value.clone()),
        Value::Array(values) => Kind::ListValue(ProtoListValue {
            values: values
                .iter()
                .map(json_to_proto_value)
                .collect::<Result<Vec<_>, _>>()?,
        }),
        Value::Object(_) => Kind::StructValue(json_object_to_proto_struct(value)?),
    };
    Ok(ProtoValue { kind: Some(kind) })
}

fn project_result(result: SmartModeClassifierResultWire) -> SmartModeClassifierResult {
    match result.result {
        Some(smart_mode_classifier_result_wire::Result::Success(success)) => {
            let decision = match success.decision {
                0 => SmartModeClassifierDecision::Unspecified,
                1 => SmartModeClassifierDecision::Allow,
                2 => SmartModeClassifierDecision::Block,
                _ => SmartModeClassifierDecision::Unknown,
            };
            SmartModeClassifierResult::Success(SmartModeClassifierSuccess {
                decision,
                block_reason: success.block_reason,
                proposed_allow_rule: success.proposed_allow_rule,
            })
        }
        Some(smart_mode_classifier_result_wire::Result::Error(error)) => {
            let (failure_reason, retryable) = parse_failure_metadata(&error.error);
            SmartModeClassifierResult::Error {
                failure_reason,
                retryable,
            }
        }
        None => SmartModeClassifierResult::Missing
    }
}

const SMART_MODE_CLASSIFIER_FAILURE_METADATA_MARKER: &str =
    "\n\nSmartModeClassifierFailureMetadata:";

fn parse_failure_metadata(error: &str) -> (Option<String>, Option<bool>) {
    let Some((_, encoded)) = error.split_once(SMART_MODE_CLASSIFIER_FAILURE_METADATA_MARKER) else {
        return (None, None);
    };
    let Ok(value) = serde_json::from_str::<Value>(encoded) else {
        return (None, None);
    };
    let Some(object) = value.as_object() else {
        return (None, None);
    };
    (
        object
            .get("failureReason")
            .and_then(Value::as_str)
            .map(str::to_string),
        object.get("retryable").and_then(Value::as_bool),
    )
}

#[derive(Clone, PartialEq, Message)]
struct SmartModeClassifierConversationMessageWire {
    #[prost(string, tag = "1")]
    role: String,
    #[prost(string, tag = "2")]
    content: String,
}

#[derive(Clone, PartialEq, Message)]
struct SmartModeClassifierArgsWire {
    #[prost(string, tag = "1")]
    tool_call_id: String,
    #[prost(string, optional, tag = "2")]
    parent_conversation_id: Option<String>,
    #[prost(message, optional, tag = "3")]
    target: Option<SmartModeRiskTargetWire>,
    #[prost(message, repeated, tag = "4")]
    conversation_context: Vec<SmartModeClassifierConversationMessageWire>,
}

#[derive(Clone, PartialEq, Message)]
struct SmartModeRiskTargetWire {
    #[prost(string, tag = "1")]
    action: String,
    #[prost(message, optional, tag = "2")]
    arguments: Option<ProtoStruct>,
}

#[derive(Clone, PartialEq, Message)]
struct ClassifySandAutoReviewRequestWire {
    #[prost(message, optional, tag = "1")]
    args: Option<SmartModeClassifierArgsWire>,
    #[prost(uint32, optional, tag = "2")]
    attempt_index: Option<u32>,
    #[prost(string, optional, tag = "3")]
    mode: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
struct ClassifySandAutoReviewResponseWire {
    #[prost(message, optional, tag = "1")]
    result: Option<SmartModeClassifierResultWire>,
}

#[derive(Clone, PartialEq, Message)]
struct SmartModeClassifierResultWire {
    #[prost(oneof = "smart_mode_classifier_result_wire::Result", tags = "1, 2")]
    result: Option<smart_mode_classifier_result_wire::Result>,
}

mod smart_mode_classifier_result_wire {
    use super::{SmartModeClassifierErrorWire, SmartModeClassifierSuccessWire};
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Result {
        #[prost(message, tag = "1")]
        Success(SmartModeClassifierSuccessWire),
        #[prost(message, tag = "2")]
        Error(SmartModeClassifierErrorWire),
    }
}

#[derive(Clone, PartialEq, Message)]
struct SmartModeClassifierSuccessWire {
    #[prost(int32, tag = "1")]
    decision: i32,
    #[prost(string, optional, tag = "2")]
    block_reason: Option<String>,
    #[prost(string, optional, tag = "3")]
    proposed_allow_rule: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
struct SmartModeClassifierErrorWire {
    #[prost(string, tag = "1")]
    error: String,
}

#[derive(Clone, PartialEq, Message)]
struct ProtoStruct {
    #[prost(map = "string, message", tag = "1")]
    fields: HashMap<String, ProtoValue>,
}

#[derive(Clone, PartialEq, Message)]
struct ProtoValue {
    #[prost(oneof = "proto_value::Kind", tags = "1, 2, 3, 4, 5, 6")]
    kind: Option<proto_value::Kind>,
}

mod proto_value {
    use super::{ProtoListValue, ProtoStruct};
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Kind {
        #[prost(enumeration = "ProtoNullValue", tag = "1")]
        NullValue(i32),
        #[prost(double, tag = "2")]
        NumberValue(f64),
        #[prost(string, tag = "3")]
        StringValue(String),
        #[prost(bool, tag = "4")]
        BoolValue(bool),
        #[prost(message, tag = "5")]
        StructValue(ProtoStruct),
        #[prost(message, tag = "6")]
        ListValue(ProtoListValue),
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
    #[repr(i32)]
    pub enum ProtoNullValue {
        NullValue = 0,
    }
}

#[derive(Clone, PartialEq, Message)]
struct ProtoListValue {
    #[prost(message, repeated, tag = "1")]
    values: Vec<ProtoValue>,
}
