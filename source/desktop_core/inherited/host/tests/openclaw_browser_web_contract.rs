use std::sync::Arc;

use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use mahayana_host_runtime::runner::routed_provider_runtime::RoutedToolBridge;
use mahayana_host_runtime::runner::sand_browser_auto_review::{
    SAND_BROWSER_AUTO_REVIEW_MAX_CDP_PARAMS_CHARS,
    SAND_BROWSER_AUTO_REVIEW_MAX_ELEMENT_CHARS,
    SAND_BROWSER_AUTO_REVIEW_MAX_FILE_PATH_CHARS,
    SAND_BROWSER_AUTO_REVIEW_MAX_KEY_CHARS,
    SAND_BROWSER_AUTO_REVIEW_MAX_TEXT_CHARS,
    SAND_BROWSER_AUTO_REVIEW_MAX_URL_CHARS,
    SandBrowserAutoReviewBlockedError,
    normalize_sand_browser_exact_action_args,
};
use mahayana_host_runtime::runner::tools::sand_browser_driver_source::SAND_BROWSER_DRIVER_SOURCE;
use mahayana_host_runtime::runner::tools::sand_browser_tools::{
    BOX_CDP_PORT_BASE, BrowserEnvelope, browser_tool_specs,
    build_browser_driver_invocation, decode_envelope, encode_envelope,
    parse_driver_response, to_driver_response, validate_browser_arguments,
};
use mahayana_host_runtime::runner::tools::sand_web_tools::{
    SandWebToolBridge, WEB_FETCH_TOOL_NAME,
    WebToolExecutor, render_web_fetch_result,
};
use serde_json::{Map, Value, json};

struct EmptyDelegateBridge;

impl RoutedToolBridge for EmptyDelegateBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(Vec::new())
    }

    fn call_tool(
        &self,
        _tool: &RoutedToolDefinition,
        _args: Value,
        _tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        Err(ProviderSessionError::Tool("unexpected delegate invocation".into()))
    }
}

struct MockWebExecutor;

impl WebToolExecutor for MockWebExecutor {
    fn search(
        &self,
        search_term: &str,
        explanation: Option<&str>,
    ) -> Result<Value, ProviderSessionError> {
        Ok(json!({
            "success": {
                "references": [
                    {
                        "title": "Search Result",
                        "url": "https://example.com",
                        "chunk": search_term
                    }
                ],
                "explanation": explanation
            }
        }))
    }

    fn fetch(&self, url: &str) -> Result<Value, ProviderSessionError> {
        Ok(json!({
            "success": {
                "url": url,
                "markdown": format!("# Content for {url}")
            }
        }))
    }
}

#[test]
fn test_ocrm_browser_o01_tool_dispatch_and_delimiter_framing() {
    let mut args = Map::new();
    args.insert("url".into(), json!("https://example.com"));

    let inv = build_browser_driver_invocation(4, "default-view", "navigate", "call-nav-1", &args, false);
    assert!(
        inv.shell_command.starts_with("node /workspace/.sand-browser/driver-v2.mjs "),
        "shell command must invoke node driver script"
    );
    assert!(
        inv.shell_command.contains(&inv.encoded_request),
        "shell command must pass base64 encoded request payload"
    );
    assert_eq!(
        inv.screenshot_path.as_deref(),
        Some("/workspace/.sand-browser/shot-call-nav-1.png"),
        "screenshot path must be formatted with sanitized tool call id"
    );

    // Verify driver response parsing with delimiter framing
    let raw_stdout = r#"
[driver stdout prefix logs...]
Connecting to CDP port 9226...
__SAND_BROWSER_RESULT__{"ok":true,"viewId":"tab-main","generation":"gen-tok-100","url":"https://example.com","title":"Example Domain","screenshot":true}
[trailing logs ignored]
"#;
    let res = parse_driver_response(raw_stdout).expect("parse driver response");
    assert!(res.ok);
    assert_eq!(res.view_id.as_deref(), Some("tab-main"));
    assert_eq!(res.generation.as_deref(), Some("gen-tok-100"));
    assert_eq!(res.url.as_deref(), Some("https://example.com"));
    assert_eq!(res.title.as_deref(), Some("Example Domain"));
    assert_eq!(res.screenshot, Some(true));
}

#[test]
fn test_ocrm_browser_o02_disallowed_cdp_command_rejection_in_driver() {
    // Verify that driver source contains strict allowlist gate and denies Runtime.evaluate
    assert!(
        SAND_BROWSER_DRIVER_SOURCE.contains("const allowedMethods = new Set(["),
        "driver must declare strict allowedMethods Set"
    );
    assert!(
        SAND_BROWSER_DRIVER_SOURCE.contains("\"Page.getLayoutMetrics\""),
        "Page.getLayoutMetrics must be permitted"
    );
    assert!(
        SAND_BROWSER_DRIVER_SOURCE.contains("\"Accessibility.getFullAXTree\""),
        "Accessibility.getFullAXTree must be permitted"
    );
    assert!(
        SAND_BROWSER_DRIVER_SOURCE.contains("CDP method \" + JSON.stringify(method) + \" is denied."),
        "driver must throw explicit denial message for arbitrary CDP methods"
    );

    // Verify that Runtime.evaluate is NOT present in the allowed set
    let allowed_start = SAND_BROWSER_DRIVER_SOURCE
        .find("const allowedMethods = new Set([")
        .expect("allowedMethods start");
    let allowed_end = SAND_BROWSER_DRIVER_SOURCE[allowed_start..]
        .find("]);")
        .expect("allowedMethods end");
    let allowed_block = &SAND_BROWSER_DRIVER_SOURCE[allowed_start..allowed_start + allowed_end];
    assert!(
        !allowed_block.contains("Runtime.evaluate"),
        "Runtime.evaluate must never be present in CDP allowed methods"
    );
}

#[test]
fn test_ocrm_browser_o03_allowed_cdp_observation_specs() {
    let specs = browser_tool_specs();
    assert_eq!(specs.len(), 20, "exactly 20 granular browser tools must be declared");

    let cdp_spec = specs
        .iter()
        .find(|s| s.name == "browser_cdp")
        .expect("browser_cdp tool");
    assert_eq!(cdp_spec.op, "cdp");
    assert!(
        cdp_spec.schema.required.contains(&"method"),
        "method must be a required parameter for browser_cdp"
    );

    let nav_spec = specs
        .iter()
        .find(|s| s.name == "browser_navigate")
        .expect("browser_navigate tool");
    assert_eq!(nav_spec.op, "navigate");
    assert!(nav_spec.schema.required.contains(&"url"));
}

#[test]
fn test_ocrm_browser_o04_view_generation_and_envelope_invariants() {
    let env = BrowserEnvelope {
        text: "{\"viewId\":\"view-alpha\",\"expectedGeneration\":\"gen-42\"}".into(),
        image_key: Some("key-1".into()),
    };
    let encoded = encode_envelope(&env);
    let decoded = decode_envelope(&encoded);
    assert_eq!(decoded.text, "{\"viewId\":\"view-alpha\",\"expectedGeneration\":\"gen-42\"}");
    assert_eq!(decoded.image_key.as_deref(), Some("key-1"));

    let mut driver_map = Map::new();
    driver_map.insert("ok".into(), json!(true));
    driver_map.insert("viewId".into(), json!("view-alpha"));
    driver_map.insert("generation".into(), json!("gen-42"));
    let resp = to_driver_response(&driver_map);
    assert_eq!(resp.view_id.as_deref(), Some("view-alpha"));
    assert_eq!(resp.generation.as_deref(), Some("gen-42"));

    // Argument validation check for browser_click
    let specs = browser_tool_specs();
    let click_spec = specs
        .iter()
        .find(|s| s.name == "browser_click")
        .expect("click tool");
    let mut args = Map::new();
    args.insert("viewId".into(), json!("tab-1"));
    let missing_ref = validate_browser_arguments(&click_spec.schema, &args);
    assert!(missing_ref.is_err(), "missing ref must fail argument validation");

    args.insert("ref".into(), json!("42"));
    let valid = validate_browser_arguments(&click_spec.schema, &args);
    assert!(valid.is_ok(), "specifying ref must pass validation");
    assert_eq!(BOX_CDP_PORT_BASE, 9_222);
}

#[test]
fn test_ocrm_browser_o05_file_upload_workspace_confinement() {
    // Verify restrictedUploadFile in driver source
    assert!(
        SAND_BROWSER_DRIVER_SOURCE.contains("function restrictedUploadFile(rawPath)"),
        "driver must contain restrictedUploadFile guard"
    );
    assert!(
        SAND_BROWSER_DRIVER_SOURCE.contains("Browser upload filePath escapes the authorized /workspace root."),
        "driver must reject paths escaping /workspace"
    );
    assert!(
        SAND_BROWSER_DRIVER_SOURCE.contains("Browser upload file exceeds the 25 MiB limit."),
        "driver must enforce 25 MiB upload limit"
    );

    // Verify tool specification requires ref and filePath
    let specs = browser_tool_specs();
    let upload_spec = specs
        .iter()
        .find(|s| s.name == "browser_upload_file")
        .expect("upload tool");
    assert!(upload_spec.schema.required.contains(&"ref"));
    assert!(upload_spec.schema.required.contains(&"filePath"));
}

#[test]
fn test_ocrm_browser_o06_file_download_size_cap_and_sha256_invariants() {
    // Verify persistTrackedDownload in driver source
    assert!(
        SAND_BROWSER_DRIVER_SOURCE.contains("async function persistTrackedDownload(download)"),
        "driver must contain persistTrackedDownload guard"
    );
    assert!(
        SAND_BROWSER_DRIVER_SOURCE.contains("Browser download exceeded the 64 MiB tracked-download limit."),
        "driver must enforce 64 MiB download cap and unlink oversized files"
    );
    assert!(
        SAND_BROWSER_DRIVER_SOURCE.contains("createHash(\"sha256\").update(bytes).digest(\"hex\")"),
        "driver must compute sha256 hash for tracked downloads"
    );

    // Verify download tool requires ref
    let specs = browser_tool_specs();
    let download_spec = specs
        .iter()
        .find(|s| s.name == "browser_download")
        .expect("download tool");
    assert!(download_spec.schema.required.contains(&"ref"));
}

#[test]
fn test_ocrm_browser_o07_web_fetch_ssrf_and_localhost_rejection() {
    let bridge: Arc<dyn RoutedToolBridge> = Arc::new(SandWebToolBridge::new(
        Arc::new(EmptyDelegateBridge),
        Arc::new(MockWebExecutor),
    ));
    let tools = bridge.list_tools().expect("tools");
    let fetch_tool = tools
        .iter()
        .find(|t| t.name == WEB_FETCH_TOOL_NAME)
        .expect("WebFetch tool");

    let ssrf_targets = [
        ("http://localhost:8080/metrics", "localhost"),
        ("http://127.0.0.1:1337/gateway", "private or loopback IP"),
        ("http://192.168.1.1/admin", "private or loopback IP"),
        ("http://10.0.0.1/internal", "private or loopback IP"),
        ("http://172.16.0.1/api", "private or loopback IP"),
        ("http://169.254.169.254/latest/meta-data", "private or loopback IP"),
        ("http://[::1]:3000/", "private or loopback IP"),
        ("ftp://example.com/file", "Invalid URL protocol"),
    ];

    for (url, expected_err) in ssrf_targets {
        let res = bridge.call_tool(fetch_tool, json!({ "url": url }), "call-ssrf");
        assert!(
            res.is_err(),
            "target URL {url} must be rejected by SSRF guard"
        );
        let err_msg = match res.unwrap_err() {
            ProviderSessionError::Tool(msg) => msg,
            other => format!("{other:?}"),
        };
        assert!(
            err_msg.contains(expected_err),
            "expected error containing '{expected_err}', got: '{err_msg}'"
        );
    }
}

#[test]
fn test_ocrm_browser_o08_web_fetch_userinfo_sanitization() {
    let bridge: Arc<dyn RoutedToolBridge> = Arc::new(SandWebToolBridge::new(
        Arc::new(EmptyDelegateBridge),
        Arc::new(MockWebExecutor),
    ));
    let tools = bridge.list_tools().expect("tools");
    let fetch_tool = tools
        .iter()
        .find(|t| t.name == WEB_FETCH_TOOL_NAME)
        .expect("WebFetch tool");

    let res = bridge
        .call_tool(
            fetch_tool,
            json!({ "url": "https://admin:secret123@example.com/api/data" }),
            "call-clean-userinfo",
        )
        .expect("fetch call");

    assert_eq!(
        res["success"]["url"],
        "https://example.com/api/data",
        "userinfo (admin:secret123) must be completely stripped before execution"
    );
}

#[test]
fn test_ocrm_browser_o09_web_fetch_content_size_truncation() {
    let long_markdown = "A".repeat(150_000);
    let res = render_web_fetch_result("https://example.com", Some(&long_markdown), None, None);

    let returned_markdown = res["success"]["markdown"]
        .as_str()
        .expect("markdown string");
    assert_eq!(
        returned_markdown.len(),
        100_000,
        "content must be truncated to exactly WEB_FETCH_MAX_CONTENT_CHARS (100,000)"
    );
}

#[test]
fn test_ocrm_browser_o10_auto_review_input_length_boundary_enforcement() {
    assert_eq!(SAND_BROWSER_AUTO_REVIEW_MAX_URL_CHARS, 2_000);
    assert_eq!(SAND_BROWSER_AUTO_REVIEW_MAX_TEXT_CHARS, 2_000);
    assert_eq!(SAND_BROWSER_AUTO_REVIEW_MAX_ELEMENT_CHARS, 500);
    assert_eq!(SAND_BROWSER_AUTO_REVIEW_MAX_KEY_CHARS, 256);
    assert_eq!(SAND_BROWSER_AUTO_REVIEW_MAX_CDP_PARAMS_CHARS, 2_000);
    assert_eq!(SAND_BROWSER_AUTO_REVIEW_MAX_FILE_PATH_CHARS, 4_096);

    // 1. Oversized URL (2001 chars)
    let oversized_url = "https://example.com/".to_string() + &"a".repeat(2000);
    let res1 = normalize_sand_browser_exact_action_args(&json!({
        "op": "navigate",
        "url": oversized_url
    }));
    assert!(res1.is_err());
    let err1 = match res1.unwrap_err() {
        SandBrowserAutoReviewBlockedError(msg) => msg,
    };
    assert!(err1.contains("max 2000 characters"));

    // 2. Conforming URL
    let res2 = normalize_sand_browser_exact_action_args(&json!({
        "op": "navigate",
        "url": "https://example.com/normal"
    }));
    assert!(res2.is_ok());

    // 3. Oversized text (2001 chars)
    let res3 = normalize_sand_browser_exact_action_args(&json!({
        "op": "type",
        "text": "b".repeat(2001)
    }));
    assert!(res3.is_err());
    let err3 = match res3.unwrap_err() {
        SandBrowserAutoReviewBlockedError(msg) => msg,
    };
    assert!(err3.contains("max 2000 characters"));
}
