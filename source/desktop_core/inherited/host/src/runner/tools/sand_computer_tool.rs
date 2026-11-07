use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Value, json};

use crate::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use crate::runner::routed_provider_runtime::RoutedToolBridge;

use crate::runner::sand_auto_review::{
    SandAutoReviewMode,
};
use crate::runner::sand_computer_auto_review::is_sand_computer_auto_review_bypass_action;

pub const SAND_COMPUTER_MAX_WAIT_MS: u64 = 30_000;
pub const SAND_COMPUTER_MAX_FOLLOW_UP_ACTIONS: usize = 9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputerActionName {
    Screenshot,
    Click,
    DoubleClick,
    Move,
    Drag,
    Type,
    Key,
    Scroll,
    Wait,
}

impl ComputerActionName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Screenshot => "screenshot",
            Self::Click => "click",
            Self::DoubleClick => "doubleClick",
            Self::Move => "move",
            Self::Drag => "drag",
            Self::Type => "type",
            Self::Key => "key",
            Self::Scroll => "scroll",
            Self::Wait => "wait",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButtonInput {
    Left,
    Right,
    Middle,
}

impl MouseButtonInput {
    fn generated_name(self) -> &'static str {
        match self {
            Self::Left => "LEFT",
            Self::Right => "RIGHT",
            Self::Middle => "MIDDLE",
        }
    }

    fn user_name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Middle => "middle",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDirectionInput {
    Up,
    Down,
    Left,
    Right,
}

impl ScrollDirectionInput {
    fn generated_name(self) -> &'static str {
        match self {
            Self::Up => "UP",
            Self::Down => "DOWN",
            Self::Left => "LEFT",
            Self::Right => "RIGHT",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComputerCoordinate {
    pub x: i64,
    pub y: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputerActionArgs {
    pub action: ComputerActionName,
    pub x: Option<i64>,
    pub y: Option<i64>,
    pub x2: Option<i64>,
    pub y2: Option<i64>,
    pub path: Vec<ComputerCoordinate>,
    pub text: Option<String>,
    pub key: Option<String>,
    pub button: Option<MouseButtonInput>,
    pub count: Option<u8>,
    pub direction: Option<ScrollDirectionInput>,
    pub amount: Option<i64>,
    pub duration_ms: Option<u64>,
    pub description: Option<String>,
    pub snapshot_generation: Option<u64>,
    pub then_actions: Vec<ComputerActionArgs>,
}

impl ComputerActionArgs {
    pub fn simple(action: ComputerActionName) -> Self {
        Self {
            action,
            x: None,
            y: None,
            x2: None,
            y2: None,
            path: Vec::new(),
            text: None,
            key: None,
            button: None,
            count: None,
            direction: None,
            amount: None,
            duration_ms: None,
            description: None,
            snapshot_generation: None,
            then_actions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SandComputerToolInputError(pub String);

pub fn drag_path(args: &ComputerActionArgs) -> Option<Vec<ComputerCoordinate>> {
    if args.path.len() >= 2 {
        return Some(args.path.clone());
    }
    Some(vec![
        ComputerCoordinate {
            x: args.x?,
            y: args.y?,
        },
        ComputerCoordinate {
            x: args.x2?,
            y: args.y2?,
        },
    ])
}

pub fn validate_computer_action(
    args: &ComputerActionArgs,
    auto_review_mode: Option<SandAutoReviewMode>,
) -> Result<(), SandComputerToolInputError> {
    if args.snapshot_generation == Some(0) {
        return Err(SandComputerToolInputError(
            "Computer snapshotGeneration must be a positive observation generation.".to_string(),
        ));
    }
    if args.then_actions.len() > SAND_COMPUTER_MAX_FOLLOW_UP_ACTIONS {
        return Err(SandComputerToolInputError(format!(
            "Computer then supports at most {SAND_COMPUTER_MAX_FOLLOW_UP_ACTIONS} follow-up actions."
        )));
    }
    validate_single_action(args, auto_review_mode, false)?;
    for follow_up in &args.then_actions {
        if follow_up.action == ComputerActionName::Screenshot {
            return Err(SandComputerToolInputError(
                "Screenshot is not allowed inside then; the tool captures one automatically at the end."
                    .to_string(),
            ));
        }
        if auto_review_mode == Some(SandAutoReviewMode::Enforce)
            && !is_sand_computer_auto_review_bypass_action(follow_up.action.as_str())
        {
            return Err(SandComputerToolInputError(format!(
                "{} is not allowed inside then while Computer Auto-review is enforcing.",
                follow_up.action.as_str()
            )));
        }
        validate_single_action(follow_up, auto_review_mode, true)?;
    }
    Ok(())
}

fn validate_single_action(
    args: &ComputerActionArgs,
    auto_review_mode: Option<SandAutoReviewMode>,
    _follow_up: bool,
) -> Result<(), SandComputerToolInputError> {
    if args.action == ComputerActionName::Drag && drag_path(args).is_none() {
        return Err(SandComputerToolInputError(
            "Drag requires x, y, x2, and y2 or a path with at least 2 points.".to_string(),
        ));
    }
    if args.action == ComputerActionName::Wait
        && args.duration_ms.unwrap_or(1_000) > SAND_COMPUTER_MAX_WAIT_MS
    {
        return Err(SandComputerToolInputError(format!(
            "Wait duration must be at most {SAND_COMPUTER_MAX_WAIT_MS}ms."
        )));
    }
    if args.action == ComputerActionName::Click
        && !matches!(args.count.unwrap_or(1), 1..=3)
    {
        return Err(SandComputerToolInputError(
            "Click count must be between 1 and 3.".to_string(),
        ));
    }
    if args.action == ComputerActionName::DoubleClick && args.count.is_some() {
        return Err(SandComputerToolInputError(
            "Double Click has a fixed click count of 2 and does not accept count.".to_string(),
        ));
    }
    if auto_review_mode == Some(SandAutoReviewMode::Enforce)
        && matches!(
            args.action,
            ComputerActionName::Click | ComputerActionName::DoubleClick | ComputerActionName::Drag
        )
        && args
            .description
            .as_deref()
            .map(str::trim)
            .is_none_or(str::is_empty)
    {
        return Err(SandComputerToolInputError(
            "Click, Double Click, and drag require description: a concise statement of the intended UI target and purpose."
                .to_string(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComputerProtocolAction {
    pub action_case: String,
    pub value: Value,
}

fn coordinate(args: &ComputerActionArgs) -> Option<Value> {
    Some(json!({
        "x": args.x?,
        "y": args.y?,
    }))
}

pub fn to_action(args: &ComputerActionArgs) -> Result<ComputerProtocolAction, SandComputerToolInputError> {
    let (action_case, value) = match args.action {
        ComputerActionName::Screenshot => ("screenshot", json!({})),
        ComputerActionName::Click => (
            "click",
            json!({
                "coordinate": coordinate(args),
                "button": args.button.unwrap_or(MouseButtonInput::Left).generated_name(),
                "count": args.count.unwrap_or(1),
            }),
        ),
        ComputerActionName::DoubleClick => (
            "click",
            json!({
                "coordinate": coordinate(args),
                "button": args.button.unwrap_or(MouseButtonInput::Left).generated_name(),
                "count": 2,
            }),
        ),
        ComputerActionName::Move => (
            "mouseMove",
            json!({"coordinate": coordinate(args)}),
        ),
        ComputerActionName::Drag => {
            let path = drag_path(args).ok_or_else(|| {
                SandComputerToolInputError(
                    "Drag requires x, y, x2, and y2 or a path with at least 2 points.".into(),
                )
            })?;
            (
                "drag",
                json!({
                    "path": path.iter().map(|point| json!({"x":point.x,"y":point.y})).collect::<Vec<_>>(),
                    "button": args.button.unwrap_or(MouseButtonInput::Left).generated_name(),
                }),
            )
        }
        ComputerActionName::Type => ("type", json!({"text":args.text.clone().unwrap_or_default()})),
        ComputerActionName::Key => ("key", json!({"key":args.key.clone().unwrap_or_default()})),
        ComputerActionName::Scroll => (
            "scroll",
            json!({
                "coordinate": coordinate(args),
                "direction": args.direction.unwrap_or(ScrollDirectionInput::Down).generated_name(),
                "amount": args.amount.unwrap_or(3),
            }),
        ),
        ComputerActionName::Wait => (
            "wait",
            json!({"durationMs":args.duration_ms.unwrap_or(1_000)}),
        ),
    };
    Ok(ComputerProtocolAction {
        action_case: action_case.to_string(),
        value,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportedComputerAction {
    Drag { x: i64, y: i64 },
    Move { x: i64, y: i64 },
    Scroll { x: i64, y: i64 },
    Click {
        x: i64,
        y: i64,
        button: String,
        count: u8,
    },
}

pub fn to_reported_action(args: &ComputerActionArgs) -> Option<ReportedComputerAction> {
    match args.action {
        ComputerActionName::Drag => {
            let first = drag_path(args)?.first().copied()?;
            Some(ReportedComputerAction::Drag {
                x: first.x,
                y: first.y,
            })
        }
        ComputerActionName::Move => Some(ReportedComputerAction::Move {
            x: args.x?,
            y: args.y?,
        }),
        ComputerActionName::Scroll => Some(ReportedComputerAction::Scroll {
            x: args.x?,
            y: args.y?,
        }),
        ComputerActionName::Click => Some(ReportedComputerAction::Click {
            x: args.x?,
            y: args.y?,
            button: args.button.unwrap_or(MouseButtonInput::Left).user_name().to_string(),
            count: args.count.unwrap_or(1),
        }),
        ComputerActionName::DoubleClick => Some(ReportedComputerAction::Click {
            x: args.x?,
            y: args.y?,
            button: args.button.unwrap_or(MouseButtonInput::Left).user_name().to_string(),
            count: 2,
        }),
        _ => None,
    }
}

pub fn reported_batch_position(sequence: &[ComputerActionArgs]) -> Option<ReportedComputerAction> {
    let positions = sequence
        .iter()
        .filter_map(to_reported_action)
        .collect::<Vec<_>>();
    positions
        .iter()
        .find(|position| !matches!(position, ReportedComputerAction::Drag { .. }))
        .cloned()
        .or_else(|| positions.first().cloned())
}

pub fn build_computer_action_sequence(
    args: &ComputerActionArgs,
    auto_review_mode: Option<SandAutoReviewMode>,
) -> Result<Vec<ComputerProtocolAction>, SandComputerToolInputError> {
    validate_computer_action(args, auto_review_mode)?;
    let mut sequence = vec![args.clone()];
    sequence.extend(args.then_actions.clone());
    let mut actions = sequence
        .iter()
        .map(to_action)
        .collect::<Result<Vec<_>, _>>()?;
    if sequence
        .last()
        .is_none_or(|action| action.action != ComputerActionName::Screenshot)
    {
        actions.push(to_action(&ComputerActionArgs::simple(
            ComputerActionName::Screenshot,
        ))?);
    }
    Ok(actions)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComputerUseSuccess {
    pub screenshot: Option<String>,
    pub screenshot_path: Option<String>,
    pub cursor_position: Option<ComputerCoordinate>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ComputerUseResult {
    Success(ComputerUseSuccess),
    Error(String),
    Other(String),
}

pub fn describe_outcome(result: &ComputerUseResult, screenshot: bool) -> String {
    if let ComputerUseResult::Error(error) = result {
        return format!(
            "{} failed: {error}",
            if screenshot { "Screenshot" } else { "Computer action" }
        );
    }
    let heading = if screenshot {
        "Screenshot captured from the box desktop."
    } else {
        "Computer action ran on the box desktop."
    };
    let ComputerUseResult::Success(success) = result else {
        return heading.to_string();
    };
    let mut lines = vec![heading.to_string()];
    if let Some(path) = success
        .screenshot_path
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        lines.push(format!("Screenshot saved to {path}."));
    }
    if let Some(position) = success.cursor_position {
        lines.push(format!("Cursor is at ({}, {}).", position.x, position.y));
    }
    lines.join("\n")
}

pub fn persist_computer_screenshot<Persist>(
    result: &mut ComputerUseResult,
    mut persist: Option<Persist>,
) -> Result<(), SandComputerToolInputError>
where
    Persist: FnMut(&[u8], &str) -> Option<String>,
{
    let ComputerUseResult::Success(success) = result else {
        return Ok(());
    };
    let Some(encoded) = success.screenshot.as_deref().filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    let Some(persist) = persist.as_mut() else {
        return Ok(());
    };
    let bytes = STANDARD.decode(encoded).map_err(|error| {
        SandComputerToolInputError(format!("invalid screenshot base64: {error}"))
    })?;
    if let Some(path) = persist(&bytes, "image/webp") {
        success.screenshot_path = Some(path);
    }
    Ok(())
}


pub type ComputerAutoReviewCallback = Arc<
    dyn Fn(&ComputerActionArgs, &str) -> Result<(), ProviderSessionError> + Send + Sync,
>;
pub type ComputerPersistImageCallback =
    Arc<dyn Fn(&[u8], &str) -> Option<String> + Send + Sync>;

pub trait ComputerToolExecutor: Send + Sync {
    fn execute(
        &self,
        args: &ComputerActionArgs,
        tool_call_id: &str,
    ) -> Result<ComputerUseResult, ProviderSessionError>;
}

pub fn to_exact_action_value(args: &ComputerActionArgs) -> Value {
    let mut object = Map::new();
    object.insert("action".into(), Value::String(args.action.as_str().into()));
    if let Some(x) = args.x { object.insert("x".into(), Value::from(x)); }
    if let Some(y) = args.y { object.insert("y".into(), Value::from(y)); }
    if let Some(x2) = args.x2 { object.insert("x2".into(), Value::from(x2)); }
    if let Some(y2) = args.y2 { object.insert("y2".into(), Value::from(y2)); }
    if !args.path.is_empty() {
        object.insert("path".into(), Value::Array(args.path.iter()
            .map(|point| json!({"x":point.x,"y":point.y}))
            .collect()));
    }
    if let Some(text) = args.text.as_ref() { object.insert("text".into(), Value::String(text.clone())); }
    if let Some(key) = args.key.as_ref() { object.insert("key".into(), Value::String(key.clone())); }
    if let Some(button) = args.button { object.insert("button".into(), Value::String(button.user_name().into())); }
    if let Some(count) = args.count { object.insert("count".into(), Value::from(count)); }
    if let Some(direction) = args.direction {
        let direction = match direction {
            ScrollDirectionInput::Up => "up",
            ScrollDirectionInput::Down => "down",
            ScrollDirectionInput::Left => "left",
            ScrollDirectionInput::Right => "right",
        };
        object.insert("direction".into(), Value::String(direction.into()));
    }
    if let Some(amount) = args.amount { object.insert("amount".into(), Value::from(amount)); }
    if let Some(duration_ms) = args.duration_ms { object.insert("durationMs".into(), Value::from(duration_ms)); }
    if let Some(snapshot_generation) = args.snapshot_generation {
        object.insert("snapshotGeneration".into(), Value::from(snapshot_generation));
    }
    Value::Object(object)
}

fn parse_i64(object: &Map<String, Value>, key: &str) -> Result<Option<i64>, ProviderSessionError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_i64().map(Some).ok_or_else(|| {
            ProviderSessionError::Tool(format!("Computer {key} must be an integer"))
        }),
    }
}

fn parse_action_name(value: &str) -> Result<ComputerActionName, ProviderSessionError> {
    match value {
        "screenshot" => Ok(ComputerActionName::Screenshot),
        "click" => Ok(ComputerActionName::Click),
        "doubleClick" => Ok(ComputerActionName::DoubleClick),
        "move" => Ok(ComputerActionName::Move),
        "drag" => Ok(ComputerActionName::Drag),
        "type" => Ok(ComputerActionName::Type),
        "key" => Ok(ComputerActionName::Key),
        "scroll" => Ok(ComputerActionName::Scroll),
        "wait" => Ok(ComputerActionName::Wait),
        other => Err(ProviderSessionError::Tool(format!(
            "Computer action is unsupported: {other}"
        ))),
    }
}

pub fn parse_remote_computer_action(
    value: &Value,
) -> Result<ComputerActionArgs, ProviderSessionError> {
    parse_computer_action(value, true)
}

fn parse_computer_action(
    value: &Value,
    allow_followups: bool,
) -> Result<ComputerActionArgs, ProviderSessionError> {
    let object = value.as_object().ok_or_else(|| {
        ProviderSessionError::Tool("Computer arguments must be an object".into())
    })?;
    let action = object
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| ProviderSessionError::Tool("Computer action is required".into()))
        .and_then(parse_action_name)?;
    let mut args = ComputerActionArgs::simple(action);
    args.snapshot_generation = match object.get("snapshotGeneration") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_u64().filter(|value| *value > 0).ok_or_else(|| {
            ProviderSessionError::Tool(
                "Computer snapshotGeneration must be a positive integer".into(),
            )
        })?),
    };
    if !allow_followups && args.snapshot_generation.is_some() {
        return Err(ProviderSessionError::Tool(
            "Computer snapshotGeneration is only allowed on the primary action".into(),
        ));
    }
    args.x = parse_i64(object, "x")?;
    args.y = parse_i64(object, "y")?;
    args.x2 = parse_i64(object, "x2")?;
    args.y2 = parse_i64(object, "y2")?;
    if let Some(path_value) = object.get("path") {
        let path = path_value.as_array().ok_or_else(|| {
            ProviderSessionError::Tool("Computer path must be an array".into())
        })?;
        args.path = path
            .iter()
            .map(|point| {
                let point = point.as_object().ok_or_else(|| {
                    ProviderSessionError::Tool("Computer drag path point must be an object".into())
                })?;
                Ok(ComputerCoordinate {
                    x: parse_i64(point, "x")?.ok_or_else(|| {
                        ProviderSessionError::Tool("Computer drag path x is required".into())
                    })?,
                    y: parse_i64(point, "y")?.ok_or_else(|| {
                        ProviderSessionError::Tool("Computer drag path y is required".into())
                    })?,
                })
            })
            .collect::<Result<Vec<_>, ProviderSessionError>>()?;
    }
    args.text = object.get("text").and_then(Value::as_str).map(str::to_string);
    args.key = object.get("key").and_then(Value::as_str).map(str::to_string);
    args.description = object.get("description").and_then(Value::as_str).map(str::to_string);
    args.button = match object.get("button").and_then(Value::as_str) {
        None => None,
        Some("left") => Some(MouseButtonInput::Left),
        Some("right") => Some(MouseButtonInput::Right),
        Some("middle") => Some(MouseButtonInput::Middle),
        Some(other) => return Err(ProviderSessionError::Tool(format!("Computer button is unsupported: {other}"))),
    };
    args.count = match parse_i64(object, "count")? {
        None => None,
        Some(value) => Some(u8::try_from(value).map_err(|_| {
            ProviderSessionError::Tool("Computer count is out of range".into())
        })?),
    };
    args.direction = match object.get("direction").and_then(Value::as_str) {
        None => None,
        Some("up") => Some(ScrollDirectionInput::Up),
        Some("down") => Some(ScrollDirectionInput::Down),
        Some("left") => Some(ScrollDirectionInput::Left),
        Some("right") => Some(ScrollDirectionInput::Right),
        Some(other) => return Err(ProviderSessionError::Tool(format!("Computer direction is unsupported: {other}"))),
    };
    args.amount = parse_i64(object, "amount")?;
    args.duration_ms = match parse_i64(object, "durationMs")? {
        None => None,
        Some(value) => Some(u64::try_from(value).map_err(|_| {
            ProviderSessionError::Tool("Computer durationMs must be non-negative".into())
        })?),
    };
    if allow_followups {
        if let Some(followups_value) = object.get("then") {
            let followups = followups_value.as_array().ok_or_else(|| {
                ProviderSessionError::Tool("Computer then must be an array".into())
            })?;
            if followups.is_empty() {
                return Err(ProviderSessionError::Tool(
                    "Computer then must contain at least one follow-up action".into(),
                ));
            }
            args.then_actions = followups
                .iter()
                .map(|followup| parse_computer_action(followup, false))
                .collect::<Result<Vec<_>, _>>()?;
        }
    } else if object.get("then").is_some() {
        return Err(ProviderSessionError::Tool(
            "Computer nested then actions are not supported".into(),
        ));
    }
    Ok(args)
}

pub fn computer_tool_definitions() -> Vec<RoutedToolDefinition> {
    vec![
        RoutedToolDefinition {
            name: "Computer".into(),
            provider_identifier: "fabushi-runner".into(),
            tool_name: "Computer".into(),
            description: Some("Interact with the box desktop using mouse, keyboard, scrolling, waits, and an automatic final screenshot.".into()),
            input_schema: json!({
                "type":"object",
                "additionalProperties":false,
                "required":["action"],
                "properties":{
                    "action":{"type":"string","enum":["screenshot","click","doubleClick","move","drag","type","key","scroll","wait"]},
                    "x":{"type":"integer"},"y":{"type":"integer"},
                    "x2":{"type":"integer"},"y2":{"type":"integer"},
                    "path":{"type":"array","items":{"type":"object","required":["x","y"],"properties":{"x":{"type":"integer"},"y":{"type":"integer"}}}},
                    "text":{"type":"string"},"key":{"type":"string"},
                    "button":{"type":"string","enum":["left","right","middle"]},
                    "count":{"type":"integer","minimum":1,"maximum":3},
                    "direction":{"type":"string","enum":["up","down","left","right"]},
                    "amount":{"type":"integer"},
                    "durationMs":{"type":"integer","minimum":0,"maximum":SAND_COMPUTER_MAX_WAIT_MS},
                    "snapshotGeneration":{"type":"integer","minimum":1},
                    "description":{"type":"string"},
                    "then":{"type":"array","minItems":1,"maxItems":SAND_COMPUTER_MAX_FOLLOW_UP_ACTIONS,"items":{"type":"object"}}
                }
            }),
        },
        RoutedToolDefinition {
            name: "Screenshot".into(),
            provider_identifier: "fabushi-runner".into(),
            tool_name: "Screenshot".into(),
            description: Some("Capture the current box desktop screenshot.".into()),
            input_schema: json!({"type":"object","additionalProperties":false,"properties":{}}),
        },
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ComputerToolExposure {
    #[default]
    Full,
    ScreenshotOnly,
    Disabled,
}

pub struct SandComputerToolBridge {
    delegate: Arc<dyn RoutedToolBridge>,
    executor: Arc<dyn ComputerToolExecutor>,
    exposure: ComputerToolExposure,
}

impl SandComputerToolBridge {
    pub fn new(
        delegate: Arc<dyn RoutedToolBridge>,
        executor: Arc<dyn ComputerToolExecutor>,
    ) -> Self {
        Self {
            delegate,
            executor,
            exposure: ComputerToolExposure::Full,
        }
    }

    pub fn with_exposure(mut self, exposure: ComputerToolExposure) -> Self {
        self.exposure = exposure;
        self
    }
}

impl RoutedToolBridge for SandComputerToolBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        if self.exposure == ComputerToolExposure::Disabled {
            return self.delegate.list_tools();
        }
        let definitions = computer_tool_definitions()
            .into_iter()
            .filter(|tool| {
                self.exposure == ComputerToolExposure::Full || tool.name == "Screenshot"
            })
            .collect::<Vec<_>>();
        let names = ["Computer", "Screenshot"];
        let mut delegated = self.delegate.list_tools()?;
        delegated.retain(|tool| {
            !names.contains(&tool.name.as_str()) && !names.contains(&tool.tool_name.as_str())
        });
        let mut tools = definitions;
        tools.extend(delegated);
        Ok(tools)
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: Value,
        tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        let effective = if matches!(tool.name.as_str(), "Computer" | "Screenshot") {
            tool.name.as_str()
        } else {
            tool.tool_name.as_str()
        };
        if self.exposure == ComputerToolExposure::Disabled
            && matches!(effective, "Computer" | "Screenshot")
        {
            return Err(ProviderSessionError::Tool(
                "Computer tools are not available for this Runner role.".into(),
            ));
        }
        if self.exposure == ComputerToolExposure::ScreenshotOnly && effective == "Computer" {
            return Err(ProviderSessionError::Tool(
                "Interactive Computer control is reserved for the computerUse subagent; this Runner only has Screenshot.".into(),
            ));
        }
        let (action, screenshot) = match effective {
            "Screenshot" => (ComputerActionArgs::simple(ComputerActionName::Screenshot), true),
            "Computer" => (parse_computer_action(&args, true)?, false),
            _ => return self.delegate.call_tool(tool, args, tool_call_id),
        };
        let result = self.executor.execute(&action, tool_call_id)?;
        let screenshot_path = match &result {
            ComputerUseResult::Success(success) => success.screenshot_path.clone(),
            _ => None,
        };
        Ok(json!({
            "content": describe_outcome(&result, screenshot),
            "screenshotPath": screenshot_path,
        }))
    }
}
