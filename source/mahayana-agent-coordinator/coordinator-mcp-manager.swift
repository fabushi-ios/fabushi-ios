import Foundation

private struct CoordinatorMcpSnapshot: Sendable {
    let display: AccountDisplayConfig
    let backendServers: [BackendMcpToolServerWire]
    let disabledToolsByServerId: [String: [String]]
    let scopeGeneration: UInt64
}

@MainActor
private final class CoordinatorMcpHostPort {
    private let hostSupervisor: MahayanaLocalHostSupervisor
    private let settingsStore: SandSettingsStore
    private var accountScope: String?
    private var scopeGeneration: UInt64 = 0
    private var lastSnapshot: CoordinatorMcpSnapshot?

    init(
        hostSupervisor: MahayanaLocalHostSupervisor,
        settingsStore: SandSettingsStore
    ) {
        self.hostSupervisor = hostSupervisor
        self.settingsStore = settingsStore
    }

    func updateAccountScope(_ rawScope: String?) {
        let trimmed = rawScope?.trimmingCharacters(in: .whitespacesAndNewlines)
        let next = trimmed?.isEmpty == false ? trimmed : nil
        guard next != accountScope else { return }
        accountScope = next
        scopeGeneration = scopeGeneration == UInt64.max ? 1 : scopeGeneration + 1
        lastSnapshot = nil
    }

    func loadAccountDisplay(forceFresh: Bool) async throws -> AccountDisplayConfig {
        let snapshot = try await loadSnapshot(forceFresh: forceFresh)
        settingsStore.setMcpDisabledToolsByServerId(snapshot.disabledToolsByServerId)
        return snapshot.display
    }

    func listBackendServers(
        serverIdentifiers: [String]
    ) async throws -> [BackendMcpToolServerWire] {
        let requested = Set(serverIdentifiers)
        return try await loadSnapshot(forceFresh: false).backendServers.filter {
            requested.contains($0.serverIdentifier)
        }
    }

    func setToolDisabled(
        serverIdentifier: String,
        toolName: String,
        disabled: Bool
    ) async throws {
        guard let scope = accountScope else {
            throw SandMcpConfigError("MCP tools require an authenticated account.")
        }
        let generation = scopeGeneration
        _ = try await hostSupervisor.request(
            method: "feature.mcp.setToolDisabled",
            params: [
                "server": serverIdentifier,
                "tool": toolName,
                "disabled": disabled,
            ]
        )
        guard generation == scopeGeneration, scope == accountScope else {
            throw SandMcpConfigError(
                "MCP tool update became stale because the account changed."
            )
        }
        lastSnapshot = nil
    }

    func executeTool(
        serverIdentifier: String,
        toolName: String,
        args: McpJSONValue
    ) async throws -> McpJSONValue {
        guard let scope = accountScope else {
            throw SandMcpConfigError("MCP tools require an authenticated account.")
        }
        let generation = scopeGeneration
        let response = try await hostSupervisor.request(
            method: "feature.mcp.toolCall",
            params: [
                "server": serverIdentifier,
                "tool": toolName,
                "arguments": foundationValue(args),
            ]
        )
        guard generation == scopeGeneration, scope == accountScope else {
            throw SandMcpConfigError(
                "MCP tool result became stale because the account changed."
            )
        }
        return McpJSONValue.from(response.value)
    }

    private func loadSnapshot(forceFresh: Bool) async throws -> CoordinatorMcpSnapshot {
        guard let scope = accountScope else {
            let empty = CoordinatorMcpSnapshot(
                display: .init(servers: [], cacheScope: nil),
                backendServers: [],
                disabledToolsByServerId: [:],
                scopeGeneration: scopeGeneration
            )
            lastSnapshot = empty
            return empty
        }
        if !forceFresh,
           let lastSnapshot,
           lastSnapshot.scopeGeneration == scopeGeneration {
            return lastSnapshot
        }

        let generation = scopeGeneration
        let response = try await hostSupervisor.request(
            method: "feature.mcp.servers",
            params: [:]
        )
        guard generation == scopeGeneration, scope == accountScope else {
            throw SandMcpConfigError(
                "MCP server snapshot became stale because the account changed."
            )
        }
        guard let object = response.value as? [String: Any],
              let rows = object["servers"] as? [[String: Any]]
        else {
            throw SandMcpConfigError("MCP Host returned an invalid server snapshot.")
        }

        let names = rows.compactMap { row -> String? in
            guard let raw = row["name"] as? String else { return nil }
            let name = raw.trimmingCharacters(in: .whitespacesAndNewlines)
            return name.isEmpty ? nil : name
        }
        let ids = stableRowIDs(names)
        var displayServers: [DisplayServer] = []
        var backendServers: [BackendMcpToolServerWire] = []
        var disabledByServerId: [String: [String]] = [:]

        for row in rows {
            guard let rawName = row["name"] as? String else { continue }
            let name = rawName.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !name.isEmpty,
                  let rowID = ids[name],
                  let wireConfig = row["fabushiConfig"] as? [String: Any]
            else { continue }

            let config: McpServerConfig
            if let command = wireConfig["command"] as? String, !command.isEmpty {
                config = .stdio(
                    command: command,
                    args: stringArray(wireConfig["args"]),
                    env: stringDictionary(wireConfig["env"])
                )
            } else if let url = wireConfig["url"] as? String, !url.isEmpty {
                config = .http(
                    url: url,
                    headers: stringDictionary(wireConfig["http_headers"])
                )
            } else {
                continue
            }

            let disabled = stringArray(wireConfig["disabled_tools"]).sorted()
            disabledByServerId[rowID] = disabled
            displayServers.append(.init(
                id: rowID,
                name: name,
                serverIdentifier: name,
                config: config,
                isTeamServer: false
            ))

            let authStatus = row["authStatus"] as? String
            let status = authStatus == "notLoggedIn" ? "needsAuth" : "connected"
            let toolObject = row["tools"] as? [String: Any] ?? [:]
            let toolNames = Set(toolObject.keys).union(disabled)
            let tools = toolNames.sorted().compactMap { toolName -> BackendToolWire? in
                let detail = toolObject[toolName] as? [String: Any]
                let canonical = (detail?["name"] as? String)?
                    .trimmingCharacters(in: .whitespacesAndNewlines)
                let resolved = canonical?.isEmpty == false ? canonical! : toolName
                guard !resolved.isEmpty else { return nil }
                return .init(
                    name: "\(name).\(resolved)",
                    providerIdentifier: name,
                    toolName: resolved,
                    description: detail?["description"] as? String ?? "",
                    inputSchema: detail?["inputSchema"].map(McpJSONValue.from)
                )
            }
            backendServers.append(.init(
                serverIdentifier: name,
                status: status,
                tools: tools,
                accountLabel: DEFAULT_MCP_ACCOUNT_KEY,
                rowServerIdentifier: name
            ))
        }

        let snapshot = CoordinatorMcpSnapshot(
            display: .init(
                servers: displayServers.sorted { $0.name < $1.name },
                cacheScope: scope
            ),
            backendServers: backendServers,
            disabledToolsByServerId: disabledByServerId,
            scopeGeneration: generation
        )
        guard generation == scopeGeneration, scope == accountScope else {
            throw SandMcpConfigError(
                "MCP server snapshot became stale because the account changed."
            )
        }
        lastSnapshot = snapshot
        return snapshot
    }

    private func stableRowIDs(_ names: [String]) -> [String: String] {
        var result: [String: String] = [:]
        var used = Set<UInt32>()
        for name in Set(names).sorted() {
            var hash: UInt32 = 2_166_136_261
            for byte in name.utf8 {
                hash ^= UInt32(byte)
                hash &*= 16_777_619
            }
            var candidate = hash & 0x7fff_ffff
            if candidate == 0 { candidate = 1 }
            while used.contains(candidate) {
                candidate = candidate == 0x7fff_ffff ? 1 : candidate + 1
            }
            used.insert(candidate)
            result[name] = String(candidate)
        }
        return result
    }

    private func stringArray(_ value: Any?) -> [String] {
        (value as? [Any] ?? []).compactMap { $0 as? String }
    }

    private func stringDictionary(_ value: Any?) -> [String: String] {
        guard let object = value as? [String: Any] else { return [:] }
        return object.reduce(into: [:]) { result, entry in
            if let value = entry.value as? String {
                result[entry.key] = value
            }
        }
    }

    private func foundationValue(_ value: McpJSONValue) -> Any {
        switch value {
        case .null: NSNull()
        case .bool(let value): value
        case .number(let value): value
        case .string(let value): value
        case .array(let values): values.map(foundationValue)
        case .object(let object): object.mapValues(foundationValue)
        }
    }
}

private final class CoordinatorMcpBackendClient: DashboardMcpExecClient, @unchecked Sendable {
    private let port: CoordinatorMcpHostPort

    init(port: CoordinatorMcpHostPort) {
        self.port = port
    }

    func listSandMcpTools(
        serverIdentifiers: [String],
        timeoutMs: Int
    ) async throws -> [BackendMcpToolServerWire] {
        try await port.listBackendServers(serverIdentifiers: serverIdentifiers)
    }

    func executeSandMcpTool(
        serverIdentifier: String,
        toolName: String,
        args: McpJSONValue,
        toolCallId: String,
        agentId: String,
        timeoutMs: Int
    ) async throws -> McpExecResult? {
        .init(
            caseName: "success",
            value: try await port.executeTool(
                serverIdentifier: serverIdentifier,
                toolName: toolName,
                args: args
            )
        )
    }

    func checkHttpMcpStatus(
        serverIds: [String],
        oauthRedirectUri: String,
        forceReauth: Bool,
        accountKey: String,
        timeoutMs: Int
    ) async throws -> [BackendMcpAuthStatusWire] {
        throw SandBackendMcpExecError(
            message: "MCP account authentication is not wired to the iOS Marketplace yet."
        )
    }

    func completeMcpOAuth(
        stateId: String,
        authorizationCode: String,
        timeoutMs: Int
    ) async throws {
        throw SandBackendMcpExecError(
            message: "MCP account authentication is not wired to the iOS Marketplace yet."
        )
    }

    func validateMcpOAuthTokens(
        targets: [BackendMcpTokenTarget],
        timeoutMs: Int
    ) async throws -> [BackendMcpTokenValidation] {
        []
    }

    func deleteMcpOAuthToken(
        serverUrl: String,
        accountKey: String,
        source: String,
        timeoutMs: Int
    ) async throws {
        throw SandBackendMcpExecError(
            message: "MCP account management is not wired to the iOS Marketplace yet."
        )
    }

    func renameMcpOAuthAccount(
        serverId: String,
        accountKey: String,
        newAccountKey: String,
        timeoutMs: Int
    ) async throws {
        throw SandBackendMcpExecError(
            message: "MCP account management is not wired to the iOS Marketplace yet."
        )
    }

    func deleteMcpOAuthAccount(
        serverId: String,
        accountKey: String,
        timeoutMs: Int
    ) async throws {
        throw SandBackendMcpExecError(
            message: "MCP account management is not wired to the iOS Marketplace yet."
        )
    }
}

@MainActor
final class CoordinatorMcpSurface {
    private let manager: SandMcpManager
    private let port: CoordinatorMcpHostPort

    private init(manager: SandMcpManager, port: CoordinatorMcpHostPort) {
        self.manager = manager
        self.port = port
    }

    static func make(
        hostSupervisor: MahayanaLocalHostSupervisor,
        settingsStore: SandSettingsStore
    ) -> CoordinatorMcpSurface {
        let port = CoordinatorMcpHostPort(
            hostSupervisor: hostSupervisor,
            settingsStore: settingsStore
        )
        let client = CoordinatorMcpBackendClient(port: port)
        let backend = DashboardSandBackendMcpExec(deps: .init(client: client))
        let definitionSource = SandMcpDefinitionSource(includeBuiltins: false)
        let discovery = SandMcpToolsDiscovery(deps: .init(
            definitionSource: definitionSource,
            settingsStore: settingsStore,
            backendListTools: { names in
                try await backend.listTools(serverIdentifiers: names)
            },
            backendExecuteTool: { identifier, tool, args, callId, agentId in
                sandMcpResultFromBackend(
                    await backend.executeTool(
                        serverIdentifier: identifier,
                        toolName: tool,
                        args: args,
                        toolCallId: callId,
                        agentId: agentId
                    )
                )
            }
        ))
        var dependencies = SandMcpManagerDependencies(
            settingsStore: settingsStore,
            backendMcpExec: backend,
            definitionSource: definitionSource,
            toolsDiscovery: discovery,
            accountDisplayConfigProvider: { _ in
                try await port.loadAccountDisplay(forceFresh: true)
            },
            autoPollEnabled: false
        )
        dependencies.setToolDisabled = { identifier, tool, disabled in
            try await port.setToolDisabled(
                serverIdentifier: identifier,
                toolName: tool,
                disabled: disabled
            )
        }
        return .init(
            manager: SandMcpManager(deps: dependencies),
            port: port
        )
    }

    func updateAccountScope(_ scope: String?) {
        port.updateAccountScope(scope)
    }

    func route(
        method: String,
        params: [String: Any]
    ) async throws -> CoordinatorDevControlRouting {
        switch method {
        case "coordinator.mcp.servers":
            let state = try await manager.listServers()
            return .handled(["servers": state.servers.map(projectServer)])

        case "coordinator.mcp.tools":
            guard let serverId = nonEmptyString(params["serverId"]) else {
                throw SandMcpConfigError("MCP server id is required.")
            }
            let tools = try await manager.listServerTools(serverId)
            return .handled([
                "serverId": serverId,
                "tools": tools.map(projectTool),
            ])

        case "coordinator.mcp.setToolDisabled":
            guard let serverId = nonEmptyString(params["serverId"]),
                  let toolName = nonEmptyString(params["tool"]),
                  let disabled = params["disabled"] as? Bool
            else {
                throw SandMcpConfigError(
                    "MCP server id, tool name, and disabled state are required."
                )
            }
            let tools = try await manager.setMcpToolDisabled(
                serverId: serverId,
                toolName: toolName,
                disabled: disabled
            )
            return .handled([
                "serverId": serverId,
                "tools": tools.map(projectTool),
            ])

        default:
            return .notHandled
        }
    }

    private func projectServer(_ server: McpServerSummary) -> [String: Any] {
        [
            "id": server.id,
            "name": server.name,
            "serverIdentifier": server.serverIdentifier,
            "transport": server.transport.rawValue,
            "toolCount": server.toolCount,
            "disabledToolCount": server.disabledToolCount,
            "status": server.status,
            "statusDetail": server.statusDetail ?? NSNull(),
        ]
    }

    private func projectTool(_ tool: McpToolListing) -> [String: Any] {
        [
            "name": tool.name,
            "title": tool.title ?? NSNull(),
            "description": tool.description ?? NSNull(),
            "isDisabled": tool.isDisabled,
        ]
    }

    private func nonEmptyString(_ value: Any?) -> String? {
        guard let value = value as? String else { return nil }
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}
