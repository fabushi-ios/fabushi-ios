import Foundation

enum CoordinatorMainMethodRegistry {
    static let methods: Set<String> = [
        "uploadAttachment", "readAttachmentImage", "readAttachmentText", "readAttachmentChunk",
        "fetchLinkMetadata", "getHostSettings", "setHostSettings", "setBoxSecrets", "refreshMcp",
        "reportConnectorAuth", "reportMcpDiscoveryFailed", "loadBoxMcpServers", "listBoxMcpServers",
        "listBoxMcpToolsRaw", "executeBoxMcpToolRaw", "updateForeverBox", "setWindowFocused",
        "getHostStatus", "listAgents", "createAgent",
        "deleteAgents", "getConversationOutline", "getSubagents", "setDevGatewayOffline",
        "setGatewayPaused"
    ]

    static func contains(_ method: String) -> Bool {
        methods.contains(method)
    }
}
