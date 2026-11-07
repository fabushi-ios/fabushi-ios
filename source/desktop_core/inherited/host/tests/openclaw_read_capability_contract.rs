//! Contract test suite for OpenClaw Media Read Capabilities & Load Options (`OCRM-UNIT-MEDIA-013`).
//!
//! Validates:
//! - Outbound media load options normalization and empty root reduction.
//! - Rejection of host read capability without explicit local roots or 'any'.
//! - Host opener isolation (outbound media access vs host outbound media access).
//! - Agent workspace selection and automatic local root containment.
//! - Tool policy denial revoking host read capability and restricting to managed outbound media roots.
//! - Non-host workspace mode (`allow_host_workspace: false`).
//! - Gateway dispatch integration for `buildOutboundMediaLoadOptions` and `resolveAgentScopedOutboundMediaAccess`.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use mahayana_host_runtime::extensions::attachments::attachments_service::{
    AttachmentsBox, AttachmentsService,
};
use mahayana_host_runtime::extensions::attachments::generate_image_service::GenerateImageAuth;
use mahayana_host_runtime::extensions::attachments::load_options::{
    build_outbound_media_load_options,
    resolve_outbound_media_local_roots, OutboundMediaAccess, OutboundMediaLoadParams,
    OutboundMediaLocalRoots,
};
use mahayana_host_runtime::extensions::attachments::read_capability::{
    is_agent_scoped_media_read_allowed_by_tool_policy,
    resolve_agent_scoped_host_outbound_media_access,
    resolve_agent_scoped_outbound_media_access, AgentScopedOutboundMediaAccessParams,
};

struct TempTestDir {
    path: PathBuf,
}

impl TempTestDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("ocrm-read-cap-test-{name}-{}", uuid::Uuid::new_v4()));
        let _ = fs::create_dir_all(&path);
        Self { path }
    }
}

impl Drop for TempTestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct MockAuth;
impl GenerateImageAuth for MockAuth {
    fn get_access_token(&self) -> Result<String, String> {
        Ok("token".into())
    }
    fn get_machine_id(&self) -> Result<String, String> {
        Ok("machine".into())
    }
}

struct MockBox;
impl AttachmentsBox for MockBox {
    fn run_state(&self, _agent_id: &str) -> Result<String, String> {
        Ok("running".into())
    }
    fn upload_file(&self, _agent_id: &str, _path: &str, _data: &[u8]) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn test_ocrm_load_options_normalization_and_root_validation() {
    // 1. Normalizes an empty local root list to None
    let empty_roots = OutboundMediaLocalRoots::Explicit(vec![]);
    assert_eq!(resolve_outbound_media_local_roots(Some(&empty_roots)), None);

    // 2. Normalizes non-empty explicit roots
    let valid_roots = OutboundMediaLocalRoots::Explicit(vec![PathBuf::from("/tmp/workspace")]);
    assert_eq!(
        resolve_outbound_media_local_roots(Some(&valid_roots)),
        Some(valid_roots.clone())
    );

    // 3. Preserves "any" sentinel
    let any_roots = OutboundMediaLocalRoots::Any("any".to_string());
    assert_eq!(
        resolve_outbound_media_local_roots(Some(&any_roots)),
        Some(OutboundMediaLocalRoots::Any("any".to_string()))
    );

    // 4. Builds valid load options with explicit roots
    let params1 = OutboundMediaLoadParams {
        max_bytes: Some(1024),
        media_local_roots: Some(valid_roots.clone()),
        ..Default::default()
    };
    let opts1 = build_outbound_media_load_options(&params1).unwrap();
    assert_eq!(opts1.max_bytes, Some(1024));
    assert_eq!(opts1.local_roots, Some(valid_roots));
    assert!(!opts1.host_read_capability);

    // 5. Builds valid load options with "any" sentinel and host read capability
    let params2 = OutboundMediaLoadParams {
        max_bytes: Some(4096),
        media_local_roots: Some(any_roots.clone()),
        has_read_file: true,
        ..Default::default()
    };
    let opts2 = build_outbound_media_load_options(&params2).unwrap();
    assert_eq!(opts2.max_bytes, Some(4096));
    assert_eq!(opts2.local_roots, Some(any_roots));
    assert!(opts2.host_read_capability);

    // 6. Rejects host read capability without explicit local roots
    let invalid_params1 = OutboundMediaLoadParams {
        max_bytes: Some(1024),
        media_access: Some(OutboundMediaAccess {
            has_read_file: true,
            ..Default::default()
        }),
        has_read_file: true,
        ..Default::default()
    };
    let err1 = build_outbound_media_load_options(&invalid_params1).unwrap_err();
    assert!(err1.contains("Host media read requires explicit localRoots"));

    let invalid_params2 = OutboundMediaLoadParams {
        max_bytes: Some(1024),
        has_read_file: true,
        ..Default::default()
    };
    let err2 = build_outbound_media_load_options(&invalid_params2).unwrap_err();
    assert!(err2.contains("Host media read requires explicit localRoots"));
}

#[test]
fn test_ocrm_read_capability_host_opener_isolation() {
    let params = AgentScopedOutboundMediaAccessParams {
        workspace_dir: Some(PathBuf::from("/tmp/openclaw-home/workspace-main")),
        tools_allow: Some(vec!["read".to_string()]),
        ..Default::default()
    };

    // Public access has has_open_file = false
    let access = resolve_agent_scoped_outbound_media_access(&params);
    assert!(access.has_read_file);
    assert_eq!(
        access.workspace_dir,
        Some(PathBuf::from("/tmp/openclaw-home/workspace-main"))
    );

    // Host access has has_open_file = true
    let host_access = resolve_agent_scoped_host_outbound_media_access(&params);
    assert!(host_access.has_read_file);
    assert!(host_access.has_open_file);
    assert_eq!(
        host_access.workspace_dir,
        Some(PathBuf::from("/tmp/openclaw-home/workspace-main"))
    );
}

#[test]
fn test_ocrm_read_capability_workspace_selection_and_containment() {
    let temp = TempTestDir::new("ws-selection");
    let provided = temp.path.join("provided");
    let override_dir = temp.path.join("override");
    fs::create_dir_all(&provided).unwrap();
    fs::create_dir_all(&override_dir).unwrap();

    // 1. Without explicit override, uses provided media access workspace
    let params1 = AgentScopedOutboundMediaAccessParams {
        media_access: Some(OutboundMediaAccess {
            workspace_dir: Some(provided.clone()),
            ..Default::default()
        }),
        tools_allow: Some(vec!["read".to_string()]),
        state_dir: Some(temp.path.clone()),
        ..Default::default()
    };
    let res1 = resolve_agent_scoped_outbound_media_access(&params1);
    assert_eq!(res1.workspace_dir, Some(provided.clone()));
    let roots1 = res1.local_roots.unwrap();
    assert!(roots1.contains(&provided));

    // 2. With explicit override, override takes precedence
    let params2 = AgentScopedOutboundMediaAccessParams {
        workspace_dir: Some(override_dir.clone()),
        media_access: Some(OutboundMediaAccess {
            workspace_dir: Some(provided.clone()),
            ..Default::default()
        }),
        tools_allow: Some(vec!["read".to_string()]),
        state_dir: Some(temp.path.clone()),
        ..Default::default()
    };
    let res2 = resolve_agent_scoped_outbound_media_access(&params2);
    assert_eq!(res2.workspace_dir, Some(override_dir.clone()));
    let roots2 = res2.local_roots.unwrap();
    assert!(roots2.contains(&override_dir));
}

#[test]
fn test_ocrm_read_capability_policy_denial_and_managed_roots() {
    let temp = TempTestDir::new("policy-denial");
    let config_dir = temp.path.join("config");
    let state_dir = temp.path.join("state");
    fs::create_dir_all(&config_dir).unwrap();
    fs::create_dir_all(&state_dir).unwrap();

    // 1. Tool policy denying "read" revokes host read capability and restricts to managed roots
    let params_denied = AgentScopedOutboundMediaAccessParams {
        workspace_dir: Some(temp.path.join("workspace")),
        tools_deny: Some(vec!["read".to_string()]),
        config_dir: Some(config_dir.clone()),
        state_dir: Some(state_dir.clone()),
        ..Default::default()
    };
    assert!(!is_agent_scoped_media_read_allowed_by_tool_policy(&params_denied));

    let res_denied = resolve_agent_scoped_outbound_media_access(&params_denied);
    assert!(!res_denied.has_read_file);
    let roots_denied = res_denied.local_roots.unwrap();
    let managed_expected = config_dir.join("media").join("outbound");
    assert_eq!(roots_denied, vec![managed_expected]);

    // 2. allow_host_workspace: false forces managed roots regardless of tool policy
    let params_no_host_ws = AgentScopedOutboundMediaAccessParams {
        workspace_dir: Some(temp.path.join("workspace")),
        allow_host_workspace: Some(false),
        tools_allow: Some(vec!["read".to_string()]),
        config_dir: Some(config_dir.clone()),
        state_dir: Some(state_dir.clone()),
        ..Default::default()
    };
    let res_no_host_ws = resolve_agent_scoped_outbound_media_access(&params_no_host_ws);
    assert!(!res_no_host_ws.has_read_file);
    assert_eq!(res_no_host_ws.workspace_dir, None);
    let managed_roots = res_no_host_ws.local_roots.unwrap();
    assert_eq!(managed_roots, vec![config_dir.join("media").join("outbound")]);
}

#[test]
fn test_ocrm_read_capability_gateway_dispatch() {
    let temp = TempTestDir::new("gateway-dispatch");
    let service = AttachmentsService::new(
        &temp.path,
        Arc::new(MockAuth),
        Arc::new(MockBox),
        None,
    );

    // 1. Dispatch buildOutboundMediaLoadOptions
    let build_args = serde_json::json!({
        "maxBytes": 2048,
        "mediaLocalRoots": ["/tmp/workspace"],
        "hasReadFile": false
    });
    let build_res = service.dispatch_gateway("buildOutboundMediaLoadOptions", &build_args);
    assert!(build_res.is_some());
    let val = build_res.unwrap().unwrap();
    assert_eq!(val["maxBytes"], 2048);
    assert_eq!(val["hostReadCapability"], false);

    // 2. Dispatch resolveAgentScopedOutboundMediaAccess
    let access_args = serde_json::json!({
        "workspaceDir": "/tmp/workspace-agent",
        "toolsAllow": ["read"]
    });
    let access_res = service.dispatch_gateway("resolveAgentScopedOutboundMediaAccess", &access_args);
    assert!(access_res.is_some());
    let val2 = access_res.unwrap().unwrap();
    assert_eq!(val2["workspaceDir"], "/tmp/workspace-agent");
    assert_eq!(val2["hasReadFile"], true);
}
