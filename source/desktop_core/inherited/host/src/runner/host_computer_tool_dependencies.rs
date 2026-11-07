use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use prost::{Enumeration, Message, Oneof};
use serde_json::Value;

use crate::extensions::inference::provider_session::ProviderSessionError;
use crate::extensions::cloud_workers::{
    CloudWorkerLeaseScope, CloudWorkerLeasedCapability,
};
use crate::extensions::cloud_workers::extension::CloudWorkerProvisioningExtension;

use super::box_tool_access::RunnerBoxResourcePort;

use crate::r#box::box_shell_command::{
    HostShellArgs, HostShellArgsInput, build_host_shell_args,
};

use super::tools::sand_computer_tool::{
    ComputerActionArgs, ComputerAutoReviewCallback, ComputerCoordinate,
    ComputerPersistImageCallback, ComputerProtocolAction, ComputerToolExecutor,
    ComputerUseResult, ComputerUseSuccess, build_computer_action_sequence,
    persist_computer_screenshot, reported_batch_position,
};

#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedComputerUseArgs {
    pub tool_call_id: String,
    pub actions: Vec<GeneratedComputerAction>,
    pub bind_unmapped_characters: Option<bool>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GeneratedComputerAction {
    MouseMove { coordinate: Option<ComputerCoordinate> },
    Click {
        coordinate: Option<ComputerCoordinate>,
        button: String,
        count: f64,
        modifier_keys: Option<String>,
    },
    MouseDown { button: String },
    MouseUp { button: String },
    Drag {
        path: Vec<ComputerCoordinate>,
        button: String,
        modifier_keys: Option<String>,
    },
    Scroll {
        coordinate: Option<ComputerCoordinate>,
        direction: String,
        amount: f64,
        modifier_keys: Option<String>,
    },
    Type { text: String },
    Key { key: String, hold_duration_ms: Option<f64> },
    Wait { duration_ms: f64 },
    Screenshot,
    CursorPosition,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ComputerProjectionError(pub String);

fn record<'a>(value: &'a Value, action_case: &str) -> Result<&'a serde_json::Map<String, Value>, ComputerProjectionError> {
    value.as_object().ok_or_else(|| {
        ComputerProjectionError(format!("computer action {action_case} has no value"))
    })
}

fn required_number(
    value: Option<&Value>,
    field: &str,
) -> Result<f64, ComputerProjectionError> {
    value
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .ok_or_else(|| ComputerProjectionError(format!("computer action field {field} is not numeric")))
}

fn required_string(
    value: Option<&Value>,
    field: &str,
) -> Result<String, ComputerProjectionError> {
    value
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| ComputerProjectionError(format!("computer action field {field} is not textual")))
}

fn optional_coordinate(value: Option<&Value>) -> Result<Option<ComputerCoordinate>, ComputerProjectionError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let object = record(value, "coordinate")?;
    Ok(Some(ComputerCoordinate {
        x: required_number(object.get("x"), "coordinate.x")? as i64,
        y: required_number(object.get("y"), "coordinate.y")? as i64,
    }))
}

fn button(value: Option<&Value>) -> Result<String, ComputerProjectionError> {
    let value = required_string(value, "button")?;
    if matches!(value.as_str(), "LEFT" | "RIGHT" | "MIDDLE" | "BACK" | "FORWARD") {
        Ok(value)
    } else {
        Err(ComputerProjectionError(format!(
            "unknown computer action button: {value}"
        )))
    }
}

fn direction(value: Option<&Value>) -> Result<String, ComputerProjectionError> {
    let value = required_string(value, "direction")?;
    if matches!(value.as_str(), "UP" | "DOWN" | "LEFT" | "RIGHT") {
        Ok(value)
    } else {
        Err(ComputerProjectionError(format!(
            "unknown computer action direction: {value}"
        )))
    }
}

pub fn generated_action(
    action: &ComputerProtocolAction,
) -> Result<GeneratedComputerAction, ComputerProjectionError> {
    let value = record(&action.value, &action.action_case)?;
    match action.action_case.as_str() {
        "mouseMove" => Ok(GeneratedComputerAction::MouseMove {
            coordinate: optional_coordinate(value.get("coordinate"))?,
        }),
        "click" => Ok(GeneratedComputerAction::Click {
            coordinate: optional_coordinate(value.get("coordinate"))?,
            button: button(value.get("button"))?,
            count: required_number(value.get("count"), "count")?,
            modifier_keys: value
                .get("modifierKeys")
                .map(|value| required_string(Some(value), "modifierKeys"))
                .transpose()?,
        }),
        "mouseDown" => Ok(GeneratedComputerAction::MouseDown {
            button: button(value.get("button"))?,
        }),
        "mouseUp" => Ok(GeneratedComputerAction::MouseUp {
            button: button(value.get("button"))?,
        }),
        "drag" => {
            let raw = value
                .get("path")
                .and_then(Value::as_array)
                .ok_or_else(|| ComputerProjectionError("computer action drag path is not an array".into()))?;
            let mut path = Vec::with_capacity(raw.len());
            for point in raw {
                path.push(optional_coordinate(Some(point))?.ok_or_else(|| {
                    ComputerProjectionError("computer action drag path contains no coordinate".into())
                })?);
            }
            Ok(GeneratedComputerAction::Drag {
                path,
                button: button(value.get("button"))?,
                modifier_keys: value
                    .get("modifierKeys")
                    .map(|value| required_string(Some(value), "modifierKeys"))
                    .transpose()?,
            })
        }
        "scroll" => Ok(GeneratedComputerAction::Scroll {
            coordinate: optional_coordinate(value.get("coordinate"))?,
            direction: direction(value.get("direction"))?,
            amount: required_number(value.get("amount"), "amount")?,
            modifier_keys: value
                .get("modifierKeys")
                .map(|value| required_string(Some(value), "modifierKeys"))
                .transpose()?,
        }),
        "type" => Ok(GeneratedComputerAction::Type {
            text: required_string(value.get("text"), "text")?,
        }),
        "key" => Ok(GeneratedComputerAction::Key {
            key: required_string(value.get("key"), "key")?,
            hold_duration_ms: value
                .get("holdDurationMs")
                .map(|value| required_number(Some(value), "holdDurationMs"))
                .transpose()?,
        }),
        "wait" => Ok(GeneratedComputerAction::Wait {
            duration_ms: required_number(value.get("durationMs"), "durationMs")?,
        }),
        "screenshot" => Ok(GeneratedComputerAction::Screenshot),
        "cursorPosition" => Ok(GeneratedComputerAction::CursorPosition),
        other => Err(ComputerProjectionError(format!(
            "unsupported computer action: {other}"
        ))),
    }
}

pub fn to_generated_computer_use_args(
    tool_call_id: &str,
    actions: &[ComputerProtocolAction],
    bind_unmapped_characters: Option<bool>,
    description: Option<&str>,
) -> Result<GeneratedComputerUseArgs, ComputerProjectionError> {
    Ok(GeneratedComputerUseArgs {
        tool_call_id: tool_call_id.to_string(),
        actions: actions
            .iter()
            .map(generated_action)
            .collect::<Result<Vec<_>, _>>()?,
        bind_unmapped_characters,
        description: description.map(ToOwned::to_owned),
    })
}

#[derive(Debug, Clone, PartialEq)]
pub enum GeneratedComputerUseResult {
    Success {
        screenshot: Option<String>,
        cursor_position: Option<ComputerCoordinate>,
    },
    Error(String),
    Other(String),
}

pub fn from_generated_computer_use_result(
    result: GeneratedComputerUseResult,
) -> ComputerUseResult {
    match result {
        GeneratedComputerUseResult::Success {
            screenshot,
            cursor_position,
        } => ComputerUseResult::Success(ComputerUseSuccess {
            screenshot,
            screenshot_path: None,
            cursor_position,
        }),
        GeneratedComputerUseResult::Error(error) => ComputerUseResult::Error(error),
        GeneratedComputerUseResult::Other(case) => ComputerUseResult::Other(case),
    }
}

pub fn execute_host_shell<ResultType, Assert, Audit, Execute>(
    args: HostShellArgsInput,
    mut assert_no_pending_approval: Assert,
    mut audit_shell_command: Audit,
    execute: Execute,
) -> ResultType
where
    Assert: FnMut(),
    Audit: FnMut(&str),
    Execute: FnOnce(HostShellArgs) -> ResultType,
{
    assert_no_pending_approval();
    audit_shell_command(&args.command);
    execute(build_host_shell_args(args))
}

pub fn resolve_browser_window_index<Context, ErrorType, Ensure, Lookup>(
    context: &Context,
    box_id: &str,
    ensure_ready: Ensure,
    lookup: Lookup,
) -> Result<Option<u32>, ErrorType>
where
    Ensure: FnOnce(&Context, &str) -> Result<(), ErrorType>,
    Lookup: FnOnce(&str) -> Option<u32>,
{
    ensure_ready(context, box_id)?;
    Ok(lookup(box_id))
}


#[derive(Clone, Copy, Debug, PartialEq, Eq, Enumeration)]
#[repr(i32)]
enum ProtoMouseButton {
    Unspecified = 0,
    Left = 1,
    Right = 2,
    Middle = 3,
    Back = 4,
    Forward = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Enumeration)]
#[repr(i32)]
enum ProtoScrollDirection {
    Unspecified = 0,
    Up = 1,
    Down = 2,
    Left = 3,
    Right = 4,
}

#[derive(Clone, PartialEq, Message)]
struct ProtoCoordinate {
    #[prost(int32, tag="1")]
    x: i32,
    #[prost(int32, tag="2")]
    y: i32,
}

#[derive(Clone, PartialEq, Message)]
struct ProtoComputerUseArgs {
    #[prost(string, tag="1")]
    tool_call_id: String,
    #[prost(message, repeated, tag="2")]
    actions: Vec<ProtoComputerUseAction>,
    #[prost(string, optional, tag="3")]
    description: Option<String>,
    #[prost(bool, optional, tag="4")]
    bind_unmapped_characters: Option<bool>,
}

#[derive(Clone, PartialEq, Message)]
struct ProtoComputerUseAction {
    #[prost(oneof="proto_computer_use_action::Action", tags="1,2,3,4,5,6,7,8,9,10,11")]
    action: Option<proto_computer_use_action::Action>,
}

mod proto_computer_use_action {
    use super::*;
    #[derive(Clone, PartialEq, Oneof)]
    pub enum Action {
        #[prost(message, tag="1")] MouseMove(ProtoMouseMoveAction),
        #[prost(message, tag="2")] Click(ProtoClickAction),
        #[prost(message, tag="3")] MouseDown(ProtoMouseDownAction),
        #[prost(message, tag="4")] MouseUp(ProtoMouseUpAction),
        #[prost(message, tag="5")] Drag(ProtoDragAction),
        #[prost(message, tag="6")] Scroll(ProtoScrollAction),
        #[prost(message, tag="7")] Type(ProtoTypeAction),
        #[prost(message, tag="8")] Key(ProtoKeyAction),
        #[prost(message, tag="9")] Wait(ProtoWaitAction),
        #[prost(message, tag="10")] Screenshot(ProtoScreenshotAction),
        #[prost(message, tag="11")] CursorPosition(ProtoCursorPositionAction),
    }
}

#[derive(Clone, PartialEq, Message)]
struct ProtoMouseMoveAction { #[prost(message, optional, tag="1")] coordinate: Option<ProtoCoordinate> }
#[derive(Clone, PartialEq, Message)]
struct ProtoClickAction {
    #[prost(message, optional, tag="1")] coordinate: Option<ProtoCoordinate>,
    #[prost(enumeration="ProtoMouseButton", tag="2")] button: i32,
    #[prost(int32, tag="3")] count: i32,
    #[prost(string, optional, tag="4")] modifier_keys: Option<String>,
}
#[derive(Clone, PartialEq, Message)]
struct ProtoMouseDownAction { #[prost(enumeration="ProtoMouseButton", tag="1")] button: i32 }
#[derive(Clone, PartialEq, Message)]
struct ProtoMouseUpAction { #[prost(enumeration="ProtoMouseButton", tag="1")] button: i32 }
#[derive(Clone, PartialEq, Message)]
struct ProtoDragAction {
    #[prost(message, repeated, tag="1")] path: Vec<ProtoCoordinate>,
    #[prost(enumeration="ProtoMouseButton", tag="2")] button: i32,
    #[prost(string, optional, tag="3")] modifier_keys: Option<String>,
}
#[derive(Clone, PartialEq, Message)]
struct ProtoScrollAction {
    #[prost(message, optional, tag="1")] coordinate: Option<ProtoCoordinate>,
    #[prost(enumeration="ProtoScrollDirection", tag="2")] direction: i32,
    #[prost(int32, tag="3")] amount: i32,
    #[prost(string, optional, tag="4")] modifier_keys: Option<String>,
}
#[derive(Clone, PartialEq, Message)]
struct ProtoTypeAction { #[prost(string, tag="1")] text: String }
#[derive(Clone, PartialEq, Message)]
struct ProtoKeyAction {
    #[prost(string, tag="1")] key: String,
    #[prost(int32, optional, tag="2")] hold_duration_ms: Option<i32>,
}
#[derive(Clone, PartialEq, Message)]
struct ProtoWaitAction { #[prost(int32, tag="1")] duration_ms: i32 }
#[derive(Clone, PartialEq, Message)]
struct ProtoScreenshotAction {}
#[derive(Clone, PartialEq, Message)]
struct ProtoCursorPositionAction {}

#[derive(Clone, PartialEq, Message)]
struct ProtoComputerUseResult {
    #[prost(oneof="proto_computer_use_result::Result", tags="1,2")]
    result: Option<proto_computer_use_result::Result>,
}
mod proto_computer_use_result {
    use super::*;
    #[derive(Clone, PartialEq, Oneof)]
    pub enum Result {
        #[prost(message, tag="1")] Success(ProtoComputerUseSuccess),
        #[prost(message, tag="2")] Error(ProtoComputerUseError),
    }
}
#[derive(Clone, PartialEq, Message)]
struct ProtoComputerUseSuccess {
    #[prost(int32, tag="1")] _action_count: i32,
    #[prost(int32, tag="2")] _duration_ms: i32,
    #[prost(string, optional, tag="3")] screenshot: Option<String>,
    #[prost(string, optional, tag="4")] _log: Option<String>,
    #[prost(string, optional, tag="5")] _screenshot_path: Option<String>,
    #[prost(message, optional, tag="6")] cursor_position: Option<ProtoCoordinate>,
}
#[derive(Clone, PartialEq, Message)]
struct ProtoComputerUseError {
    #[prost(string, tag="1")] error: String,
}

fn proto_coordinate(value: ComputerCoordinate) -> Result<ProtoCoordinate, ComputerProjectionError> {
    Ok(ProtoCoordinate {
        x: i32::try_from(value.x).map_err(|_| ComputerProjectionError("computer x coordinate is out of int32 range".into()))?,
        y: i32::try_from(value.y).map_err(|_| ComputerProjectionError("computer y coordinate is out of int32 range".into()))?,
    })
}

fn proto_button(value: &str) -> Result<i32, ComputerProjectionError> {
    Ok(match value {
        "LEFT" => ProtoMouseButton::Left as i32,
        "RIGHT" => ProtoMouseButton::Right as i32,
        "MIDDLE" => ProtoMouseButton::Middle as i32,
        "BACK" => ProtoMouseButton::Back as i32,
        "FORWARD" => ProtoMouseButton::Forward as i32,
        other => return Err(ComputerProjectionError(format!("unknown computer action button: {other}"))),
    })
}

fn proto_direction(value: &str) -> Result<i32, ComputerProjectionError> {
    Ok(match value {
        "UP" => ProtoScrollDirection::Up as i32,
        "DOWN" => ProtoScrollDirection::Down as i32,
        "LEFT" => ProtoScrollDirection::Left as i32,
        "RIGHT" => ProtoScrollDirection::Right as i32,
        other => return Err(ComputerProjectionError(format!("unknown computer action direction: {other}"))),
    })
}

fn finite_i32(value: f64, field: &str) -> Result<i32, ComputerProjectionError> {
    if !value.is_finite() || value.fract() != 0.0 || value < i32::MIN as f64 || value > i32::MAX as f64 {
        return Err(ComputerProjectionError(format!("computer action field {field} is not an int32")));
    }
    Ok(value as i32)
}

fn proto_action(value: &GeneratedComputerAction) -> Result<ProtoComputerUseAction, ComputerProjectionError> {
    use proto_computer_use_action::Action;
    let action = match value {
        GeneratedComputerAction::MouseMove { coordinate } => Action::MouseMove(ProtoMouseMoveAction {
            coordinate: coordinate.map(proto_coordinate).transpose()?,
        }),
        GeneratedComputerAction::Click { coordinate, button, count, modifier_keys } => Action::Click(ProtoClickAction {
            coordinate: coordinate.map(proto_coordinate).transpose()?,
            button: proto_button(button)?,
            count: finite_i32(*count, "count")?,
            modifier_keys: modifier_keys.clone(),
        }),
        GeneratedComputerAction::MouseDown { button } => Action::MouseDown(ProtoMouseDownAction { button: proto_button(button)? }),
        GeneratedComputerAction::MouseUp { button } => Action::MouseUp(ProtoMouseUpAction { button: proto_button(button)? }),
        GeneratedComputerAction::Drag { path, button, modifier_keys } => Action::Drag(ProtoDragAction {
            path: path.iter().copied().map(proto_coordinate).collect::<Result<Vec<_>, _>>()?,
            button: proto_button(button)?,
            modifier_keys: modifier_keys.clone(),
        }),
        GeneratedComputerAction::Scroll { coordinate, direction, amount, modifier_keys } => Action::Scroll(ProtoScrollAction {
            coordinate: coordinate.map(proto_coordinate).transpose()?,
            direction: proto_direction(direction)?,
            amount: finite_i32(*amount, "amount")?,
            modifier_keys: modifier_keys.clone(),
        }),
        GeneratedComputerAction::Type { text } => Action::Type(ProtoTypeAction { text: text.clone() }),
        GeneratedComputerAction::Key { key, hold_duration_ms } => Action::Key(ProtoKeyAction {
            key: key.clone(),
            hold_duration_ms: hold_duration_ms.map(|value| finite_i32(value, "holdDurationMs")).transpose()?,
        }),
        GeneratedComputerAction::Wait { duration_ms } => Action::Wait(ProtoWaitAction {
            duration_ms: finite_i32(*duration_ms, "durationMs")?,
        }),
        GeneratedComputerAction::Screenshot => Action::Screenshot(ProtoScreenshotAction {}),
        GeneratedComputerAction::CursorPosition => Action::CursorPosition(ProtoCursorPositionAction {}),
    };
    Ok(ProtoComputerUseAction { action: Some(action) })
}

pub fn encode_generated_computer_use_args(
    args: &GeneratedComputerUseArgs,
) -> Result<Vec<u8>, ComputerProjectionError> {
    let proto = ProtoComputerUseArgs {
        tool_call_id: args.tool_call_id.clone(),
        actions: args.actions.iter().map(proto_action).collect::<Result<Vec<_>, _>>()?,
        description: args.description.clone(),
        bind_unmapped_characters: args.bind_unmapped_characters,
    };
    Ok(proto.encode_to_vec())
}

pub fn decode_generated_computer_use_result(
    bytes: &[u8],
) -> Result<GeneratedComputerUseResult, ComputerProjectionError> {
    let decoded = ProtoComputerUseResult::decode(bytes)
        .map_err(|error| ComputerProjectionError(format!("invalid ComputerUseResult protobuf: {error}")))?;
    Ok(match decoded.result {
        Some(proto_computer_use_result::Result::Success(success)) => {
            GeneratedComputerUseResult::Success {
                screenshot: success.screenshot,
                cursor_position: success.cursor_position.map(|point| ComputerCoordinate {
                    x: i64::from(point.x),
                    y: i64::from(point.y),
                }),
            }
        }
        Some(proto_computer_use_result::Result::Error(error)) => GeneratedComputerUseResult::Error(error.error),
        None => GeneratedComputerUseResult::Other("missing_result".into()),
    })
}

pub type ComputerAvailabilityCheck = Arc<dyn Fn(&ComputerActionArgs) -> Result<(), ProviderSessionError> + Send + Sync>;
pub type ComputerActionReportCallback = Arc<dyn Fn(&crate::runner::tools::sand_computer_tool::ReportedComputerAction, &str) + Send + Sync>;
pub type ComputerPostActionCallback = Arc<dyn Fn(&str) + Send + Sync>;

pub struct ProductionComputerToolExecutor {
    box_resources: Arc<dyn RunnerBoxResourcePort>,
    auto_review: Option<ComputerAutoReviewCallback>,
    persist_image: Option<ComputerPersistImageCallback>,
    availability_check: Option<ComputerAvailabilityCheck>,
    action_report: Option<ComputerActionReportCallback>,
    post_action: Option<ComputerPostActionCallback>,
    bind_unmapped_characters: bool,
}

const MAX_CLOUD_WORKER_COMPUTER_REPLAY_KEYS: usize = 1024;

/// Fenced consumer adapter for an already enrolled Cloud Worker capability.
///
/// This adapter owns no Computer grant, device enrollment, sensitive-result
/// storage or Computer side effect. It revalidates the ephemeral worker
/// capability and delegates the action to exactly one existing
/// `ComputerToolExecutor`.
pub struct CloudWorkerScopedComputerToolExecutor {
    extension: Arc<CloudWorkerProvisioningExtension>,
    capability: CloudWorkerLeasedCapability,
    expected_scope: CloudWorkerLeaseScope,
    delegate: Arc<dyn ComputerToolExecutor>,
    replay_keys: Mutex<HashSet<String>>,
}

impl CloudWorkerScopedComputerToolExecutor {
    pub fn new(
        extension: Arc<CloudWorkerProvisioningExtension>,
        capability: CloudWorkerLeasedCapability,
        expected_scope: CloudWorkerLeaseScope,
        delegate: Arc<dyn ComputerToolExecutor>,
    ) -> Self {
        Self {
            extension,
            capability,
            expected_scope,
            delegate,
            replay_keys: Mutex::new(HashSet::new()),
        }
    }

    fn validate(&self, tool_call_id: &str) -> Result<(), ProviderSessionError> {
        if self.capability.scope() != &self.expected_scope {
            return Err(ProviderSessionError::Tool(
                "cloud-worker Computer capability scope does not match the consumer scope".into(),
            ));
        }
        if tool_call_id.trim().is_empty() {
            return Err(ProviderSessionError::Tool(
                "cloud-worker Computer operation requires a replay key".into(),
            ));
        }
        self.extension
            .validate_current_capability(&self.capability)
            .map_err(|error| ProviderSessionError::Tool(format!(
                "cloud-worker Computer capability rejected before delegation: {error}"
            )))?;
        let replay_key = tool_call_id.trim();
        let mut replay_keys = self.replay_keys.lock().map_err(|_| {
            ProviderSessionError::Tool(
                "cloud-worker Computer replay-key guard is unavailable".into(),
            )
        })?;
        if replay_keys.len() >= MAX_CLOUD_WORKER_COMPUTER_REPLAY_KEYS
            || !replay_keys.insert(replay_key.to_string())
        {
            return Err(ProviderSessionError::Tool(
                "cloud-worker Computer replay key was already used or capacity is exhausted".into(),
            ));
        }
        Ok(())
    }
}

impl ComputerToolExecutor for CloudWorkerScopedComputerToolExecutor {
    fn execute(
        &self,
        args: &ComputerActionArgs,
        tool_call_id: &str,
    ) -> Result<ComputerUseResult, ProviderSessionError> {
        self.validate(tool_call_id)?;
        self.delegate.execute(args, tool_call_id)
    }
}

impl ProductionComputerToolExecutor {
    pub fn new(box_resources: Arc<dyn RunnerBoxResourcePort>) -> Self {
        Self {
            box_resources,
            auto_review: None,
            persist_image: None,
            availability_check: None,
            action_report: None,
            post_action: None,
            bind_unmapped_characters: false,
        }
    }

    pub fn with_auto_review_callback(mut self, callback: ComputerAutoReviewCallback) -> Self {
        self.auto_review = Some(callback);
        self
    }

    pub fn with_persist_image_callback(mut self, callback: ComputerPersistImageCallback) -> Self {
        self.persist_image = Some(callback);
        self
    }

    pub fn with_availability_check(mut self, check: ComputerAvailabilityCheck) -> Self {
        self.availability_check = Some(check);
        self
    }

    pub fn with_action_report_callback(mut self, callback: ComputerActionReportCallback) -> Self {
        self.action_report = Some(callback);
        self
    }

    pub fn with_post_action_callback(mut self, callback: ComputerPostActionCallback) -> Self {
        self.post_action = Some(callback);
        self
    }

    pub fn with_unicode_typing(mut self, enabled: bool) -> Self {
        self.bind_unmapped_characters = enabled;
        self
    }
}

impl ComputerToolExecutor for ProductionComputerToolExecutor {
    fn execute(
        &self,
        args: &ComputerActionArgs,
        tool_call_id: &str,
    ) -> Result<ComputerUseResult, ProviderSessionError> {
        if let Some(check) = self.availability_check.as_ref() {
            check(args)?;
        }
        let actions = build_computer_action_sequence(args, None)
            .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
        if let Some(auto_review) = self.auto_review.as_ref() {
            auto_review(args, tool_call_id)?;
        }
        if let Some(reported) = reported_batch_position(
            &std::iter::once(args.clone())
                .chain(args.then_actions.iter().cloned())
                .collect::<Vec<_>>(),
        ) {
            if let Some(report) = self.action_report.as_ref() {
                report(&reported, tool_call_id);
            }
        }
        let generated = to_generated_computer_use_args(
            tool_call_id,
            &actions,
            self.bind_unmapped_characters.then_some(true),
            args.description.as_deref().map(str::trim).filter(|value| !value.is_empty()),
        ).map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
        let request = encode_generated_computer_use_args(&generated)
            .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
        let action_case = actions.first().map(|action| action.action_case.as_str());
        let response = self
            .box_resources
            .execute_computer_use_protobuf_with_action_case(request, action_case)?;
        let generated_result = decode_generated_computer_use_result(&response)
            .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
        let mut result = from_generated_computer_use_result(generated_result);
        persist_computer_screenshot(
            &mut result,
            self.persist_image.as_ref().map(|persist| {
                move |bytes: &[u8], mime: &str| persist(bytes, mime)
            }),
        ).map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
        if let Some(post_action) = self.post_action.as_ref() {
            post_action(tool_call_id);
        }
        Ok(result)
    }
}
