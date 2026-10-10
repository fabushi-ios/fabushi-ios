import Foundation

struct RoutedMCPTool: Equatable, Sendable {
    let name: String
    let providerIdentifier: String
    let toolName: String
    let description: String?
    let inputSchema: CoordinatorPayload?
}

actor RoutedMCPBridge {
    typealias ListTools = @Sendable () async throws -> [RoutedMCPTool]
    typealias CallTool = @Sendable (_ tool: RoutedMCPTool, _ arguments: CoordinatorPayload, _ toolCallId: String) async throws -> CoordinatorPayload

    private let listToolsImpl: ListTools
    private let callToolImpl: CallTool
    private var cachedTools: [String: RoutedMCPTool] = [:]

    init(listTools: @escaping ListTools, callTool: @escaping CallTool) {
        listToolsImpl = listTools
        callToolImpl = callTool
    }

    func listTools() async throws -> [RoutedMCPTool] {
        let tools = try await listToolsImpl()
        cachedTools = Dictionary(uniqueKeysWithValues: tools.map { ($0.name, $0) })
        return tools
    }

    func callTool(name: String, arguments: CoordinatorPayload) async throws -> CoordinatorPayload {
        let tool: RoutedMCPTool
        if let cached = cachedTools[name] {
            tool = cached
        } else {
            _ = try await listTools()
            guard let refreshed = cachedTools[name] else {
                throw ControlPortCallError(code: "unknown-mcp-tool", message: "Unknown routed MCP tool: \(name)")
            }
            tool = refreshed
        }
        return try await callToolImpl(tool, arguments, UUID().uuidString.lowercased())
    }
}
