use mahayana_host_runtime::extensions::inference::llm_transport_rate_limit::{
    is_terminal_assistant_error, redact_secret_sentinels_for_egress, AdmissionError,
    AdmissionResult, RateLimitAdmissionController, RequestPriority,
};
use mahayana_host_runtime::extensions::inference::stream_wrappers_service::{
    is_anthropic_bedrock_model, is_google_thinking_required_model,
    normalize_anthropic_tool_schema, resolve_anthropic_cache_family,
    resolve_google_thinking_level, AnthropicCacheFamily, GoogleThinkingLevel,
    OpenAiServiceTierObserver,
};
use mahayana_host_runtime::extensions::inference::oauth_registry_service::{
    generate_pkce_challenge_from_verifier, validate_anthropic_callback_host,
    OAuthCredentials, OAuthError, OAuthProviderId, OAuthProviderRegistry,
};
use serde_json::json;

#[test]
fn test_ocrm_llm_rate_limit_priority_and_admission() {
    let mut controller = RateLimitAdmissionController::new();
    let route = "openai:gpt-4o:account-1";

    // Immediate admission when open
    let res = controller.admit_request(route, RequestPriority::Interactive, 1000, 5000);
    assert_eq!(res, Ok(AdmissionResult::AdmittedImmediate));

    // Simulate 429 rate limit with 2000ms retry-after
    controller.record_rate_limit(route, 2000, 1000);

    // Background request arrives at t=1100
    let bg_res = controller.admit_request(route, RequestPriority::Background, 1100, 5000);
    assert_eq!(
        bg_res,
        Err(AdmissionError::RateLimited {
            retry_at_ms: 3000,
            queue_depth: 1
        })
    );

    // Interactive request arrives at t=1200
    let int_res = controller.admit_request(route, RequestPriority::Interactive, 1200, 5000);
    assert_eq!(
        int_res,
        Err(AdmissionError::RateLimited {
            retry_at_ms: 3000,
            queue_depth: 2
        })
    );

    // Third request with too short timeout (500ms when wait is 1700ms)
    let timeout_res = controller.admit_request(route, RequestPriority::Interactive, 1300, 500);
    assert_eq!(
        timeout_res,
        Err(AdmissionError::TimeoutExceeded {
            wait_ms: 1700,
            max_retry_delay_ms: 500
        })
    );

    // At t=2999, gate is still closed
    assert!(controller.pop_next_admitted(route, 2999).is_none());

    // At t=3000, gate opens; interactive waiter should be popped FIRST even though background arrived earlier
    let (first_waiter, first_res) = controller.pop_next_admitted(route, 3000).expect("first waiter");
    assert_eq!(first_waiter.priority, RequestPriority::Interactive);
    assert_eq!(first_res, AdmissionResult::AdmittedFromQueue { wait_ms: 1800 });

    // Second waiter is background
    let (second_waiter, second_res) = controller.pop_next_admitted(route, 3001).expect("second waiter");
    assert_eq!(second_waiter.priority, RequestPriority::Background);
    assert_eq!(second_res, AdmissionResult::AdmittedFromQueue { wait_ms: 1901 });

    // Queue empty
    assert!(controller.pop_next_admitted(route, 3002).is_none());
}

#[test]
fn test_ocrm_llm_terminal_error_and_secret_redaction() {
    // Terminal assistant error checks
    assert!(is_terminal_assistant_error(Some("repeated_tool"), None, None));
    assert!(is_terminal_assistant_error(Some("repetitive_output"), None, None));
    assert!(is_terminal_assistant_error(Some("provider_failure_with_output"), None, None));
    assert!(is_terminal_assistant_error(Some("provider_post_dispatch_ambiguity"), None, None));
    assert!(is_terminal_assistant_error(Some("websocket_non_retryable_close"), None, None));
    assert!(is_terminal_assistant_error(None, Some(401), None));
    assert!(is_terminal_assistant_error(None, Some(403), None));
    assert!(is_terminal_assistant_error(None, Some(404), None));

    // Transient retryable errors (not terminal)
    assert!(!is_terminal_assistant_error(None, Some(429), None));
    assert!(!is_terminal_assistant_error(None, Some(500), None));
    assert!(!is_terminal_assistant_error(None, Some(503), None));

    // Secret sentinels egress redaction
    let text = "Header Authorization: Bearer __OPENCLAW_SECRET_sk-live-999__ and key __OPENCLAW_SECRET_test-42__ ok.";
    let redacted = redact_secret_sentinels_for_egress(text);
    assert_eq!(
        redacted,
        "Header Authorization: Bearer [REDACTED_SECRET] and key [REDACTED_SECRET] ok."
    );
}

#[test]
fn test_ocrm_llm_stream_wrappers_cache_and_tools() {
    // Bedrock model and cache family detection
    assert!(is_anthropic_bedrock_model("us.anthropic.claude-sonnet-4-6"));
    assert!(is_anthropic_bedrock_model("arn:aws:bedrock:us-east-1:123456789012:application-inference-profile/my-claude-prod"));
    assert!(!is_anthropic_bedrock_model("arn:aws:bedrock:us-east-1:123456789012:application-inference-profile/my-llama-prod"));
    assert!(!is_anthropic_bedrock_model("gpt-4o"));

    assert_eq!(
        resolve_anthropic_cache_family("anthropic.claude-3-5-sonnet", "bedrock", None),
        AnthropicCacheFamily::AnthropicBedrock
    );
    assert_eq!(
        resolve_anthropic_cache_family("claude-3-5-sonnet-20241022", "anthropic", Some("https://api.anthropic.com/v1")),
        AnthropicCacheFamily::AnthropicDirect
    );
    assert_eq!(
        resolve_anthropic_cache_family("claude-3-5-sonnet", "custom", Some("https://proxy.example.com/v1")),
        AnthropicCacheFamily::CustomAnthropicApi
    );

    // Tool schema normalization
    let empty_schema = json!({});
    let normalized = normalize_anthropic_tool_schema(&empty_schema);
    assert_eq!(normalized, json!({"type": "object", "properties": {}}));

    // OpenAI Service tier observer
    let mut observer = OpenAiServiceTierObserver::new("acct-1", Some("scale".to_string()));
    assert_eq!(observer.effective_tier(1000), Some("scale".to_string()));

    // Record rejection at t=1000 with 10s cooloff
    observer.record_tier_rejection(1000, 10000);
    assert_eq!(observer.effective_tier(2000), Some("default".to_string()));
    // After cooloff at t=12000
    assert_eq!(observer.effective_tier(12000), Some("scale".to_string()));

    // Google Thinking Level
    assert!(is_google_thinking_required_model("gemini-2.5-pro"));
    assert!(!is_google_thinking_required_model("gemini-2.5-flash"));
    assert_eq!(
        resolve_google_thinking_level("gemini-2.5-pro", Some(0)),
        Some(GoogleThinkingLevel::Low) // Rejects 0, falls back to Low
    );
    assert_eq!(
        resolve_google_thinking_level("gemini-2.5-pro", Some(2048)),
        Some(GoogleThinkingLevel::Low)
    );
    assert_eq!(
        resolve_google_thinking_level("gemini-2.5-flash", Some(8192)),
        Some(GoogleThinkingLevel::Medium)
    );
}

#[test]
fn test_ocrm_llm_oauth_registry_and_pkce() {
    // Anthropic callback host security validation
    assert!(validate_anthropic_callback_host("127.0.0.1").is_ok());
    assert!(validate_anthropic_callback_host("localhost").is_ok());
    assert!(validate_anthropic_callback_host("::1").is_ok());
    assert!(validate_anthropic_callback_host("example.com").is_err());
    assert!(validate_anthropic_callback_host("192.168.1.1").is_err());

    // PKCE verification
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let challenge = generate_pkce_challenge_from_verifier(verifier);
    assert!(!challenge.is_empty());
    assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");

    // Registry token expiry check
    let mut registry = OAuthProviderRegistry::new();
    let creds = OAuthCredentials {
        access_token: "sk-ant-oauth-token-123".to_string(),
        refresh_token: "sk-ant-refresh-456".to_string(),
        expires_at_ms: 5000,
        token_type: "Bearer".to_string(),
        scope: Some("user:inference".to_string()),
    };
    registry.store_credentials(OAuthProviderId::Anthropic, creds);

    assert_eq!(
        registry.get_api_key(&OAuthProviderId::Anthropic, 4000),
        Ok("sk-ant-oauth-token-123".to_string())
    );
    assert_eq!(
        registry.get_api_key(&OAuthProviderId::Anthropic, 5001),
        Err(OAuthError::TokenExpired {
            expires_at_ms: 5000,
            now_ms: 5001
        })
    );
    assert_eq!(
        registry.get_api_key(&OAuthProviderId::OpenAICodex, 1000),
        Err(OAuthError::NotConfigured)
    );
}
