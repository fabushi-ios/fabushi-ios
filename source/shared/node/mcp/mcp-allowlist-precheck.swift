import Foundation

struct McpAllowlistPrecheckDecision: Equatable, Sendable {
    let allowed: Bool
    let reason: String

    static let allowedDecision = Self(allowed: true, reason: "allowed")

    static func denied(_ reason: String) -> Self {
        .init(allowed: false, reason: reason)
    }
}

func precheckMcpToolAllowlist(
    request: McpToolExecutionRequest,
    display: AccountDisplayConfig,
    discoveredTools: [McpDiscoveredTool],
    disabledToolsByServerId: [String: [String]]
) -> McpAllowlistPrecheckDecision {
    let providerIdentifier = request.providerIdentifier
        .trimmingCharacters(in: .whitespacesAndNewlines)
    let toolName = request.name.trimmingCharacters(in: .whitespacesAndNewlines)

    guard !providerIdentifier.isEmpty, !toolName.isEmpty else {
        return .denied("MCP server and tool identifiers are required.")
    }
    guard !display.unavailable, display.cacheScope?.isEmpty == false else {
        return .denied("MCP allowlist is unavailable for the current account scope.")
    }
    guard let server = display.servers.first(where: { row in
        guard let rowIdentifier = row.serverIdentifier else { return false }
        return displayRowOwnsIdentifier(
            providerIdentifier,
            rowIdentifier: rowIdentifier,
            slots: row.accounts
        )
    }) else {
        return .denied(
            "MCP server \"\(providerIdentifier)\" is not in the current account allowlist."
        )
    }
    guard !server.disabledByTeamAdminPolicy else {
        return .denied(
            "MCP server \"\(server.name)\" is disabled by team policy."
        )
    }

    let disabled = Set(disabledToolsByServerId[server.id] ?? [])
    guard !disabled.contains(toolName) else {
        return .denied(
            "Tool \"\(toolName)\" is disabled for \"\(server.name)\"."
        )
    }

    let advertised = discoveredTools.contains { tool in
        tool.providerIdentifier == providerIdentifier
            && (tool.name == toolName || tool.toolName == toolName)
    }
    guard advertised else {
        if server.config.transport == .stdio {
            return .denied(
                "Remote Runner did not advertise MCP tool \"\(toolName)\" for \"\(server.name)\"."
            )
        }
        return .denied(
            "MCP tool \"\(toolName)\" is not in the current allowlist for \"\(server.name)\"."
        )
    }

    return .allowedDecision
}
