use mahayana_host_runtime::extensions::proxy_capture::header_redactor::ProxyHeaderRedactor;
use mahayana_host_runtime::extensions::secrets::audit_manager::{
    SecretFindingKind, SecretsAuditManager,
};
use mahayana_host_runtime::extensions::security::policy_auditor::SecurityPolicyAuditor;
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;

#[test]
fn test_ocrm_secrets_audit_manager() {
    let mgr = SecretsAuditManager::new();

    let config = json!({
        "providers": {
            "anthropic": {
                "api_key": "sk-ant-api03-abcdef1234567890abcdef1234567890",
                "timeout": 30
            },
            "openai": {
                "apiKey": "[REDACTED]"
            },
            "custom": {
                "secret_token": "safe-normal-value"
            }
        }
    });

    let findings = mgr.audit_json_value(&config, "root");
    assert_eq!(findings.len(), 2);

    let plaintext = findings
        .iter()
        .find(|f| f.finding_kind == SecretFindingKind::PlaintextResidue)
        .expect("Plaintext residue found");
    assert!(plaintext.key_path.contains("anthropic.api_key"));
    assert_eq!(
        plaintext.suggested_secret_ref.as_deref(),
        Some("secret:api_key")
    );

    let redacted = findings
        .iter()
        .find(|f| f.finding_kind == SecretFindingKind::RedactedValue)
        .expect("Redacted finding found");
    assert!(redacted.key_path.contains("openai.apiKey"));

    // Channel env vars lookup
    let slack_vars = mgr.get_channel_env_vars("slack");
    assert_eq!(slack_vars, vec!["SLACK_BOT_TOKEN", "SLACK_APP_TOKEN"]);

    let discord_vars = mgr.get_channel_env_vars("DISCORD");
    assert_eq!(discord_vars, vec!["DISCORD_BOT_TOKEN"]);
}

#[test]
fn test_ocrm_security_policy_auditor() {
    let auditor = SecurityPolicyAuditor::new();

    // Safe binaries allowlist
    assert!(auditor.is_binary_allowed("git"));
    assert!(auditor.is_binary_allowed("/usr/bin/git"));
    assert!(auditor.is_binary_allowed("cargo"));
    assert!(auditor.is_binary_allowed("python3"));
    assert!(auditor.is_binary_allowed("bash"));

    assert!(!auditor.is_binary_allowed("arbitrary_exploit"));
    assert!(!auditor.is_binary_allowed("sudo"));

    // Symlink traversal check
    let root = Path::new("/tmp/workspace");
    let safe_child = Path::new("/tmp/workspace/src/main.rs");
    let outside_child = Path::new("/tmp/other/secret.key");

    assert!(auditor.check_symlink_traversal(root, safe_child).is_ok());
    assert!(auditor.check_symlink_traversal(root, outside_child).is_err());
}

#[test]
fn test_ocrm_proxy_capture_header_redactor() {
    let redactor = ProxyHeaderRedactor::new();

    let mut headers = HashMap::new();
    headers.insert(
        "Authorization".to_string(),
        "Bearer secret-token-123".to_string(),
    );
    headers.insert("X-Api-Key".to_string(), "api-key-999".to_string());
    headers.insert("Cookie".to_string(), "session=abc; auth=xyz".to_string());
    headers.insert("Content-Type".to_string(), "application/json".to_string());
    headers.insert("Accept".to_string(), "*/*".to_string());

    let redacted = redactor.redact_headers(&headers);

    assert_eq!(redacted.get("Authorization").unwrap(), "[REDACTED]");
    assert_eq!(redacted.get("X-Api-Key").unwrap(), "[REDACTED]");
    assert_eq!(redacted.get("Cookie").unwrap(), "[REDACTED]");
    assert_eq!(redacted.get("Content-Type").unwrap(), "application/json");
    assert_eq!(redacted.get("Accept").unwrap(), "*/*");

    let conservative = redactor.redact_headers_with_additional(
        &HashMap::from([
            ("x-vendor-access-token".to_string(), "vendor-secret".to_string()),
            ("x-session-id".to_string(), "session-123".to_string()),
            ("X-Routing-Target".to_string(), "private-route".to_string()),
            ("Accept".to_string(), "text/plain".to_string()),
        ]),
        ["x-routing-target"],
    );
    assert_eq!(conservative.get("x-vendor-access-token").unwrap(), "[REDACTED]");
    assert_eq!(conservative.get("x-session-id").unwrap(), "[REDACTED]");
    assert_eq!(conservative.get("X-Routing-Target").unwrap(), "[REDACTED]");
    assert_eq!(conservative.get("Accept").unwrap(), "text/plain");
    assert_eq!(
        redactor.redact_url("https://gateway.test/capture?token=live&safe=1&session_id=s-1"),
        "https://gateway.test/capture?token=[REDACTED]&safe=1&session_id=[REDACTED]"
    );
}
