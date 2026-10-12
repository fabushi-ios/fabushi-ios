import Foundation

private let RESERVED_MCP_SERVER_NAMES: Set<String> = ["__proto__", "constructor", "prototype"]

func getTransport(_ config: McpServerConfig) -> McpTransport {
    config.transport
}

func getCommand(_ config: McpServerConfig) -> String? {
    guard config.transport == .stdio, let command = config.command else { return nil }
    return ([command] + config.args).joined(separator: " ")
}

func validateServerName(_ raw: String) throws -> String {
    let name = raw.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !name.isEmpty else { throw SandMcpConfigError("MCP server name is required.") }
    guard !RESERVED_MCP_SERVER_NAMES.contains(name) else {
        throw SandMcpConfigError("MCP server name \"\(name)\" is reserved.")
    }
    guard !name.contains("/"), !name.contains("\\"), !name.contains("\0") else {
        throw SandMcpConfigError("MCP server names cannot include slashes or null bytes.")
    }
    guard !name.contains("--") else {
        throw SandMcpConfigError("MCP server names cannot include \"--\".")
    }
    return name
}

func parseServerConfig(
    _ configJson: String,
    parse: (Any) throws -> McpServerConfig
) throws -> McpServerConfig {
    guard let data = configJson.data(using: .utf8) else {
        throw SandMcpConfigError("MCP server configuration is not valid UTF-8 JSON.")
    }
    return try parse(JSONSerialization.jsonObject(with: data))
}

protocol McpJsonConvertible {
    func toJson() -> Any
}

func toJsonArgs(_ args: [String: Any]) -> [String: Any] {
    args.mapValues { value in
        (value as? any McpJsonConvertible)?.toJson() ?? value
    }
}
