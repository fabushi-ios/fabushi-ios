use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value, json};

use crate::cloud_agents::cloud_agent_images::{
    CloudAgentImage, CloudAgentImagesError, load_cloud_agent_images,
};
use crate::cloud_agents::cloud_agent_transcript_dump::cloud_agent_transcript_dump_path;
use crate::extensions::cloud_agents::cloud_agent_request_composition::{
    CloudAgentEnvironment, CloudAgentImageInput,
};
use crate::extensions::cloud_agents::cloud_agents_service::{
    CloudAgentDetail, CloudAgentSummary, SandCloudAgentManager,
};
use crate::extensions::cloud_agents::model_catalog_fetch::{
    SandModelCatalogEntry, SandModelCatalogParameterValue,
};
use crate::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use crate::runner::box_tool_access::{
    RunnerBoxReadRequest, RunnerBoxResourcePort, RunnerBoxWriteRequest,
};
use crate::runner::routed_provider_runtime::{
    RoutedProviderCancellation, RoutedToolBridge,
};

pub const CLOUD_AGENT_TOOL_NAME: &str = "CloudAgent";
pub const CLOUD_AGENT_TOOL_IDENTIFIER: &str = "CLOUD_AGENT";
pub const CANCELLED_BEFORE_CLOUD_AGENT_CALL: &str =
    "The cloud agent action was cancelled.";

const DESTRUCTIVE_ACTIONS: &[&str] = &["cancel", "archive", "unarchive", "delete"];
const REVIEW_GATED_ACTIONS: &[&str] = &[
    "launch",
    "reply",
    "rename",
    "cancel",
    "archive",
    "unarchive",
    "delete",
];

pub type CloudAgentReviewHook = Arc<
    dyn Fn(&Value, &[CloudAgentImage], &str) -> Result<Option<String>, ProviderSessionError>
        + Send
        + Sync,
>;
pub type CloudAgentWatchHook = Arc<dyn Fn(&str, bool, Option<Value>) + Send + Sync>;

#[derive(Clone)]
pub struct CloudAgentToolDependencies {
    pub manager: Arc<SandCloudAgentManager>,
    pub agent_dir: PathBuf,
    pub box_resources: Arc<dyn RunnerBoxResourcePort>,
    pub cancellation: RoutedProviderCancellation,
    pub review: Option<CloudAgentReviewHook>,
    pub quiet_origin: Option<Value>,
    pub watch: Option<CloudAgentWatchHook>,
}

#[derive(Clone)]
pub struct CloudAgentToolBridge {
    upstream: Arc<dyn RoutedToolBridge>,
    deps: CloudAgentToolDependencies,
}

impl CloudAgentToolBridge {
    pub fn new(
        upstream: Arc<dyn RoutedToolBridge>,
        deps: CloudAgentToolDependencies,
    ) -> Self {
        Self { upstream, deps }
    }

    fn is_cloud_agent_tool(tool: &RoutedToolDefinition) -> bool {
        tool.name == CLOUD_AGENT_TOOL_NAME
            || tool.tool_name == CLOUD_AGENT_TOOL_NAME
            || tool.tool_name == CLOUD_AGENT_TOOL_IDENTIFIER
    }
}

pub fn cloud_agent_tool_definition() -> RoutedToolDefinition {
    RoutedToolDefinition {
        name: CLOUD_AGENT_TOOL_NAME.into(),
        provider_identifier: "fabushi-runner".into(),
        tool_name: CLOUD_AGENT_TOOL_NAME.into(),
        description: Some(
            "Manage Cursor cloud agents: launch, list, inspect, watch, follow up, and manage their lifecycle."
                .into(),
        ),
        input_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "launch", "list", "models", "get", "dump", "watch",
                        "reply", "rename", "cancel", "archive", "unarchive",
                        "delete", "list_artifacts"
                    ]
                },
                "prompt": {"type":"string"},
                "images": {
                    "type":"array",
                    "items":{
                        "type":"object",
                        "additionalProperties":false,
                        "required":["url"],
                        "properties":{"url":{"type":"string","minLength":1}}
                    }
                },
                "repo_url":{"type":"string"},
                "starting_ref":{"type":"string"},
                "model":{"type":"string"},
                "model_params":{
                    "type":"object",
                    "additionalProperties":{"type":"string"}
                },
                "title":{"type":"string"},
                "environment":{
                    "type":"object",
                    "additionalProperties":false,
                    "required":["type"],
                    "properties":{
                        "type":{"type":"string","enum":["cloud","pool","machine","environment"]},
                        "name":{"type":"string"},
                        "id":{"type":"string"},
                        "team_id":{"type":"integer"}
                    }
                },
                "interrupt":{"type":"boolean"},
                "agent_id":{"type":"string"},
                "scope":{"type":"string","enum":["launched","all"]},
                "include_archived":{"type":"boolean"},
                "limit":{"type":"integer","minimum":1},
                "confirm":{"type":"boolean"}
            }
        }),
    }
}

impl RoutedToolBridge for CloudAgentToolBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        let mut tools = self.upstream.list_tools()?;
        tools.retain(|tool| !Self::is_cloud_agent_tool(tool));
        tools.insert(0, cloud_agent_tool_definition());
        Ok(tools)
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: Value,
        tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        if !Self::is_cloud_agent_tool(tool) {
            return self.upstream.call_tool(tool, args, tool_call_id);
        }
        run_cloud_agent_action(&self.deps, args, tool_call_id).map(Value::String)
    }
}

fn run_cloud_agent_action(
    deps: &CloudAgentToolDependencies,
    args: Value,
    tool_call_id: &str,
) -> Result<String, ProviderSessionError> {
    let object = args.as_object().ok_or_else(|| {
        ProviderSessionError::Tool("CloudAgent arguments must be an object".into())
    })?;
    let action = required_trimmed(object, "action", "CloudAgent")?;

    if DESTRUCTIVE_ACTIONS.contains(&action)
        && object.get("confirm").and_then(Value::as_bool) != Some(true)
    {
        return Ok(format!(
            "'{action}' is destructive. Confirm with the user first (e.g. a SendMessage widget), then call CloudAgent again with confirm: true."
        ));
    }

    let attached_images = if matches!(action, "launch" | "reply") {
        load_images(deps, object.get("images"), tool_call_id).map_err(|message| {
            ProviderSessionError::Tool(format!(
                "{}: {message}",
                if action == "launch" {
                    "Could not launch the cloud agent"
                } else {
                    "Could not send the follow-up"
                }
            ))
        })?
    } else {
        Vec::new()
    };

    if REVIEW_GATED_ACTIONS.contains(&action) {
        if let Some(review) = deps.review.as_ref() {
            if let Some(reason) = review(&args, &attached_images, tool_call_id)? {
                return Ok(reason);
            }
        }
    }

    match action {
        "launch" => launch_action(deps, object, &attached_images),
        "list" => list_action(deps, object),
        "models" => models_action(deps),
        "get" => get_action(deps, object),
        "dump" => dump_action(deps, object, tool_call_id),
        "watch" => watch_action(deps, object),
        "reply" => reply_action(deps, object, &attached_images),
        "rename" => rename_action(deps, object),
        "cancel" => cancel_action(deps, object),
        "archive" => archive_action(deps, object, true),
        "unarchive" => archive_action(deps, object, false),
        "delete" => delete_action(deps, object),
        "list_artifacts" => list_artifacts_action(deps, object),
        other => Err(ProviderSessionError::Tool(format!(
            "Unknown CloudAgent action '{other}'."
        ))),
    }
}

fn load_images(
    deps: &CloudAgentToolDependencies,
    value: Option<&Value>,
    tool_call_id: &str,
) -> Result<Vec<CloudAgentImage>, String> {
    let urls = parse_image_urls(value)?;
    if urls.is_empty() {
        return Ok(Vec::new());
    }
    let box_resources = Arc::clone(&deps.box_resources);
    let call_id = tool_call_id.to_string();
    let mut read_box_file = move |path: &str| {
        read_box_binary(
            box_resources.as_ref(),
            path,
            &format!("{call_id}:cloud-agent-image"),
        )
    };
    load_cloud_agent_images(
        &urls,
        &deps.agent_dir,
        Some(&mut read_box_file),
    )
    .map_err(cloud_agent_image_error_message)
}

fn parse_image_urls(value: Option<&Value>) -> Result<Vec<String>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let rows = value
        .as_array()
        .ok_or_else(|| "images must be an array".to_string())?;
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            row.as_object()
                .and_then(|object| object.get("url"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|url| !url.is_empty())
                .map(str::to_string)
                .ok_or_else(|| format!("images[{index}].url is required"))
        })
        .collect()
}

fn cloud_agent_image_error_message(error: CloudAgentImagesError) -> String {
    match error {
        CloudAgentImagesError::NotFileUrl(message)
        | CloudAgentImagesError::NotImage(message)
        | CloudAgentImagesError::Unreadable(message)
        | CloudAgentImagesError::Refused(message)
        | CloudAgentImagesError::TooLarge(message) => message,
    }
}

fn read_box_binary(
    box_resources: &dyn RunnerBoxResourcePort,
    path: &str,
    tool_call_id: &str,
) -> Result<Vec<u8>, String> {
    let result = box_resources
        .execute_read(RunnerBoxReadRequest {
            path: path.to_string(),
            tool_call_id: tool_call_id.to_string(),
            offset: None,
            limit: None,
            encoding_hint: None,
        })
        .map_err(|error| error.to_string())?;
    if result.get("kind").and_then(Value::as_str) != Some("success") {
        return Err(format!("Box Read failed for '{path}': {result}"));
    }
    let output = result.get("output").ok_or_else(|| {
        format!("Box Read returned no output for '{path}'.")
    })?;
    if output.get("kind").and_then(Value::as_str) != Some("data") {
        return Err(format!(
            "Box Read did not return binary data for '{path}'."
        ));
    }
    let bytes = output
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("Box Read returned malformed binary data for '{path}'."))?;
    bytes
        .iter()
        .map(|value| {
            value
                .as_u64()
                .filter(|byte| *byte <= u8::MAX as u64)
                .map(|byte| byte as u8)
                .ok_or_else(|| format!("Box Read returned invalid binary data for '{path}'."))
        })
        .collect()
}

fn launch_action(
    deps: &CloudAgentToolDependencies,
    object: &Map<String, Value>,
    images: &[CloudAgentImage],
) -> Result<String, ProviderSessionError> {
    let prompt = required_trimmed(object, "prompt", "launch")?;
    let environment = parse_environment(object.get("environment"))?;
    if matches!(
        environment,
        Some(CloudAgentEnvironment::Environment {
            public_id: None,
            name: None
        })
    ) {
        return Ok(
            "A saved-environment launch needs environment.name (the environment's display name) or environment.id (its public id)."
                .into(),
        );
    }

    let repo_url = if matches!(environment, Some(CloudAgentEnvironment::Environment { .. })) {
        optional_trimmed(object.get("repo_url")).map(str::to_string)
    } else {
        Some(required_trimmed(object, "repo_url", "launch")?.to_string())
    };
    let model = optional_trimmed(object.get("model")).map(str::to_string);
    let model_params = parse_model_params(object.get("model_params"))?;
    if let Some(message) =
        validate_model_selection(deps.manager.as_ref(), model.as_deref(), &model_params)
    {
        return Ok(message);
    }
    if deps.cancellation.is_cancelled() {
        return Ok(CANCELLED_BEFORE_CLOUD_AGENT_CALL.into());
    }

    let result = deps
        .manager
        .launch(crate::extensions::cloud_agents::cloud_agents_service::CloudAgentLaunchArgs {
            prompt: prompt.to_string(),
            repo_url,
            starting_ref: optional_trimmed(object.get("starting_ref")).map(str::to_string),
            environment: environment.clone(),
            model_id: model.clone(),
            model_params,
            images: images.iter().map(to_image_input).collect(),
            title: optional_trimmed(object.get("title")).map(str::to_string),
        })
        .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
    deps.manager.remember_managed_id(&result.bc_id);
    if let Some(watch) = deps.watch.as_ref() {
        watch(&result.bc_id, false, deps.quiet_origin.clone());
    }
    let followup = if deps.watch.is_some() {
        r#"You're revived automatically when it finishes, so keep working and don't poll it — use "reply" to send a follow-up, or "get" if you need its status sooner."#
    } else {
        r#"Use action "get" with this agent_id to check status, or "reply" to send a follow-up."#
    };
    let pr = if matches!(environment, Some(CloudAgentEnvironment::Machine { .. })) {
        " It opens a PR when done only if the worker can push to the repo."
    } else {
        " It will open a PR when done."
    };
    Ok(format!(
        "Launched cloud agent {}.\n{}\nIt runs on {}.{} {}",
        result.bc_id,
        result.url,
        cloud_agent_runtime_description(environment.as_ref()),
        pr,
        followup,
    ))
}

fn list_action(
    deps: &CloudAgentToolDependencies,
    object: &Map<String, Value>,
) -> Result<String, ProviderSessionError> {
    let scope = optional_trimmed(object.get("scope")).unwrap_or("launched");
    let limit = object
        .get("limit")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok());
    let include_archived = object
        .get("include_archived")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let all = deps
        .manager
        .list(limit, include_archived)
        .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
    let filtered = if scope == "all" {
        all
    } else {
        all.into_iter()
            .filter(|summary| deps.manager.is_managed_id(&summary.bc_id))
            .collect()
    };
    if filtered.is_empty() {
        return Ok(if scope == "all" {
            "No cloud agents found.".into()
        } else {
            r#"No cloud agents launched via this tool yet. Use scope: "all" to list every cloud agent on the account."#.into()
        });
    }
    let mut lines = vec![if scope == "all" {
        format!("Cloud agents on the account ({}):", filtered.len())
    } else {
        format!("Cloud agents launched via this tool ({}):", filtered.len())
    }];
    lines.extend(filtered.iter().map(summary_line));
    Ok(lines.join("\n"))
}

fn models_action(
    deps: &CloudAgentToolDependencies,
) -> Result<String, ProviderSessionError> {
    let catalog = deps
        .manager
        .list_models()
        .map_err(ProviderSessionError::Tool)?;
    if catalog.is_empty() {
        return Ok("No models available.".into());
    }
    let mut lines = vec![format!(
        "Available cloud-agent models ({}). Pass 'model' (id) and optional 'model_params' (param id → value) to launch/reply:",
        catalog.len()
    )];
    lines.extend(model_catalog_lines(&catalog));
    Ok(lines.join("\n"))
}

fn get_action(
    deps: &CloudAgentToolDependencies,
    object: &Map<String, Value>,
) -> Result<String, ProviderSessionError> {
    let id = required_trimmed(object, "agent_id", "get")?;
    let Some(detail) = deps
        .manager
        .get(id)
        .map_err(|error| ProviderSessionError::Tool(error.to_string()))?
    else {
        return Ok(format!("No cloud agent found for {id}."));
    };
    Ok(format_detail(&detail))
}

fn dump_action(
    deps: &CloudAgentToolDependencies,
    object: &Map<String, Value>,
    tool_call_id: &str,
) -> Result<String, ProviderSessionError> {
    let id = required_trimmed(object, "agent_id", "dump")?;
    if !deps.manager.is_managed_id(id) {
        return Ok(format!(
            r#"dump is limited to cloud agents you launched, watched, or replied to this session (not {id}). Use action "watch" with {id} first to manage it, or read its changes from its branch via `gh pr diff` / `gh api`."#
        ));
    }
    let Some(dump) = deps
        .manager
        .get_transcript_dump(id)
        .map_err(|error| ProviderSessionError::Tool(error.to_string()))?
    else {
        return Ok(format!("No cloud agent found for {id}."));
    };
    let progress = match dump.status.as_str() {
        "running" | "creating" => {
            "The run is still in progress, so this is the transcript so far."
        }
        "finished" => "The run has finished, so this is the complete transcript.",
        _ => "The run has ended, so this is the transcript as left.",
    };
    if dump.line_count == 0 {
        return Ok(format!(
            "Cloud agent {id} hasn't produced any transcript output yet. {progress}"
        ));
    }
    let path = cloud_agent_transcript_dump_path(id);
    deps.box_resources
        .execute_write(RunnerBoxWriteRequest {
            path: path.clone(),
            data: dump.jsonl.as_bytes().to_vec(),
            tool_call_id: format!("{tool_call_id}:cloud-agent-dump"),
        })?;
    Ok(format!(
        "Wrote {id}'s full transcript to {path} ({} bytes, {} message lines).\n{progress}\nFormat: JSONL, one JSON message per line — role, text, reasoning/thinking, tool calls with args, and tool results. Use Shell to grep it or Read to read it.\nFor just the final report, read the last line: `tail -n 1 {path}` (the final assistant message).",
        dump.jsonl.len(),
        dump.line_count,
    ))
}

fn watch_action(
    deps: &CloudAgentToolDependencies,
    object: &Map<String, Value>,
) -> Result<String, ProviderSessionError> {
    let id = required_trimmed(object, "agent_id", "watch")?;
    let Some(watch) = deps.watch.as_ref() else {
        return Ok(format!(
            r#"Watching isn't available here. Use action "get" with {id} to check its status."#
        ));
    };
    watch(id, false, deps.quiet_origin.clone());
    deps.manager.remember_managed_id(id);
    Ok(format!(
        "Watching {id}. You'll be revived automatically when it finishes — keep working and don't poll it."
    ))
}

fn reply_action(
    deps: &CloudAgentToolDependencies,
    object: &Map<String, Value>,
    images: &[CloudAgentImage],
) -> Result<String, ProviderSessionError> {
    let id = required_trimmed(object, "agent_id", "reply")?;
    let prompt = required_trimmed(object, "prompt", "reply")?;
    let model = optional_trimmed(object.get("model")).map(str::to_string);
    let model_params = parse_model_params(object.get("model_params"))?;
    if let Some(message) =
        validate_model_selection(deps.manager.as_ref(), model.as_deref(), &model_params)
    {
        return Ok(message);
    }
    let interrupt = object
        .get("interrupt")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let was_running = if interrupt {
        deps.manager
            .get(id)
            .ok()
            .flatten()
            .is_some_and(|detail| {
                matches!(detail.summary.status.as_str(), "running" | "creating")
            })
    } else {
        false
    };
    if deps.cancellation.is_cancelled() {
        return Ok(CANCELLED_BEFORE_CLOUD_AGENT_CALL.into());
    }
    let run_id = deps
        .manager
        .reply(crate::extensions::cloud_agents::cloud_agents_service::CloudAgentReplyArgs {
            bc_id: id.to_string(),
            prompt: prompt.to_string(),
            images: images.iter().map(to_image_input).collect(),
            interrupt,
            model_id: model,
            model_params,
        })
        .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
    deps.manager.remember_managed_id(id);
    if let Some(watch) = deps.watch.as_ref() {
        watch(id, true, deps.quiet_origin.clone());
    }
    let sent = if !interrupt {
        format!("Sent follow-up to {id} (run {run_id})")
    } else if was_running {
        format!(
            "Interrupted {id}'s active run and delivered the follow-up immediately (run {run_id}); it starts processing now"
        )
    } else {
        format!(
            "Sent follow-up to {id} (run {run_id}); it wasn't running, so there was nothing to interrupt and it starts a fresh run"
        )
    };
    Ok(format!(
        "{sent}. {}",
        if deps.watch.is_some() {
            "You're revived automatically when this follow-up finishes, so keep working and don't poll it."
        } else {
            r#"Use "get" to check status."#
        }
    ))
}

fn rename_action(
    deps: &CloudAgentToolDependencies,
    object: &Map<String, Value>,
) -> Result<String, ProviderSessionError> {
    let id = required_trimmed(object, "agent_id", "rename")?;
    let title = required_trimmed(object, "title", "rename")?;
    deps.manager
        .rename(id, title)
        .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
    Ok(format!(
        r#"Renamed {id} to "{title}". The new title shows everywhere the agent appears (cursor.com, the IDE sidebar, mobile)."#
    ))
}

fn cancel_action(
    deps: &CloudAgentToolDependencies,
    object: &Map<String, Value>,
) -> Result<String, ProviderSessionError> {
    let id = required_trimmed(object, "agent_id", "cancel")?;
    deps.manager
        .cancel(id)
        .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
    Ok(format!("Requested cancellation of the active run for {id}."))
}

fn archive_action(
    deps: &CloudAgentToolDependencies,
    object: &Map<String, Value>,
    archived: bool,
) -> Result<String, ProviderSessionError> {
    let action = if archived { "archive" } else { "unarchive" };
    let id = required_trimmed(object, "agent_id", action)?;
    deps.manager
        .set_archived(id, archived)
        .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
    Ok(format!(
        "{} {id}.",
        if archived { "Archived" } else { "Unarchived" }
    ))
}

fn delete_action(
    deps: &CloudAgentToolDependencies,
    object: &Map<String, Value>,
) -> Result<String, ProviderSessionError> {
    let id = required_trimmed(object, "agent_id", "delete")?;
    deps.manager
        .delete(id)
        .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
    deps.manager.forget_managed_id(id);
    Ok(format!("Permanently deleted {id}."))
}

fn list_artifacts_action(
    deps: &CloudAgentToolDependencies,
    object: &Map<String, Value>,
) -> Result<String, ProviderSessionError> {
    let id = required_trimmed(object, "agent_id", "list_artifacts")?;
    let artifacts = deps
        .manager
        .list_artifacts(id)
        .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
    if artifacts.is_empty() {
        return Ok(format!("No artifacts for {id}."));
    }
    let mut lines = vec![format!("Artifacts for {id} ({}):", artifacts.len())];
    lines.extend(
        artifacts
            .iter()
            .map(|artifact| format!("- {} ({} bytes)", artifact.path, artifact.size_bytes)),
    );
    Ok(lines.join("\n"))
}

fn parse_environment(
    value: Option<&Value>,
) -> Result<Option<CloudAgentEnvironment>, ProviderSessionError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let object = value.as_object().ok_or_else(|| {
        ProviderSessionError::Tool("environment must be an object".into())
    })?;
    let environment_type = required_trimmed(object, "type", "environment")?;
    let team_id = object.get("team_id").and_then(Value::as_i64);
    Ok(Some(match environment_type {
        "cloud" => CloudAgentEnvironment::Cloud,
        "pool" => CloudAgentEnvironment::Pool {
            name: optional_trimmed(object.get("name")).map(str::to_string),
            team_id,
        },
        "machine" => CloudAgentEnvironment::Machine {
            name: required_trimmed(object, "name", "machine environment")?.to_string(),
            team_id,
        },
        "environment" => CloudAgentEnvironment::Environment {
            public_id: optional_trimmed(object.get("id")).map(str::to_string),
            name: optional_trimmed(object.get("name")).map(str::to_string),
        },
        other => {
            return Err(ProviderSessionError::Tool(format!(
                "Unknown cloud-agent environment type '{other}'."
            )))
        }
    }))
}

fn parse_model_params(
    value: Option<&Value>,
) -> Result<BTreeMap<String, String>, ProviderSessionError> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let object = value.as_object().ok_or_else(|| {
        ProviderSessionError::Tool("model_params must be an object".into())
    })?;
    object
        .iter()
        .map(|(key, value)| {
            value
                .as_str()
                .map(|value| (key.clone(), value.to_string()))
                .ok_or_else(|| {
                    ProviderSessionError::Tool(format!(
                        "model_params.{key} must be a string"
                    ))
                })
        })
        .collect()
}

fn validate_model_selection(
    manager: &SandCloudAgentManager,
    model_id: Option<&str>,
    params: &BTreeMap<String, String>,
) -> Option<String> {
    let id = model_id.unwrap_or_default().trim();
    if id.is_empty() {
        return (!params.is_empty())
            .then(|| "'model' is required when 'model_params' are provided.".to_string());
    }
    let Ok(catalog) = manager.list_models() else {
        return None;
    };
    let Some(entry) = catalog.iter().find(|entry| {
        entry.id.eq_ignore_ascii_case(id)
            || entry.aliases.iter().any(|alias| alias.eq_ignore_ascii_case(id))
    }) else {
        return Some(format!(
            "Unknown model '{}'. Use the 'models' action to list valid models. Available: {}.",
            model_id.unwrap_or_default(),
            catalog
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    };

    let mut errors = Vec::new();
    for (param_id, value) in params {
        let Some(schema) = entry.params.iter().find(|candidate| candidate.id == *param_id) else {
            errors.push(format!("Unknown parameter '{param_id}'."));
            continue;
        };
        if !schema.values.iter().any(|candidate| candidate.value == *value) {
            errors.push(format!(
                "Invalid value '{value}' for '{param_id}' (allowed: {}).",
                schema
                    .values
                    .iter()
                    .map(|candidate| candidate.value.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    if errors.is_empty() {
        None
    } else {
        Some(format!(
            "Invalid model_params for {}: {}",
            entry.id,
            errors.join(" ")
        ))
    }
}

fn model_catalog_lines(catalog: &[SandModelCatalogEntry]) -> Vec<String> {
    let mut lines = Vec::new();
    for entry in catalog {
        let display_name = entry
            .display_name
            .as_deref()
            .map(|name| format!(" ({name})"))
            .unwrap_or_default();
        let aliases = if entry.aliases.is_empty() {
            String::new()
        } else {
            format!(" [aliases: {}]", entry.aliases.join(", "))
        };
        lines.push(format!("- {}{display_name}{aliases}", entry.id));
        for param in &entry.params {
            lines.push(format!(
                "    {}: {}",
                param.id,
                param
                    .values
                    .iter()
                    .map(|value| value.value.as_str())
                    .collect::<Vec<_>>()
                    .join(" | ")
            ));
        }
        for restriction in describe_param_incompatibilities(entry) {
            lines.push(format!("    (restriction) {restriction}"));
        }
    }
    lines
}

fn describe_param_incompatibilities(entry: &SandModelCatalogEntry) -> Vec<String> {
    if entry.variants.is_empty() {
        return Vec::new();
    }
    let mut present = Vec::<SandModelCatalogParameterValue>::new();
    for variant in &entry.variants {
        for value in variant {
            if !present.iter().any(|candidate| candidate == value) {
                present.push(value.clone());
            }
        }
    }
    let mut notes = Vec::new();
    for (index, a) in present.iter().enumerate() {
        for b in present.iter().skip(index + 1) {
            if a.id == b.id {
                continue;
            }
            let co_occurs = entry.variants.iter().any(|variant| {
                let has_a = variant
                    .iter()
                    .any(|value| value.id == a.id && value.value == a.value);
                let has_b = variant
                    .iter()
                    .any(|value| value.id == b.id && value.value == b.value);
                has_a && has_b
            });
            if !co_occurs {
                notes.push(format!(
                    "{}={} cannot be combined with {}={}",
                    a.id, a.value, b.id, b.value
                ));
            }
        }
    }
    notes
}

fn to_image_input(image: &CloudAgentImage) -> CloudAgentImageInput {
    CloudAgentImageInput {
        data: image.data.clone(),
        path: Some(image.path.clone()),
        mime_type: Some(image.mime_type.clone()),
    }
}

fn summary_line(summary: &CloudAgentSummary) -> String {
    let mut bits = vec![summary.status.clone()];
    if summary.is_archived {
        bits.push("archived".into());
    }
    if !summary.branch_name.is_empty() {
        bits.push(summary.branch_name.clone());
    }
    if !summary.pr_url.is_empty() {
        bits.push(summary.pr_url.clone());
    }
    format!(
        "- {} — {} [{}] {}",
        summary.bc_id,
        if summary.name.trim().is_empty() {
            "(unnamed)"
        } else {
            summary.name.trim()
        },
        bits.join(", "),
        summary.url,
    )
}

fn format_detail(detail: &CloudAgentDetail) -> String {
    let summary = &detail.summary;
    let mut lines = vec![
        format!(
            "Cloud agent {} — {}",
            summary.bc_id,
            if summary.name.trim().is_empty() {
                "(unnamed)"
            } else {
                summary.name.trim()
            }
        ),
        format!(
            "Status: {}{}",
            summary.status,
            if summary.is_archived { " (archived)" } else { "" }
        ),
        format!("URL: {}", summary.url),
    ];
    if !summary.branch_name.is_empty() {
        lines.push(format!("Branch: {}", summary.branch_name));
    }
    if !summary.pr_url.is_empty() {
        lines.push(format!("Pull request: {}", summary.pr_url));
    }
    if detail.files_changed > 0 {
        lines.push(format!(
            "Changes: +{}/-{} across {} file(s).",
            detail.lines_added, detail.lines_removed, detail.files_changed
        ));
    }
    if let Some(error) = detail.error.as_deref().filter(|value| !value.is_empty()) {
        lines.push(error.to_string());
    }
    lines.join("\n")
}

fn cloud_agent_runtime_description(
    environment: Option<&CloudAgentEnvironment>,
) -> String {
    match environment {
        None | Some(CloudAgentEnvironment::Cloud) => "a Cursor VM".into(),
        Some(CloudAgentEnvironment::Pool { name, .. }) => match name.as_deref() {
            Some(name) if !name.trim().is_empty() => {
                format!("the '{}' self-hosted pool", name.trim())
            }
            _ => "an eligible self-hosted pool".into(),
        },
        Some(CloudAgentEnvironment::Machine { name, .. }) => {
            format!("the '{}' private worker", name.trim())
        }
        Some(CloudAgentEnvironment::Environment { public_id, name }) => {
            let label = name
                .as_deref()
                .or(public_id.as_deref())
                .unwrap_or_default();
            format!("a Cursor VM in the '{label}' saved environment")
        }
    }
}

fn required_trimmed<'a>(
    object: &'a Map<String, Value>,
    field: &str,
    action: &str,
) -> Result<&'a str, ProviderSessionError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ProviderSessionError::Tool(format!(
                "'{field}' is required for the '{action}' action."
            ))
        })
}

fn optional_trimmed(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}
