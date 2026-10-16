use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpOAuthProviderPolicy {
    pub provider: &'static str,
    pub client_registration: &'static str,
    pub unauthenticated_connect: bool,
    pub rejects_custom_scheme_redirects: bool,
    pub authorization_access_type: &'static str,
    pub authorization_prompt: &'static str,
    pub scopes: &'static [&'static str],
}

const fn policy(scopes: &'static [&'static str]) -> McpOAuthProviderPolicy {
    McpOAuthProviderPolicy {
        provider: "google-workspace",
        client_registration: "static",
        unauthenticated_connect: true,
        rejects_custom_scheme_redirects: true,
        authorization_access_type: "offline",
        authorization_prompt: "consent",
        scopes,
    }
}

pub fn mcp_oauth_provider_policies() -> BTreeMap<&'static str, McpOAuthProviderPolicy> {
    BTreeMap::from([
        ("gmailmcp.googleapis.com", policy(&[
            "https://www.googleapis.com/auth/gmail.readonly",
            "https://www.googleapis.com/auth/gmail.compose",
            "https://www.googleapis.com/auth/gmail.modify",
        ])),
        ("drivemcp.googleapis.com", policy(&[
            "https://www.googleapis.com/auth/drive.readonly",
            "https://www.googleapis.com/auth/drive.file",
        ])),
        ("calendarmcp.googleapis.com", policy(&[
            "https://www.googleapis.com/auth/calendar.events",
            "https://www.googleapis.com/auth/calendar.calendarlist.readonly",
            "https://www.googleapis.com/auth/calendar.events.readonly",
            "https://www.googleapis.com/auth/calendar.events.freebusy",
        ])),
        ("docsmcp.googleapis.com", policy(&["https://www.googleapis.com/auth/documents"])),
        ("sheetsmcp.googleapis.com", policy(&["https://www.googleapis.com/auth/spreadsheets"])),
        ("slidesmcp.googleapis.com", policy(&["https://www.googleapis.com/auth/presentations"])),
    ])
}

pub fn google_workspace_mcp_hosts() -> BTreeSet<&'static str> {
    mcp_oauth_provider_policies().keys().copied().collect()
}

pub const MCP_OAUTH_EXTENSION_ID: &str = "anysphere.cursor-mcp";
pub const MCP_OAUTH_RETURN_PATH: &str = "/oauth/return";
pub const MCP_OAUTH_DESKTOP_RETURN_URL: &str = "cursor://anysphere.cursor-mcp/oauth/return";
pub const MCP_OAUTH_LOOPBACK_CALLBACK_URL: &str = "http://localhost:8787/callback";

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_google_workspace_oauth_policy_contract() {
        let policies=mcp_oauth_provider_policies();
        assert_eq!(policies.len(), 6);
        let gmail=&policies["gmailmcp.googleapis.com"];
        assert!(gmail.unauthenticated_connect);
        assert!(gmail.rejects_custom_scheme_redirects);
        assert_eq!(gmail.authorization_access_type, "offline");
        assert_eq!(gmail.authorization_prompt, "consent");
        assert_eq!(gmail.scopes.len(), 3);
        assert_eq!(google_workspace_mcp_hosts().len(), 6);
        assert_eq!(MCP_OAUTH_DESKTOP_RETURN_URL, format!("cursor://{MCP_OAUTH_EXTENSION_ID}{MCP_OAUTH_RETURN_PATH}"));
    }
}
