pub const ACTION_AUDIT: &str = "action-audit";
pub const ATTACHMENTS: &str = "attachments";
pub const AUTH: &str = "auth";
pub const AUTO_REVIEW: &str = "auto-review";
pub const AUTOMATIONS: &str = "automations";
pub const BOX_LIFECYCLE: &str = "box-lifecycle";
pub const BOX_STORE_SYNC: &str = "box-store-sync";
pub const BROWSER_UA: &str = "browser-ua";
pub const CLOUD_AGENTS: &str = "cloud-agents";
pub const CODEBASE_TELEMETRY: &str = "codebase-telemetry";
pub const CONTENT_SEARCH: &str = "content-search";
pub const CROSS_USER_SHARING: &str = "cross-user-sharing";
pub const EXPERIMENTS: &str = "experiments";
pub const FOREVER_BOX: &str = "forever-box";
pub const HOST_UPGRADE: &str = "host-upgrade";
pub const INFERENCE: &str = "inference";
pub const LOCAL_EXEC: &str = "local-exec";
pub const LOCAL_TOOL_PERMISSION: &str = "local-tool-permission";
pub const MANAGED_SETUP: &str = "managed-setup";
pub const MCP: &str = "mcp";
pub const MEMORY: &str = "memory";
pub const NOTIFICATIONS: &str = "notifications";
pub const NOTIFY_BUS: &str = "notify-bus";
pub const SECRETS: &str = "secrets";
pub const SESSION: &str = "session";
pub const SETTINGS: &str = "settings";
pub const SOURCE_MAP: &str = "source-map";
pub const STATE_BACKSTOP: &str = "state-backstop";
pub const TEACH_RECORDING: &str = "teach-recording";
pub const TELEMETRY: &str = "telemetry";
pub const TRANSCRIPT: &str = "transcript";
pub const TRAYS: &str = "trays";
pub const TURN_EXECUTION: &str = "turn-execution";
pub const WALLPAPER: &str = "wallpaper";
pub const WEBAUTHN_PROXY: &str = "webauthn-proxy";

pub const HOST_EXTENSION_IDS: [&str; 35] = [
    ACTION_AUDIT, ATTACHMENTS, AUTH, AUTO_REVIEW, AUTOMATIONS, BOX_LIFECYCLE,
    BOX_STORE_SYNC, BROWSER_UA, CLOUD_AGENTS, CODEBASE_TELEMETRY, CONTENT_SEARCH,
    CROSS_USER_SHARING, EXPERIMENTS, FOREVER_BOX, HOST_UPGRADE, INFERENCE,
    LOCAL_EXEC, LOCAL_TOOL_PERMISSION, MANAGED_SETUP, MCP, MEMORY, NOTIFICATIONS,
    NOTIFY_BUS, SECRETS, SESSION, SETTINGS, SOURCE_MAP, STATE_BACKSTOP,
    TEACH_RECORDING, TELEMETRY, TRANSCRIPT, TRAYS, TURN_EXECUTION, WALLPAPER,
    WEBAUTHN_PROXY,
];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_extension_ids_match_reference_inventory() {
        assert_eq!(HOST_EXTENSION_IDS.len(), 35);
        assert!(HOST_EXTENSION_IDS.contains(&MCP));
        assert!(HOST_EXTENSION_IDS.contains(&WEBAUTHN_PROXY));
    }
}
