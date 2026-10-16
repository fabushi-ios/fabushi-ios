import Foundation

func isMcpServerId(_ rawId: String) -> Bool {
    let id = rawId.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !id.isEmpty, id.first != "0" else { return false }
    return id.allSatisfy(\.isNumber)
}

func validateMcpServerId(_ rawId: String) throws -> String {
    let id = rawId.trimmingCharacters(in: .whitespacesAndNewlines)
    guard isMcpServerId(id) else {
        throw SandMcpConfigError("MCP server ID must be a positive decimal string.")
    }
    return id
}

func parseInt32McpServerId(_ rawId: String) throws -> Int32 {
    let id = try validateMcpServerId(rawId)
    guard let parsed = Int64(id), parsed <= Int64(Int32.max) else {
        throw SandMcpConfigError("MCP server ID is outside the supported range.")
    }
    return Int32(parsed)
}
