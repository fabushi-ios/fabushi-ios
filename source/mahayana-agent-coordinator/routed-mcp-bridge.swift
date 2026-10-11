import Foundation

struct RoutedMCPTool: Equatable, Sendable {
    let name: String
    let providerIdentifier: String
    let toolName: String
    let description: String?
    let inputSchema: CoordinatorPayload?
}

struct RoutedMCPScopeToken: Equatable, Sendable {
    let accountScope: String
    let generation: UInt64
}

struct RoutedMCPBridgeError: Error, LocalizedError, Equatable, Sendable {
    let code: String
    let message: String
    var errorDescription: String? { message }
}

actor RoutedMCPBridge {
    typealias ListTools = @Sendable () async throws -> [RoutedMCPTool]
    typealias CallTool = @Sendable (
        _ tool: RoutedMCPTool,
        _ arguments: CoordinatorPayload,
        _ toolCallId: String,
        _ agentId: String?
    ) async throws -> CoordinatorPayload
    typealias LegacyCallTool = @Sendable (
        _ tool: RoutedMCPTool,
        _ arguments: CoordinatorPayload,
        _ toolCallId: String
    ) async throws -> CoordinatorPayload
    typealias CurrentScope = @Sendable () async -> RoutedMCPScopeToken?

    private let listToolsImpl: ListTools
    private let callToolImpl: CallTool
    private let currentScopeImpl: CurrentScope?
    private var cachedTools: [String: RoutedMCPTool] = [:]
    private var cachedScope: RoutedMCPScopeToken?
    private var inFlight: [String: Task<CoordinatorPayload, Error>] = [:]
    private var inFlightRequestIds: [String: String] = [:]

    init(
        listTools: @escaping ListTools,
        currentScope: @escaping CurrentScope,
        callTool: @escaping CallTool
    ) {
        listToolsImpl = listTools
        currentScopeImpl = currentScope
        callToolImpl = callTool
    }

    init(
        listTools: @escaping ListTools,
        callTool: @escaping LegacyCallTool
    ) {
        listToolsImpl = listTools
        currentScopeImpl = nil
        callToolImpl = { tool, arguments, toolCallId, _ in
            try await callTool(tool, arguments, toolCallId)
        }
    }

    func listTools() async throws -> [RoutedMCPTool] {
        let tools = try await listToolsImpl()
        cachedTools = Dictionary(uniqueKeysWithValues: tools.map { ($0.name, $0) })
        cachedScope = nil
        return tools
    }

    func listTools(scope: RoutedMCPScopeToken) async throws -> [RoutedMCPTool] {
        try await ensureCurrent(scope)
        let tools = try await listToolsImpl()
        try Task.checkCancellation()
        try await ensureCurrent(scope)
        cachedTools = Dictionary(uniqueKeysWithValues: tools.map { ($0.name, $0) })
        cachedScope = scope
        return tools
    }

    func callTool(name: String, arguments: CoordinatorPayload) async throws -> CoordinatorPayload {
        let tool = try await resolveLegacyTool(name)
        return try await callToolImpl(
            tool,
            arguments,
            UUID().uuidString.lowercased(),
            nil
        )
    }

    func callTool(
        name: String,
        arguments: CoordinatorPayload,
        requestId: String,
        toolCallId: String,
        agentId: String?,
        scope: RoutedMCPScopeToken
    ) async throws -> CoordinatorPayload {
        let normalizedRequestId = requestId.trimmingCharacters(in: .whitespacesAndNewlines)
        let normalizedCallId = toolCallId.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !normalizedRequestId.isEmpty, normalizedRequestId.utf8.count <= 256 else {
            throw RoutedMCPBridgeError(
                code: "RUNNER_TOOL_RELAY_PROTOCOL_ERROR",
                message: "routed MCP request requires requestId of at most 256 bytes"
            )
        }
        guard !normalizedCallId.isEmpty, normalizedCallId.utf8.count <= 256 else {
            throw RoutedMCPBridgeError(
                code: "MCP_BRIDGE_INVALID_CALL_ID",
                message: "routed MCP call requires toolCallId of at most 256 bytes"
            )
        }

        try Task.checkCancellation()
        try await ensureCurrent(scope)
        let tool = try await resolveTool(name, scope: scope)
        let key = "\(scope.accountScope):\(scope.generation):\(normalizedCallId)"
        if let existing = inFlight[key] {
            guard inFlightRequestIds[key] == normalizedRequestId else {
                throw RoutedMCPBridgeError(
                    code: "MCP_BRIDGE_CALL_ID_REUSED",
                    message: "routed MCP toolCallId is already owned by another request"
                )
            }
            return try await existing.value
        }

        let task = Task<CoordinatorPayload, Error> {
            try Task.checkCancellation()
            try await self.ensureCurrent(scope)
            let value = try await self.callToolImpl(
                tool,
                arguments,
                normalizedCallId,
                agentId
            )
            try Task.checkCancellation()
            try await self.ensureCurrent(scope)
            return value
        }
        inFlight[key] = task
        inFlightRequestIds[key] = normalizedRequestId
        defer {
            inFlight.removeValue(forKey: key)
            inFlightRequestIds.removeValue(forKey: key)
        }

        return try await withTaskCancellationHandler(
            operation: { try await task.value },
            onCancel: { task.cancel() }
        )
    }

    private func resolveLegacyTool(_ name: String) async throws -> RoutedMCPTool {
        if let cached = cachedTools[name] {
            return cached
        }
        _ = try await listTools()
        guard let refreshed = cachedTools[name] else {
            throw ControlPortCallError(
                code: "unknown-mcp-tool",
                message: "Unknown routed MCP tool: \(name)"
            )
        }
        return refreshed
    }

    private func resolveTool(
        _ name: String,
        scope: RoutedMCPScopeToken
    ) async throws -> RoutedMCPTool {
        if cachedScope == scope, let cached = cachedTools[name] {
            return cached
        }
        _ = try await listTools(scope: scope)
        guard let refreshed = cachedTools[name] else {
            throw ControlPortCallError(
                code: "unknown-mcp-tool",
                message: "Unknown routed MCP tool: \(name)"
            )
        }
        return refreshed
    }

    private func ensureCurrent(_ expected: RoutedMCPScopeToken) async throws {
        guard let currentScopeImpl else {
            throw RoutedMCPBridgeError(
                code: "MCP_BRIDGE_SCOPE_UNAVAILABLE",
                message: "routed MCP scope fencing is unavailable"
            )
        }
        guard let current = await currentScopeImpl(), current == expected else {
            throw RoutedMCPBridgeError(
                code: "MCP_BRIDGE_STALE_SCOPE",
                message: "routed MCP result became stale because the account/session scope changed"
            )
        }
    }
}
