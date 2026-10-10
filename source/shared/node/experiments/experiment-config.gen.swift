import Foundation

/// Swift projection of the pinned Grok Bot 0.18 experiment registry.
///
/// The complete registry names are retained so iOS can audit every upstream
/// experiment/config key. Values with direct iOS product effects carry typed
/// fallbacks below; unrelated desktop-only knobs remain discoverable by name
/// without importing the Node/Statsig runtime.
indirect enum IOSExperimentConfigValue: Equatable, Sendable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([IOSExperimentConfigValue])
    case object([String: IOSExperimentConfigValue])

    var boolValue: Bool? {
        if case .bool(let value) = self { return value }
        return nil
    }

    var stringValue: String? {
        if case .string(let value) = self { return value }
        return nil
    }

    var numberValue: Double? {
        if case .number(let value) = self { return value }
        return nil
    }
}

let GROK_FEATURE_FLAG_NAMES = Set(BUNDLED_FEATURE_FLAGS.keys)

let GROK_EXPERIMENT_NAMES: Set<String> = [
    "composer_gc_safety_net_1s",
    "tsgo_disable_auto_imports_internal",
    "sand_model_selection",
    "limit_hit_ui_2026_06",
    "third_party_usage_nudge_2026_07",
    "third_party_usage_nudge_policy_2026_07",
    "third_party_on_demand_nudge_2026_08",
    "automations_button_in_ide",
    "local_agent_interruption_move_to_cloud_glass",
    "parallel_agents_try_cloud_nudge_glass",
    "long_running_local_agent_cloud_nudge_glass",
    "glass_sidebar_one_repo_environment_grouping",
    "dashboard_user_checklist_enterprise",
    "dashboard_admin_setup_enterprise",
    "dashboard_team_admin_onboarding_checklist",
    "new_team_admin_setup_wizard",
    "new_team_add_teammates_callout",
    "new_team_name_prefill",
    "dashboard_member_remove_modal",
    "push_mcps",
    "glass_setup_cloud_pill",
    "glass_start_on_new_chat",
    "glass_individual_onboarding_checklist",
    "send_to_cloud_composer_glass",
    "connect_repo_env_setup_pill_glass",
    "branch_mismatch_move_to_cloud_glass",
    "plan_build_cloud_button",
    "ide_connect_git_repos_start",
    "ide_steer_from_cloud_start",
    "cloud_agent_steer_from_phone_glass",
    "glass_account_menu_ios_download",
    "glass_account_menu_usage_remaining",
    "glass_agent_demos_setup_pill",
    "cloud_setup_cta_glass_running_agent_session",
    "set_up_env_pill_glass",
    "agent_desktop_glass_env_setup",
    "glass_automations_sidebar_new_tag",
    "new_tag_cloud_runtime_glass",
    "runtime_picker_discovery_nudge_glass",
    "remote_control_runtime_picker_glass",
    "default_web_users_cloud_in_glass",
    "cloud_setup_cta_web_running_agent_session",
    "suggested_prompts",
    "suggested_mode_switch",
    "plugin_keyword_nudge_rollout",
    "plugin_keyword_nudge_latency",
    "plugin_keyword_nudge_inline",
    "marketplace_tab_customize_label",
    "customize_default_manage_tab",
    "customize_migration",
    "marketplace_card_install_cta_ab",
    "marketplace_detail_authenticate_cta_ab",
    "marketplace_try_in_chat_prompt_ab",
    "composer_run_button_style",
    "new_placeholder",
    "cursor_launch_at_login",
    "subscription_only_degraded_extended_usage",
    "onboarding_default_layout_agent",
    "onboarding_left_right_chat",
    "separate_auto_and_api_usage_bars_for_individuals",
    "onboarding_skip_post_login",
    "free_user_locked_model_2026_05",
    "free_user_composer_grok_picker_2026_07",
    "model_picker_promote_first_party",
    "model_picker_promote_first_party_v3",
    "model_picker_usage_display_2026_08",
    "free_user_usage_summary_display_mode",
    "pro_auto_mode_new_users",
    "pro_auto_mode_existing_users",
    "premium_auto_mode",
    "terminal_tip",
    "cli_install_ad",
    "cli_install_ad_v2",
    "agent_backend_ab_test_1",
    "agent_backend_ab_test_2",
    "new_chat_auto_switch",
    "new_teams_pricing_cancellation_flow",
    "team_pending_cancellation_cancel_now",
    "yearly_upgrade_inplace",
    "cloud_agent_remove_on_demand_requirement",
    "dashboard_user_menu_view_plans",
    "dashboard_create_team_sidebar_cta",
    "sidebar_bottom_section_cta",
    "free_user_create_team_cta",
    "dashboard_free_overview_cleanup",
    "download_bottom_dashboard",
    "web_mobile_ios_launch_ad",
    "web_cloud_agents_agent_window_ad",
    "router_settings_disabled_info",
    "onboarding_redirect_git_to_login_deep_control",
    "dashboard_invite_modal_version",
    "set_up_env_pill_web",
    "completely_free_env_setup_glass_and_web",
    "env_setup_free_callout_web",
    "completely_free_env_setup_glass",
    "dynamic_automation_templates",
    "team_pinned_marketplace_plugins",
    "glass_new_chat_header",
    "glass_ftux_wizard",
    "glass_ftux_first_action",
    "glass_ftux_app_scan",
    "glass_recommended_actions",
    "glass_start_onboarding_pill",
    "terminal_agent_integration",
    "ide_update_ux_exp",
    "vega_launch_broadcast",
    "effort_first_model_picker",
    "effort_first_grouped_models_2026_08",
    "slash_menu_team_discovery_ranking",
]

let GROK_DYNAMIC_CONFIG_NAMES: Set<String> = [
    "mobile_iap_products",
    "remote_workspace_readiness_config",
    "ai_code_tracking_poll",
    "solidjs_stack_trace_limit",
    "idle_extension_host_killer_config",
    "marketplace_listing_config",
    "editor_bugbot_config",
    "client_speculative_summarization_config",
    "new_conversation_ux_config",
    "meta_agent_config",
    "task_card_tips",
    "product_tips_config",
    "composer_sandboxing_promo",
    "playwright_log_configs",
    "privacy_mode_acknowledgement_onboarding",
    "tools_concurrency_config",
    "client_rg",
    "http2_ping_config",
    "http2_agent_connection_pool_config",
    "http1_keepalive_config",
    "ws_dark_durability_probe_config",
    "abort_controller_logging_config",
    "hooks_client_config",
    "composer_hang_detection_config",
    "composer_errors_without_button_support",
    "nal_stall_detector_timeout_config",
    "nal_request_context_blob_transport_config",
    "simulated_thinking_error_timeout",
    "agent_loop_phase_display",
    "in_app_ads_dev_override_config",
    "in_app_ads_quiet_period_config",
    "environment_setup_resume_config",
    "perf_monitor_control",
    "glass_reactivated_user_routing_config",
    "retry_interceptor_config",
    "retry_interceptor_params_config",
    "text_delta_pacing_config",
    "extension_monitor_control",
    "agent_memory_pressure_monitor",
    "sand_process_metrics",
    "sand_rpc_tracing",
    "gc_trace_control",
    "disable_infinite_cloud_agent_stream_retries",
    "cloud_agent_shared_blob_cache",
    "sand_min_client_version",
    "sand_mobile_version_support",
    "sand_computer_use_playwright_config",
    "sand_browser_use_model",
    "grok_bot_conversation_size_limits",
    "sand_model_filter",
    "sand_default_model",
    "sand_automations_model",
    "agent_store_sync_client_config",
    "gemini_video_attachment_config",
    "agent_layout_migration",
    "default_diff_mode",
    "switch_mode_tool_config",
    "mcp_auth_status_copy_config",
    "mcp_reconnect_config",
    "mcp_oauth_sweep_config",
    "mcp_oauth_refresh_policy",
    "mcp_oauth_loopback_redirect",
    "mcp_oauth_backend_redis_lock_config",
    "sand_pressure_cpu_profiler_config",
    "inline_diff_performance_config",
    "tray_refresh_config",
    "performance_events_config",
    "background_composer_list_limit",
    "switch_to_model_slug_config",
    "debug_mode_ui_instructions_config",
    "user_intent_config",
    "browser_default_url_config",
    "glass_per_app_tabs_config",
    "glass_tiling_config",
    "glass_fsd_launch_pill_config",
    "glass_btw_side_question_prompt_config",
    "glass_loaded_agent_lru_cap",
    "glass_workspace_lifecycle_metrics",
    "glass_pr_operations_polling_config",
    "cloud_agent_stream_reattach",
    "tool_limits_config",
    "update_prompt_config",
    "internal_release_track_override",
    "sand_internal_release_track_override",
    "giant_json_stringify_config",
    "giant_buffer_retention_config",
    "giant_json_parse_config",
    "giant_vsbuffer_decode_config",
    "mcp_ipc_timeouts",
    "cc_override_models_config",
    "sentry_session_recording_config",
    "extension_signature_verification_bypass_list",
    "sandbox_default_network_allowlist",
    "auto_spillover_ui_config",
    "portal_outage_alert",
    "slack_mcp_client_id",
    "file_watcher_metrics_config",
    "file_watcher_forwarded_storm_config",
    "statsig_dummy_gauge_config",
    "memory_monitor_user_toast_config",
    "cpu_monitor_config",
    "memory_pressure_profiling_config",
    "memory_monitor_config",
    "plugin_onboarding_by_job_role",
    "leaked_disposables_tracker",
    "solidjs_memo_audit_config",
    "solidjs_listener_stacks_config",
    "shell_exec_output_backpressure_config",
    "canvas_prompt_text_config",
    "glass_start_onboarding_pill_config",
    "glass_ftux_first_action_config",
    "shutdown_hang_watchdog_config",
    "update_diagnostics",
    "startup_diagnostics",
    "renderer_ping_config",
    "editor_input_latency_metrics_config",
    "editor_tokenization_metrics_config",
    "ripgrep_invocation_monitor_config",
    "grep_fallback_monitor_config",
    "instant_grep_indexing_config",
    "git_diff_reply_limit_config",
    "renderer_slow_interaction_sentry_config",
    "glass_fps_monitor_config",
]

let IOS_EXPERIMENT_FALLBACKS: [String: [String: IOSExperimentConfigValue]] = [
    "sand_model_selection": [
        "enabled": .bool(false),
    ],
]

let IOS_DYNAMIC_CONFIG_FALLBACKS: [String: [String: IOSExperimentConfigValue]] = [
    "sand_process_metrics": [
        "local_enabled": .bool(false),
        "backend_reporting_enabled": .bool(false),
        "subsample_polling_rate_sec": .number(0),
        "sample_polling_rate_min": .number(0),
    ],
    "sand_rpc_tracing": [
        "enabled": .bool(false),
        "sample_ratio": .number(0.01),
    ],
    "sand_min_client_version": [
        "min_version": .string(""),
        "backend_min_version": .string(""),
    ],
    "sand_mobile_version_support": [
        "min_recommended_build": .number(0),
        "min_allowed_build": .number(0),
        "update_url": .string(""),
    ],
    "sand_computer_use_playwright_config": [
        "modelId": .string("claude-opus-4-8"),
        "maxMode": .bool(false),
        "parameters": .array([
            .object(["id": .string("thinking"), "value": .string("false")]),
            .object(["id": .string("effort"), "value": .string("low")]),
        ]),
    ],
    "sand_browser_use_model": [
        "modelId": .string("claude-opus-4-8"),
        "maxMode": .bool(false),
        "parameters": .array([
            .object(["id": .string("thinking"), "value": .string("false")]),
            .object(["id": .string("effort"), "value": .string("low")]),
        ]),
    ],
    "grok_bot_conversation_size_limits": [
        "soft_limit_mb": .number(256),
        "hard_limit_mb": .number(1024),
    ],
    "sand_model_filter": [
        "allowedModelIds": .array([]),
        "defaultParameters": .object([:]),
    ],
    "sand_default_model": [
        "modelId": .string("default"),
        "maxMode": .bool(false),
        "parameters": .array([]),
    ],
    "sand_automations_model": [
        "modelId": .string("default"),
        "maxMode": .bool(false),
        "parameters": .array([]),
    ],
    "sand_pressure_cpu_profiler_config": [
        "sustainedPressureWindowMs": .number(150_000),
        "profileDurationMs": .number(15_000),
        "minIntervalMs": .number(21_600_000),
        "maxRetainedProfiles": .number(3),
    ],
    "sand_internal_release_track_override": [
        "releaseTrack": .string(""),
        "unlockInternalTracks": .bool(false),
    ],
    "sandbox_default_network_allowlist": [
        "allowlist": .array([]),
    ],
]

func parseExperimentBoolean(_ value: IOSExperimentConfigValue) -> Bool? {
    value.boolValue
}

func parseExperimentString(_ value: IOSExperimentConfigValue) -> String? {
    value.stringValue
}

func parseExperimentNumber(_ value: IOSExperimentConfigValue) -> Double? {
    value.numberValue
}

func parseExperimentStringArray(_ value: IOSExperimentConfigValue) -> [String]? {
    guard case .array(let items) = value else { return nil }
    let values = items.compactMap(\.stringValue)
    return values.count == items.count ? values : nil
}

func parseExperimentNumberArray(_ value: IOSExperimentConfigValue) -> [Double]? {
    guard case .array(let items) = value else { return nil }
    let values = items.compactMap(\.numberValue)
    return values.count == items.count ? values : nil
}

func parseExperimentEnum(
    _ value: IOSExperimentConfigValue,
    allowed: Set<String>
) -> String? {
    guard let raw = value.stringValue, allowed.contains(raw) else { return nil }
    return raw
}
