//! Agent-scoped outbound media read capability resolver and policy enforcement.
//!
//! Ported with 100% behavioral equivalence from OpenClaw:
//! - `src/media/read-capability.ts`

use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

use crate::extensions::attachments::load_options::OutboundMediaAccess;
use crate::extensions::attachments::local_roots::{
    get_agent_scoped_media_local_roots_for_sources,
};
use crate::extensions::attachments::media_store::get_media_dir;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostOutboundMediaAccess {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_roots: Option<Vec<PathBuf>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_dir: Option<PathBuf>,
    pub has_read_file: bool,
    pub has_open_file: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboundHostMediaPolicyContext {
    pub session_key: Option<String>,
    pub message_provider: Option<String>,
    pub group_id: Option<String>,
    pub group_channel: Option<String>,
    pub group_space: Option<String>,
    pub account_id: Option<String>,
    pub requester_sender_id: Option<String>,
    pub requester_sender_name: Option<String>,
    pub requester_sender_username: Option<String>,
    pub requester_sender_e164: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentScopedOutboundMediaAccessParams {
    pub agent_id: Option<String>,
    pub media_sources: Option<Vec<String>>,
    pub workspace_dir: Option<PathBuf>,
    pub session_workspace_dir: Option<PathBuf>,
    pub workspace_only: Option<bool>,
    pub allow_host_workspace: Option<bool>,
    pub media_access: Option<OutboundMediaAccess>,
    pub workspace_media_access: Option<OutboundMediaAccess>,
    pub policy_context: Option<OutboundHostMediaPolicyContext>,
    pub tools_allow: Option<Vec<String>>,
    pub tools_deny: Option<Vec<String>>,
    pub state_dir: Option<PathBuf>,
    pub config_dir: Option<PathBuf>,
}

pub fn is_agent_scoped_media_read_allowed_by_tool_policy(
    params: &AgentScopedOutboundMediaAccessParams,
) -> bool {
    if let Some(ref deny) = params.tools_deny {
        if deny.iter().any(|t| t == "read" || t == "*") {
            return false;
        }
    }
    if let Some(ref allow) = params.tools_allow {
        if !allow.iter().any(|t| t == "read" || t == "*") {
            return false;
        }
    }
    true
}

pub fn get_managed_media_local_roots(
    config_dir: Option<&Path>,
    media_sources: Option<&[String]>,
) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let base = match config_dir {
        Some(d) => d.join("media").join("outbound"),
        None => get_media_dir().join("outbound"),
    };
    roots.push(base);

    if let Some(sources) = media_sources {
        for s in sources {
            let p = Path::new(s);
            if let Some(parent) = p.parent() {
                if parent.to_string_lossy().contains(".openclaw")
                    || parent.to_string_lossy().contains("media")
                {
                    if !roots.contains(&parent.to_path_buf()) {
                        roots.push(parent.to_path_buf());
                    }
                }
            }
        }
    }
    roots
}

fn append_workspace_dir_to_local_roots(
    roots: &mut Vec<PathBuf>,
    workspace_dir: Option<&Path>,
) {
    if let Some(ws) = workspace_dir {
        let ws_buf = ws.to_path_buf();
        if !roots.contains(&ws_buf) {
            roots.push(ws_buf);
        }
    }
}

pub fn resolve_agent_scoped_media_access(
    params: &AgentScopedOutboundMediaAccessParams,
    include_host_opener: bool,
) -> HostOutboundMediaAccess {
    let state_dir = params
        .state_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let config_dir = params
        .config_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from("/tmp"));

    if params.allow_host_workspace == Some(false) {
        let managed = get_managed_media_local_roots(
            Some(&config_dir),
            params.media_sources.as_deref(),
        );
        return HostOutboundMediaAccess {
            local_roots: Some(managed),
            workspace_dir: None,
            has_read_file: false,
            has_open_file: false,
        };
    }

    let resolved_workspace_dir = params
        .workspace_dir
        .clone()
        .or_else(|| params.media_access.as_ref().and_then(|a| a.workspace_dir.clone()))
        .or_else(|| {
            params
                .workspace_media_access
                .as_ref()
                .and_then(|a| a.workspace_dir.clone())
        })
        .or_else(|| {
            params
                .agent_id
                .as_deref()
                .map(|id| state_dir.join(format!("workspace-{id}")))
        });

    let media_read_allowed = is_agent_scoped_media_read_allowed_by_tool_policy(params);

    if !media_read_allowed {
        let managed = get_managed_media_local_roots(
            Some(&config_dir),
            params.media_sources.as_deref(),
        );
        return HostOutboundMediaAccess {
            local_roots: Some(managed),
            workspace_dir: resolved_workspace_dir,
            has_read_file: false,
            has_open_file: false,
        };
    }

    let mut configured_roots = params
        .media_access
        .as_ref()
        .and_then(|a| a.local_roots.clone())
        .unwrap_or_else(|| {
            get_agent_scoped_media_local_roots_for_sources(
                &state_dir,
                &config_dir,
                params.agent_id.as_deref(),
                resolved_workspace_dir.as_deref(),
                params.session_workspace_dir.as_deref(),
                params.media_sources.as_deref().unwrap_or(&[]),
                !params.workspace_only.unwrap_or(false),
            )
        });

    if let Some(ref ws_access) = params.workspace_media_access {
        if let Some(ref ws_roots) = ws_access.local_roots {
            for r in ws_roots {
                if !configured_roots.contains(r) {
                    configured_roots.push(r.clone());
                }
            }
        }
    }

    append_workspace_dir_to_local_roots(
        &mut configured_roots,
        resolved_workspace_dir.as_deref(),
    );

    let has_read_file = params
        .media_access
        .as_ref()
        .map(|a| a.has_read_file)
        .unwrap_or(true);

    HostOutboundMediaAccess {
        local_roots: Some(configured_roots),
        workspace_dir: resolved_workspace_dir,
        has_read_file,
        has_open_file: include_host_opener,
    }
}

pub fn resolve_agent_scoped_outbound_media_access(
    params: &AgentScopedOutboundMediaAccessParams,
) -> OutboundMediaAccess {
    let host = resolve_agent_scoped_media_access(params, false);
    OutboundMediaAccess {
        local_roots: host.local_roots,
        workspace_dir: host.workspace_dir,
        has_read_file: host.has_read_file,
    }
}

pub fn resolve_agent_scoped_host_outbound_media_access(
    params: &AgentScopedOutboundMediaAccessParams,
) -> HostOutboundMediaAccess {
    resolve_agent_scoped_media_access(params, true)
}
