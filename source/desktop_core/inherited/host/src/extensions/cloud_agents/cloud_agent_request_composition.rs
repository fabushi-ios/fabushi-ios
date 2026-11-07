use std::collections::BTreeMap;

use serde_json::{Map, Value, json};
use url::Url;
use uuid::Uuid;

use super::cloud_agent_launch_error::SandCloudAgentLaunchError;

pub const PRIVATE_WORKER_SHARED_ASSIGNMENT_ALLOWED_LABEL_KEY: &str =
    "cursor.private_worker.shared_assignment_allowed";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedEnvironmentRepo {
    pub repo_url: String,
    pub scm_repo_node_id: Option<String>,
    pub git_enterprise_uuid: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedEnvironmentRepoConfig {
    pub repos: Vec<SavedEnvironmentRepo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedEnvironment {
    pub public_id: String,
    pub name: String,
    pub repo_config: Option<SavedEnvironmentRepoConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudAgentEnvironment {
    Cloud,
    Environment {
        public_id: Option<String>,
        name: Option<String>,
    },
    Pool {
        name: Option<String>,
        team_id: Option<i64>,
    },
    Machine {
        name: String,
        team_id: Option<i64>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudAgentImageInput {
    pub data: Vec<u8>,
    pub path: Option<String>,
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudAgentRepo {
    pub sanitized_repo_url: String,
    pub http_repo_url: String,
    pub base_branch: Option<String>,
}

pub fn build_cloud_agent_requested_model(
    model_id: Option<&str>,
    params: Option<&BTreeMap<String, String>>,
) -> Result<Option<Value>, SandCloudAgentLaunchError> {
    let parameters = params
        .into_iter()
        .flat_map(|values| values.iter())
        .map(|(id, value)| json!({ "id": id, "value": value }))
        .collect::<Vec<_>>();
    let trimmed = model_id.unwrap_or_default().trim();
    if trimmed.is_empty() {
        if !parameters.is_empty() {
            return Err(SandCloudAgentLaunchError::new(
                "'model' is required when 'model_params' are provided.",
            ));
        }
        return Ok(None);
    }
    Ok(Some(json!({
        "modelId": trimmed,
        "maxMode": true,
        "parameters": parameters,
    })))
}

pub fn build_cloud_agent_user_message(
    prompt: &str,
    mode: Option<Value>,
    images: &[CloudAgentImageInput],
) -> Value {
    let mut message = Map::new();
    message.insert("text".into(), Value::String(prompt.to_string()));
    message.insert("messageId".into(), Value::String(Uuid::new_v4().to_string()));
    if let Some(mode) = mode {
        message.insert("mode".into(), mode);
    }
    if !images.is_empty() {
        let selected_images = images
            .iter()
            .map(|image| {
                json!({
                    "dataOrBlobId": {
                        "case": "data",
                        "value": image.data,
                    },
                    "path": image.path.as_deref().unwrap_or_default(),
                    "mimeType": image.mime_type.as_deref().unwrap_or_default(),
                })
            })
            .collect::<Vec<_>>();
        message.insert(
            "selectedContext".into(),
            json!({ "selectedImages": selected_images }),
        );
    }
    Value::Object(message)
}

pub fn build_cloud_agent_conversation_action(user_message: Value) -> Value {
    json!({
        "action": {
            "case": "userMessageAction",
            "value": {
                "userMessage": user_message,
                "sendToInteractionListener": true,
            }
        }
    })
}

fn trim_remote_path(pathname: &str) -> String {
    let mut normalized = pathname.trim_end_matches('/').to_string();
    if normalized.to_ascii_lowercase().ends_with(".git") {
        let new_len = normalized.len().saturating_sub(4);
        normalized.truncate(new_len);
    }
    if normalized.is_empty() || normalized == "/" {
        String::new()
    } else if normalized.starts_with('/') {
        normalized
    } else {
        format!("/{normalized}")
    }
}

fn has_explicit_url_scheme(raw: &str) -> bool {
    let Some(index) = raw.find("://") else {
        return false;
    };
    let scheme = &raw[..index];
    let mut chars = scheme.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && chars.all(|value| {
            value.is_ascii_alphanumeric() || matches!(value, '+' | '.' | '-')
        })
}

fn scp_like_candidate(raw: &str) -> Option<String> {
    let colon = raw.find(':')?;
    let head = &raw[..colon];
    let tail = &raw[colon + 1..];
    if head.is_empty()
        || tail.is_empty()
        || head.chars().any(|value| value == '/' || value.is_whitespace())
    {
        return None;
    }
    Some(format!("ssh://{head}/{tail}"))
}

fn to_sanitization_candidate(raw: &str) -> String {
    if has_explicit_url_scheme(raw) {
        return raw.to_string();
    }
    let https = format!("https://{raw}");
    if Url::parse(&https).is_ok() {
        return https;
    }
    scp_like_candidate(raw).unwrap_or(https)
}

fn parsed_host_with_port(parsed: &Url) -> Option<String> {
    let host = parsed.host_str()?;
    match parsed.port() {
        Some(port) => Some(format!("{host}:{port}")),
        None => Some(host.to_string()),
    }
}

pub fn sanitize_remote_url(raw: &str) -> String {
    let value = raw.trim();
    if value.is_empty() {
        return String::new();
    }
    let Ok(parsed) = Url::parse(&to_sanitization_candidate(value)) else {
        return String::new();
    };
    let Some(host) = parsed.host_str() else {
        return String::new();
    };
    format!("{}{}", host, trim_remote_path(parsed.path())).to_ascii_lowercase()
}

pub fn sanitize_remote_url_for_http_url(raw: &str) -> String {
    let value = raw.trim();
    if value.is_empty() {
        return String::new();
    }
    if let Ok(parsed) = Url::parse(&to_sanitization_candidate(value)) {
        if let Some(host) = parsed_host_with_port(&parsed) {
            let protocol = match parsed.scheme() {
                "http" => "http",
                "https" => "https",
                _ => "https",
            };
            return format!(
                "{protocol}://{}{}",
                host.to_ascii_lowercase(),
                trim_remote_path(parsed.path())
            );
        }
    }
    let normalized = sanitize_remote_url(value);
    if normalized.is_empty() {
        String::new()
    } else {
        format!("https://{normalized}")
    }
}

fn valid_repo_segment(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && chars.all(|candidate| {
            candidate.is_ascii_alphanumeric() || matches!(candidate, '.' | '_' | '-')
        })
}

pub fn normalize_repo_reference(value: &str) -> String {
    let trimmed = value.trim();
    let mut parts = trimmed.split('/');
    let owner = parts.next().unwrap_or_default();
    let repo = parts.next().unwrap_or_default();
    if parts.next().is_none() && valid_repo_segment(owner) && valid_repo_segment(repo) {
        format!("https://github.com/{owner}/{repo}")
    } else {
        trimmed.to_string()
    }
}

pub fn build_repo_from_remote(
    remote: &str,
    base_branch: Option<&str>,
) -> Result<CloudAgentRepo, SandCloudAgentLaunchError> {
    let normalized = normalize_repo_reference(remote);
    let sanitized_repo_url = sanitize_remote_url(&normalized);
    let http_repo_url = sanitize_remote_url_for_http_url(&normalized);
    if sanitized_repo_url.is_empty() || http_repo_url.is_empty() {
        return Err(SandCloudAgentLaunchError::new(format!(
            "The Cursor agent could not parse the repository '{remote}'."
        )));
    }
    Ok(CloudAgentRepo {
        sanitized_repo_url,
        http_repo_url,
        base_branch: base_branch.map(str::to_string),
    })
}

fn describe_saved_environment(environment: &SavedEnvironment) -> String {
    let name = environment.name.trim();
    if name.is_empty() {
        environment.public_id.clone()
    } else {
        name.to_string()
    }
}

fn canonical_remote(value: &str) -> String {
    sanitize_remote_url(&normalize_repo_reference(value))
}

pub fn resolve_launch_repo_reference(
    repo_url: Option<&str>,
    saved_environment: Option<&SavedEnvironment>,
) -> Result<String, SandCloudAgentLaunchError> {
    let requested = repo_url.unwrap_or_default().trim();
    let Some(saved) = saved_environment else {
        if requested.is_empty() {
            return Err(SandCloudAgentLaunchError::new(
                "'repo_url' is required to launch a cloud agent.",
            ));
        }
        return Ok(requested.to_string());
    };

    let label = describe_saved_environment(saved);
    let repos = saved
        .repo_config
        .as_ref()
        .map(|config| config.repos.as_slice())
        .unwrap_or_default();
    let primary = repos
        .first()
        .map(|entry| entry.repo_url.trim())
        .unwrap_or_default();

    if primary.is_empty() {
        if requested.is_empty() {
            return Err(SandCloudAgentLaunchError::new(format!(
                "The '{label}' environment has no repositories configured, so pass repo_url to pick the repo to launch on."
            )));
        }
        return Ok(requested.to_string());
    }

    if requested.is_empty() || canonical_remote(requested) == canonical_remote(primary) {
        return Ok(primary.to_string());
    }

    let secondary = repos
        .iter()
        .skip(1)
        .any(|entry| canonical_remote(&entry.repo_url) == canonical_remote(requested));
    if secondary {
        return Err(SandCloudAgentLaunchError::new(format!(
            "'{requested}' is a secondary repo of the '{label}' environment; the launch's branch/PR lands on its primary repo '{primary}'. Omit repo_url to use it."
        )));
    }

    let listed = repos
        .iter()
        .map(|entry| entry.repo_url.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Err(SandCloudAgentLaunchError::new(format!(
        "'{requested}' is not part of the '{label}' environment (repos: {listed}). Omit repo_url to use its primary repo, or pick a different environment."
    )))
}

pub fn to_start_repo_config(config: Option<&SavedEnvironmentRepoConfig>) -> Option<Value> {
    let config = config?;
    if config.repos.is_empty() {
        return None;
    }
    Some(json!({
        "repos": config.repos.iter().map(|repo| {
            let mut value = Map::new();
            value.insert("repoUrl".into(), Value::String(repo.repo_url.clone()));
            if let Some(id) = &repo.scm_repo_node_id {
                value.insert("scmRepoNodeId".into(), Value::String(id.clone()));
            }
            if let Some(id) = &repo.git_enterprise_uuid {
                value.insert("gitEnterpriseUuid".into(), Value::String(id.clone()));
            }
            Value::Object(value)
        }).collect::<Vec<_>>()
    }))
}

pub fn resolve_cloud_agent_environment_fields(
    repo_url: &str,
    environment: Option<&CloudAgentEnvironment>,
) -> Result<Value, SandCloudAgentLaunchError> {
    match environment {
        None
        | Some(CloudAgentEnvironment::Cloud)
        | Some(CloudAgentEnvironment::Environment { .. }) => Ok(json!({})),
        Some(CloudAgentEnvironment::Pool { name, .. }) => {
            let repo = sanitize_remote_url(repo_url);
            if repo.is_empty() {
                return Err(SandCloudAgentLaunchError::new(format!(
                    "The Cursor agent could not derive private-worker routing labels from '{repo_url}'."
                )));
            }
            let mut labels = vec![json!({ "key": "repo", "value": repo })];
            if let Some(name) = name.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
                labels.push(json!({ "key": "pool", "value": name }));
            }
            Ok(json!({ "usePrivateWorker": true, "labels": labels }))
        }
        Some(CloudAgentEnvironment::Machine { name, .. }) => {
            let name = name.trim();
            if name.is_empty() {
                return Err(SandCloudAgentLaunchError::new(
                    "A private-worker machine environment requires a registered name.",
                ));
            }
            Ok(json!({
                "usePrivateWorker": true,
                "labels": [
                    { "key": "name", "value": name },
                    {
                        "key": PRIVATE_WORKER_SHARED_ASSIGNMENT_ALLOWED_LABEL_KEY,
                        "value": "true"
                    }
                ]
            }))
        }
    }
}
