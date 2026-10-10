import Foundation

enum McpTransport: String, Equatable, Sendable {
    case stdio
    case sse
    case http
}

struct McpServerConfig: Equatable, Sendable {
    let transport: McpTransport
    var url: String? = nil
    var command: String? = nil
    var args: [String] = []
    var headers: [String: String] = [:]
    var env: [String: String] = [:]

    static func stdio(
        command: String,
        args: [String] = [],
        env: [String: String] = [:]
    ) -> Self {
        .init(transport: .stdio, command: command, args: args, env: env)
    }

    static func sse(url: String, headers: [String: String] = [:]) -> Self {
        .init(transport: .sse, url: url, headers: headers)
    }

    static func http(url: String, headers: [String: String] = [:]) -> Self {
        .init(transport: .http, url: url, headers: headers)
    }
}

struct McpDisplayAccountSlot: Equatable, Sendable {
    let accountKey: String
    let hasToken: Bool
    var serverIdentifier: String? = nil
}

struct DisplayServer: Equatable, Sendable {
    let id: String
    let name: String
    var serverIdentifier: String? = nil
    let config: McpServerConfig
    let isTeamServer: Bool
    var disabledByTeamAdminPolicy: Bool = false
    var pluginId: String? = nil
    var isRequired: Bool = false
    var managedByTeamPluginPolicy: Bool = false
    var accounts: [McpDisplayAccountSlot] = []
}

struct AccountDisplayConfig: Equatable, Sendable {
    let servers: [DisplayServer]
    var cacheScope: String? = nil
    var unavailable: Bool = false
    var unresolvedServerIds: [String] = []
}

struct McpRuntimeConfig: Equatable, Sendable {
    var mcpServers: [String: McpServerConfig]
}

let EMPTY_MCP_CONFIG = McpRuntimeConfig(mcpServers: [:])

func normalizeAccountKey(_ raw: String) throws -> String {
    let key = normalizeMcpAccountLabel(raw)
    guard !key.isEmpty else { throw SandMcpConfigError("MCP account label is required.") }
    return key
}

func runtimeConfigFromDisplay(_ display: AccountDisplayConfig?) -> McpRuntimeConfig? {
    guard let display else { return nil }
    var servers: [String: McpServerConfig] = [:]
    for server in display.servers {
        guard let identifier = server.serverIdentifier,
              !server.disabledByTeamAdminPolicy else { continue }
        servers[identifier] = server.config
    }
    return .init(mcpServers: servers)
}

func validateMarketplacePluginId(_ raw: String) throws -> String {
    let value = raw.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !value.isEmpty, value.allSatisfy(\.isNumber) else {
        throw SandMcpConfigError("Invalid marketplace plugin id \"\(raw)\".")
    }
    return value
}

struct McpPluginAttribution: Equatable, Sendable {
    var pluginId: String? = nil
    var isRequired = false
    var managedByTeamPluginPolicy = false
}

func pluginAttributionFromDisplayServer(_ server: DisplayServer) -> McpPluginAttribution {
    .init(
        pluginId: server.pluginId,
        isRequired: server.isRequired,
        managedByTeamPluginPolicy: server.managedByTeamPluginPolicy
    )
}

struct McpServerStatus: Equatable, Sendable {
    let status: String
    var statusDetail: String? = nil
}

func statusFromBackendListStatus(_ status: String?) -> McpServerStatus {
    switch status {
    case "connected": .init(status: "connected")
    case "needsAuth": .init(status: "needsAuth", statusDetail: "Authentication required")
    case "loading": .init(status: "initializing")
    case "error": .init(status: "error", statusDetail: "Failed to load MCP server")
    default: .init(status: "error", statusDetail: "Not reported by backend")
    }
}

func statusFromBoxListStatus(
    _ status: String?,
    unavailable: Bool = false,
    detail: String? = nil
) -> McpServerStatus {
    switch status {
    case "connected": .init(status: "connected")
    case "needsAuth": .init(status: "needsAuth", statusDetail: "Authentication required")
    case "loading": .init(status: "initializing")
    case "error":
        .init(
            status: "error",
            statusDetail: detail?.isEmpty == false ? detail : "Failed to load MCP server"
        )
    default:
        .init(
            status: "error",
            statusDetail: unavailable
                ? "Fabushi's computer unreachable"
                : "Not reported by Fabushi's computer"
        )
    }
}
