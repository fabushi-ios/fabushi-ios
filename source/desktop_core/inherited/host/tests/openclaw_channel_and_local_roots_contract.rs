use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use serde_json::json;

use mahayana_host_runtime::extensions::attachments::attachments_service::{
    AttachmentsBox, AttachmentsService,
};
use mahayana_host_runtime::extensions::attachments::channel_inbound_roots::{
    clear_channel_contract_registry, register_channel_contract,
    register_channel_initialization_failure,
    resolve_channel_inbound_attachment_roots,
    resolve_channel_inbound_attachment_roots_for_channel,
    resolve_channel_remote_inbound_attachment_roots,
    ChannelInboundRootsConfig, ChannelMediaContractApi, ChannelMsgContext,
    ChannelPluginManifest, ChannelPluginsConfig,
};
use mahayana_host_runtime::extensions::attachments::generate_image_service::GenerateImageAuth;
use mahayana_host_runtime::extensions::attachments::host_read_media_policy::{
    assert_host_read_media_allowed, AssertHostReadParams, HostReadHtmlTrust,
    HOST_READ_DECLARED_TEXT_ERROR,
};
use mahayana_host_runtime::extensions::attachments::local_roots::{
    filter_shared_media_local_roots, get_agent_scoped_media_local_roots_for_sources,
    get_default_media_local_roots, get_session_safe_default_media_local_roots,
};
use sha2::{Digest, Sha256};

struct TempTestDir {
    path: PathBuf,
}

impl TempTestDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("ocrm-roots-test-{name}-{}", uuid::Uuid::new_v4()));
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

static TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct LocalChatContract;
impl ChannelMediaContractApi for LocalChatContract {
    fn resolve_inbound_attachment_roots(&self, account_id: Option<&str>) -> Option<Vec<String>> {
        let acc = account_id.unwrap_or("default");
        Some(vec![format!("/local/{acc}")])
    }
    fn resolve_remote_inbound_attachment_roots(&self, account_id: Option<&str>) -> Option<Vec<String>> {
        let acc = account_id.unwrap_or("default");
        Some(vec![format!("/remote/{acc}")])
    }
}

fn acquire_test_lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn test_ocrm_channel_inbound_roots_fast_path_and_precedence() {
    let _lock = acquire_test_lock();
    clear_channel_contract_registry();
    register_channel_contract("localchat", LocalChatContract);

    let cfg = ChannelInboundRootsConfig::default();
    let ctx = ChannelMsgContext {
        surface: Some("localchat".to_string()),
        provider: None,
        account_id: Some("work".to_string()),
    };

    let inbound = resolve_channel_inbound_attachment_roots(&cfg, &[], &ctx).unwrap();
    assert_eq!(inbound, Some(vec!["/local/work".to_string()]));

    let remote = resolve_channel_remote_inbound_attachment_roots(&cfg, &[], &ctx).unwrap();
    assert_eq!(remote, Some(vec!["/remote/work".to_string()]));

    // Explicit channel id query
    let direct = resolve_channel_inbound_attachment_roots_for_channel(&cfg, &[], Some("localchat"), Some("alice")).unwrap();
    assert_eq!(direct, Some(vec!["/local/alice".to_string()]));

    clear_channel_contract_registry();
}

#[test]
fn test_ocrm_channel_inbound_roots_failure_propagation() {
    let _lock = acquire_test_lock();
    clear_channel_contract_registry();
    let channel_id = "fixture-media-contract-failure";
    let failure_msg = "Unable to resolve bundled plugin public surface synthetic dependency failure";

    register_channel_initialization_failure(channel_id, failure_msg);

    let cfg = ChannelInboundRootsConfig::default();
    let result = resolve_channel_inbound_attachment_roots_for_channel(&cfg, &[], Some(channel_id), Some("work"));

    assert!(result.is_err(), "Initialization failure must propagate as Err");
    assert_eq!(result.unwrap_err(), failure_msg);

    clear_channel_contract_registry();
}

#[test]
fn test_ocrm_channel_inbound_roots_policy_denial() {
    let _lock = acquire_test_lock();
    clear_channel_contract_registry();
    register_channel_contract("imessage", LocalChatContract);

    let mut untrusted_manifest = ChannelPluginManifest {
        id: "imessage".to_string(),
        origin: "global".to_string(),
        channels: vec!["imessage".to_string()],
        trusted_official_install: false, // untrusted!
        root_dir: PathBuf::from("/plugins/imessage"),
    };

    let cfg = ChannelInboundRootsConfig::default();
    let ctx = ChannelMsgContext {
        surface: None,
        provider: Some("imessage".to_string()),
        account_id: Some("work".to_string()),
    };

    // Untrusted installed plugin ignored if not in registry
    clear_channel_contract_registry();
    let res = resolve_channel_inbound_attachment_roots(&cfg, &[untrusted_manifest.clone()], &ctx).unwrap();
    assert_eq!(res, None);

    // Trusted installed plugin succeeds
    untrusted_manifest.trusted_official_install = true;
    register_channel_contract("imessage", LocalChatContract);
    let res_trusted = resolve_channel_inbound_attachment_roots(&cfg, &[untrusted_manifest.clone()], &ctx).unwrap();
    assert_eq!(res_trusted, Some(vec!["/local/work".to_string()]));

    // Globally disabled plugins revoke authority
    let disabled_cfg = ChannelInboundRootsConfig {
        plugins: ChannelPluginsConfig {
            enabled: false,
            deny: vec![],
        },
        bundled_plugins_dir: None,
    };
    let res_disabled = resolve_channel_inbound_attachment_roots(&disabled_cfg, &[untrusted_manifest.clone()], &ctx).unwrap();
    assert_eq!(res_disabled, None);

    // Denied plugin in config revokes authority
    let denied_cfg = ChannelInboundRootsConfig {
        plugins: ChannelPluginsConfig {
            enabled: true,
            deny: vec!["imessage".to_string()],
        },
        bundled_plugins_dir: None,
    };
    let res_denied = resolve_channel_inbound_attachment_roots(&denied_cfg, &[untrusted_manifest], &ctx).unwrap();
    assert_eq!(res_denied, None);

    clear_channel_contract_registry();
}

#[test]
fn test_ocrm_local_roots_defaults_and_filtering() {
    let temp = TempTestDir::new("local-roots-defaults");
    let state_dir = temp.path.join("state");
    let config_dir = temp.path.join("config");
    fs::create_dir_all(&state_dir).unwrap();
    fs::create_dir_all(&config_dir).unwrap();

    let defaults = get_default_media_local_roots(&state_dir, &config_dir);
    let defaults_str: Vec<String> = defaults.iter().map(|p| p.to_string_lossy().to_string()).collect();

    assert!(defaults_str.iter().any(|p| p.ends_with("media")));
    assert!(defaults_str.iter().any(|p| p.ends_with("canvas")));
    assert!(defaults_str.iter().any(|p| p.ends_with("workspace")));
    assert!(defaults_str.iter().any(|p| p.ends_with("sandboxes")));

    // Safe default roots drop the shared sandboxes parent
    let safe_defaults = get_session_safe_default_media_local_roots(&state_dir, &config_dir, None);
    let safe_str: Vec<String> = safe_defaults.iter().map(|p| p.to_string_lossy().to_string()).collect();
    let sandboxes_dir_str = state_dir.join("sandboxes").to_string_lossy().to_string();
    assert!(!safe_str.contains(&sandboxes_dir_str));

    // Passing the shared sandboxes directory itself as session_workspace_dir is rejected
    let bad_session = state_dir.join("sandboxes");
    let roots_bad = filter_shared_media_local_roots(&defaults, &state_dir, Some(&bad_session));
    let roots_bad_str: Vec<String> = roots_bad.iter().map(|p| p.to_string_lossy().to_string()).collect();
    assert!(!roots_bad_str.contains(&sandboxes_dir_str));
}

#[test]
fn test_ocrm_local_roots_sibling_isolation_and_parent_expansion() {
    let temp = TempTestDir::new("local-roots-sibling");
    let state_dir = temp.path.join("state");
    let config_dir = temp.path.join("config");
    fs::create_dir_all(&state_dir).unwrap();
    fs::create_dir_all(&config_dir).unwrap();

    let session_a_ws = state_dir.join("sandboxes").join("session-a");
    let session_b_ws = state_dir.join("sandboxes").join("session-b");
    fs::create_dir_all(&session_a_ws).unwrap();
    fs::create_dir_all(&session_b_ws).unwrap();

    let sibling_secret_file = session_b_ws.join("secret.txt");
    fs::write(&sibling_secret_file, b"confidential").unwrap();

    let media_sources = vec![sibling_secret_file.to_string_lossy().to_string()];

    // Expand roots for session A naming session B's file
    let roots = get_agent_scoped_media_local_roots_for_sources(
        &state_dir,
        &config_dir,
        Some("ops"),
        None,
        Some(&session_a_ws),
        &media_sources,
        true, // tool fs expansion allowed
    );

    let roots_str: Vec<String> = roots.iter().map(|p| p.to_string_lossy().to_string()).collect();

    // Session A's workspace must be present (compare canonicalized)
    let canon_a = fs::canonicalize(&session_a_ws).unwrap().to_string_lossy().to_string();
    assert!(roots_str.contains(&canon_a));

    // Sibling session B's workspace must NEVER be present!
    let canon_b = fs::canonicalize(&session_b_ws).unwrap().to_string_lossy().to_string();
    assert!(!roots_str.contains(&canon_b));
    let canon_sandboxes = fs::canonicalize(state_dir.join("sandboxes")).unwrap().to_string_lossy().to_string();
    assert!(!roots_str.contains(&canon_sandboxes));
}

#[test]
fn test_ocrm_host_read_media_policy_format_admission() {
    let temp = TempTestDir::new("host-read-policy");
    let txt_file = temp.path.join("doc.txt");
    fs::write(&txt_file, b"Hello World, this is normal ascii text.\nLine 2.\n").unwrap();

    // Valid plain text with .txt extension succeeds
    let p_txt = AssertHostReadParams {
        file_path: Some(&txt_file),
        content_type: Some("text/plain"),
        buffer: Some(b"Hello World, this is normal ascii text.\nLine 2.\n"),
        ..Default::default()
    };
    assert!(assert_host_read_media_allowed(&p_txt).is_ok());

    // Bogus binary text fails
    let binary_bytes = vec![0u8, 1, 2, 3, 4, 5, 255, 254];
    let p_binary = AssertHostReadParams {
        file_path: Some(&txt_file),
        content_type: Some("text/plain"),
        buffer: Some(&binary_bytes),
        ..Default::default()
    };
    assert!(assert_host_read_media_allowed(&p_binary).is_err());

    // HTML without provenance fails with HOST_READ_DECLARED_TEXT_ERROR
    let html_file = temp.path.join("report.html");
    let html_content = b"<!doctype html><html><body>Report</body></html>";
    fs::write(&html_file, html_content).unwrap();

    let p_html_untrusted = AssertHostReadParams {
        file_path: Some(&html_file),
        content_type: Some("text/html"),
        buffer: Some(html_content),
        ..Default::default()
    };
    let err = assert_host_read_media_allowed(&p_html_untrusted).unwrap_err();
    assert!(err.0.contains(HOST_READ_DECLARED_TEXT_ERROR));

    // HTML with trusted outbound SHA256 and size succeeds
    let sha256_hex = format!("{:x}", Sha256::digest(html_content));
    let trust = HostReadHtmlTrust::Outbound {
        expected_sha256: sha256_hex,
        expected_size: html_content.len(),
    };
    let p_html_trusted = AssertHostReadParams {
        file_path: Some(&html_file),
        content_type: Some("text/html"),
        buffer: Some(html_content),
        trusted_generated_html_path: Some(&trust),
        ..Default::default()
    };
    assert!(assert_host_read_media_allowed(&p_html_trusted).is_ok());

    // Binary media (image/png) succeeds without text check
    let p_png = AssertHostReadParams {
        sniffed_content_type: Some("image/png"),
        kind: Some("image"),
        buffer: Some(b"\x89PNG\r\n\x1a\nfake"),
        ..Default::default()
    };
    assert!(assert_host_read_media_allowed(&p_png).is_ok());
}

#[test]
fn test_ocrm_channel_and_local_roots_gateway_dispatch() {
    let temp = TempTestDir::new("roots-gateway");
    let service = AttachmentsService::new(
        &temp.path,
        Arc::new(MockAuth),
        Arc::new(MockBox),
        None,
    );

    // 1. resolveLocalRoots
    let roots_val = service.dispatch_gateway(
        "resolveLocalRoots",
        &json!({
            "stateDir": temp.path.join("state").to_string_lossy().to_string(),
            "configDir": temp.path.join("config").to_string_lossy().to_string(),
            "agentId": "test-agent"
        }),
    );
    assert!(roots_val.is_some());
    assert!(roots_val.unwrap().is_ok());

    // 2. assertHostReadMediaAllowed
    let allow_val = service.dispatch_gateway(
        "assertHostReadMediaAllowed",
        &json!({
            "sniffedContentType": "image/jpeg",
            "kind": "image"
        }),
    );
    assert!(allow_val.is_some());
    let res = allow_val.unwrap();
    assert!(res.is_ok());
    assert_eq!(res.unwrap()["allowed"], true);
}
