
private func accountDisplayConfig(from fetched: AccountMcpFetchResult) -> AccountDisplayConfig {
    let servers = fetched.servers.map { server -> DisplayServer in
        let config: McpServerConfig
        switch server.config {
        case .stdio(let value):
            config = .stdio(
                command: value.command,
                args: value.args,
                env: value.env ?? [:]
            )
        case .remote(let value):
            if value.type?.lowercased() == "sse" {
                config = .sse(url: value.url, headers: value.headers ?? [:])
            } else {
                config = .http(url: value.url, headers: value.headers ?? [:])
            }
        }
        return .init(
            id: server.id,
            name: server.name,
            serverIdentifier: server.serverIdentifier,
            config: config,
            isTeamServer: server.isTeamServer,
            disabledByTeamAdminPolicy: server.disabledByTeamAdminPolicy,
            pluginId: server.pluginId,
            isRequired: server.isRequired,
            managedByTeamPluginPolicy: server.managedByTeamPluginPolicy,
            accounts: (server.accounts ?? []).map {
                .init(
                    accountKey: $0.accountKey,
                    hasToken: $0.hasToken,
                    serverIdentifier: $0.serverIdentifier
                )
            }
        )
    }
    return .init(
        servers: servers,
        cacheScope: fetched.cacheScope,
        unavailable: fetched.unavailable,
        unresolvedServerIds: fetched.unresolvedServerIds
    )
}

import Foundation

func projectSkillPublishTargets(_ teams: [IOSCursorSkillPublishTeam]) -> [[String: Any]] {
    teams
        .filter { $0.isDirectMember && $0.teamId > 0 }
        .sorted {
            if $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedSame {
                return $0.teamId < $1.teamId
            }
            return $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending
        }
        .map {
            [
                "teamId": NSNumber(value: $0.teamId),
                "name": $0.name,
            ]
        }
}

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
    private var dashboardServerIdentifiers = Set<String>()

    init(
        hostSupervisor: MahayanaLocalHostSupervisor,
        settingsStore: SandSettingsStore
    ) {
        self.hostSupervisor = hostSupervisor
        self.settingsStore = settingsStore
    }

    func updateDashboardServerIdentifiers(_ identifiers: Set<String>) {
        dashboardServerIdentifiers = identifiers
    }

    func usesDashboardServer(_ identifier: String) -> Bool {
        dashboardServerIdentifiers.contains(identifier)
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

    func exportWorkflowPublishPackage(
        agentId: String,
        workflowId: String
    ) async throws -> [String: Any] {
        let response = try await hostSupervisor.request(
            method: "feature.workflow.publishPackage",
            params: [
                "agentId": agentId,
                "workflowId": workflowId,
            ]
        )
        guard let package = response.value as? [String: Any] else {
            throw SandMcpConfigError("Skill publish package response is invalid.")
        }
        return package
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
    private let dashboard: IOSCursorDashboardClient

    init(
        port: CoordinatorMcpHostPort,
        dashboard: IOSCursorDashboardClient
    ) {
        self.port = port
        self.dashboard = dashboard
    }

    func listSandMcpTools(
        serverIdentifiers: [String],
        timeoutMs: Int
    ) async throws -> [BackendMcpToolServerWire] {
        var dashboardIdentifiers: [String] = []
        var hostIdentifiers: [String] = []
        for identifier in serverIdentifiers {
            if await port.usesDashboardServer(identifier) {
                dashboardIdentifiers.append(identifier)
            } else {
                hostIdentifiers.append(identifier)
            }
        }
        var rows: [BackendMcpToolServerWire] = []
        if !dashboardIdentifiers.isEmpty {
            rows += try await dashboard.listSandMcpTools(
                serverIdentifiers: dashboardIdentifiers,
                timeoutMs: timeoutMs
            )
        }
        if !hostIdentifiers.isEmpty {
            rows += try await port.listBackendServers(serverIdentifiers: hostIdentifiers)
        }
        return rows
    }

    func executeSandMcpTool(
        serverIdentifier: String,
        toolName: String,
        args: McpJSONValue,
        toolCallId: String,
        agentId: String,
        timeoutMs: Int
    ) async throws -> McpExecResult? {
        if await port.usesDashboardServer(serverIdentifier) {
            return try await dashboard.executeSandMcpTool(
                serverIdentifier: serverIdentifier,
                toolName: toolName,
                args: args,
                toolCallId: toolCallId,
                agentId: agentId,
                timeoutMs: timeoutMs
            )
        }
        return .init(
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
        try await dashboard.checkHttpMcpStatus(
            serverIds: serverIds,
            oauthRedirectUri: oauthRedirectUri,
            forceReauth: forceReauth,
            accountKey: accountKey,
            timeoutMs: timeoutMs
        )
    }

    func completeMcpOAuth(
        stateId: String,
        authorizationCode: String,
        timeoutMs: Int
    ) async throws {
        try await dashboard.completeMcpOAuth(
            stateId: stateId,
            authorizationCode: authorizationCode,
            timeoutMs: timeoutMs
        )
    }

    func validateMcpOAuthTokens(
        targets: [BackendMcpTokenTarget],
        timeoutMs: Int
    ) async throws -> [BackendMcpTokenValidation] {
        try await dashboard.validateMcpOAuthTokens(targets: targets, timeoutMs: timeoutMs)
    }

    func deleteMcpOAuthToken(
        serverUrl: String,
        accountKey: String,
        source: String,
        timeoutMs: Int
    ) async throws {
        try await dashboard.deleteMcpOAuthToken(
            serverUrl: serverUrl,
            accountKey: accountKey,
            source: source,
            timeoutMs: timeoutMs
        )
    }

    func renameMcpOAuthAccount(
        serverId: String,
        accountKey: String,
        newAccountKey: String,
        timeoutMs: Int
    ) async throws {
        try await dashboard.renameMcpOAuthAccount(
            serverId: serverId,
            accountKey: accountKey,
            newAccountKey: newAccountKey,
            timeoutMs: timeoutMs
        )
    }

    func deleteMcpOAuthAccount(
        serverId: String,
        accountKey: String,
        timeoutMs: Int
    ) async throws {
        try await dashboard.deleteMcpOAuthAccount(
            serverId: serverId,
            accountKey: accountKey,
            timeoutMs: timeoutMs
        )
    }
}

@MainActor
final class CoordinatorMcpSurface {
    private let manager: SandMcpManager
    private let port: CoordinatorMcpHostPort
    private let cursorAuth: IOSCursorAuthService
    private let dashboard: IOSCursorDashboardClient

    private init(
        manager: SandMcpManager,
        port: CoordinatorMcpHostPort,
        cursorAuth: IOSCursorAuthService,
        dashboard: IOSCursorDashboardClient
    ) {
        self.manager = manager
        self.port = port
        self.cursorAuth = cursorAuth
        self.dashboard = dashboard
    }

    static func make(
        hostSupervisor: MahayanaLocalHostSupervisor,
        settingsStore: SandSettingsStore
    ) -> CoordinatorMcpSurface {
        let port = CoordinatorMcpHostPort(
            hostSupervisor: hostSupervisor,
            settingsStore: settingsStore
        )
        let cursorAuth = IOSCursorAuthService()
        let credentials = AccountMcpCredentials(
            getAccessToken: { backendURL in
                try await cursorAuth.getValidAccessToken(backendURL: backendURL)
            },
            getMachineId: {
                try await cursorAuth.getMachineID()
            }
        )
        let dashboard = IOSCursorDashboardClient(credentials: credentials)
        let accountDependencies = AccountMcpDependencies(
            getAccessToken: { backendURL in
                try await cursorAuth.getValidAccessToken(backendURL: backendURL)
            },
            getMachineId: {
                try await cursorAuth.getMachineID()
            },
            getBackendUrl: { getConfiguredBackendUrl() },
            createClient: { credentials in
                IOSCursorDashboardClient(credentials: credentials)
            }
        )
        let client = CoordinatorMcpBackendClient(port: port, dashboard: dashboard)
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
            accountDisplayConfigProvider: { requireFreshRead in
                if await cursorAuth.status().loggedIn,
                   let fetched = await fetchAccountMcpServers(accountDependencies),
                   !fetched.unavailable {
                    let display = accountDisplayConfig(from: fetched)
                    await port.updateDashboardServerIdentifiers(
                        Set(display.servers.compactMap(\.serverIdentifier))
                    )
                    return display
                }
                await port.updateDashboardServerIdentifiers([])
                return try await port.loadAccountDisplay(forceFresh: requireFreshRead)
            },
            accountMcpWriter: createAccountMcpWriter(accountDependencies),
            autoPollEnabled: true
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
            port: port,
            cursorAuth: cursorAuth,
            dashboard: dashboard
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
        case "coordinator.skill.publishTargets":
            guard (await cursorAuth.status()).loggedIn else {
                throw SandMcpConfigError("Skill publishing requires Cursor sign-in.")
            }
            let teams = try await dashboard.getSkillPublishTeams()
            return .handled(["teams": projectSkillPublishTargets(teams)])

        case "coordinator.skill.publishUpload":
            guard (await cursorAuth.status()).loggedIn else {
                throw SandMcpConfigError("Skill publishing requires Cursor sign-in.")
            }
            guard let agentId = nonEmptyString(params["agentId"]),
                  let workflowId = nonEmptyString(params["workflowId"]),
                  let teamNumber = params["teamId"] as? NSNumber
            else {
                throw SandMcpConfigError(
                    "Skill publishing requires agentId, workflowId, and teamId."
                )
            }
            let teamValue = teamNumber.int64Value
            guard teamValue > 0, teamValue <= Int64(Int32.max) else {
                throw SandMcpConfigError("Skill publishing teamId is invalid.")
            }
            let package = try await port.exportWorkflowPublishPackage(
                agentId: agentId,
                workflowId: workflowId
            )
            guard let name = nonEmptyString(package["name"]),
                  let displayName = nonEmptyString(package["displayName"]),
                  let description = nonEmptyString(package["description"]),
                  let encoded = nonEmptyString(package["pluginTarGzBase64"]),
                  let archive = Data(base64Encoded: encoded)
            else {
                throw SandMcpConfigError("Skill publish package is incomplete.")
            }
            let published = try await dashboard.publishSkillPlugin(
                teamId: Int32(teamValue),
                name: name,
                displayName: displayName,
                description: description,
                pluginTarGz: archive
            )
            // This is intentionally only an upload primitive. The private workflow
            // remains authoritative until a later refresh confirms the same plugin
            // id + commit SHA in the installed plugin-skill projection.
            return .handled([
                "workflowId": workflowId,
                "pluginId": published.pluginId,
                "commitSha": published.commitSha,
                "confirmed": false,
            ])

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

        case "coordinator.mcp.cursorAuth.status":
            return .handled(projectCursorAuthStatus(await cursorAuth.status()))

        case "coordinator.mcp.cursorAuth.start":
            let start = try cursorAuth.beginLogin()
            return .handled([
                "attemptId": start.attemptId,
                "loginUrl": start.loginURL.absoluteString,
            ])

        case "coordinator.mcp.cursorAuth.poll":
            guard let attemptId = nonEmptyString(params["attemptId"]) else {
                throw SandMcpConfigError("MCP backend sign-in attempt id is required.")
            }
            if let status = try await cursorAuth.pollLogin(attemptId: attemptId) {
                await manager.reload()
                return .handled([
                    "completed": true,
                    "auth": projectCursorAuthStatus(status),
                ])
            }
            return .handled(["completed": false])

        case "coordinator.mcp.cursorAuth.cancel":
            guard let attemptId = nonEmptyString(params["attemptId"]) else {
                throw SandMcpConfigError("MCP backend sign-in attempt id is required.")
            }
            cursorAuth.cancelLogin(attemptId: attemptId)
            return .handled(["cancelled": true])

        case "coordinator.mcp.cursorAuth.logout":
            try await cursorAuth.logout()
            await manager.reload()
            return .handled(["loggedIn": false])

        case "coordinator.mcp.authenticate":
            guard let serverId = nonEmptyString(params["serverId"]) else {
                throw SandMcpConfigError("MCP server id is required.")
            }
            let accountKey = nonEmptyString(params["accountKey"]) ?? DEFAULT_MCP_ACCOUNT_KEY
            let result = try await manager.authenticateServer(
                serverId,
                accountKey: accountKey,
                forceReauth: params["forceReauth"] as? Bool ?? false,
                trigger: "connector_card"
            )
            let payload: [String: Any] = [
                "status": result.status.rawValue,
                "serverName": result.serverName,
                "authorizationUrl": result.authorizationUrl ?? NSNull(),
                "message": result.message ?? NSNull(),
            ]
            return .handled(payload)

        case "coordinator.mcp.oauthCallback":
            guard let rawURL = nonEmptyString(params["url"]),
                  let url = URL(string: rawURL)
            else {
                throw SandMcpConfigError("MCP OAuth callback URL is invalid.")
            }
            let outcome = await manager.handleOAuthCallback(url)
            await manager.reload()
            return .handled(["outcome": projectOAuthOutcome(outcome)])

        case "coordinator.mcp.logoutAccount":
            guard let serverId = nonEmptyString(params["serverId"]),
                  let accountKey = nonEmptyString(params["accountKey"])
            else {
                throw SandMcpConfigError("MCP server id and account label are required.")
            }
            let state = try await manager.logoutAccount(serverId: serverId, accountKey: accountKey)
            return .handled(["servers": state.servers.map(projectServer)])

        case "coordinator.mcp.renameAccount":
            guard let serverId = nonEmptyString(params["serverId"]),
                  let accountKey = nonEmptyString(params["accountKey"]),
                  let newAccountKey = nonEmptyString(params["newAccountKey"])
            else {
                throw SandMcpConfigError("MCP server id and account labels are required.")
            }
            let state = try await manager.renameAccount(
                serverId: serverId,
                accountKey: accountKey,
                newAccountKey: newAccountKey
            )
            return .handled(["servers": state.servers.map(projectServer)])

        case "coordinator.mcp.removeAccount":
            guard let serverId = nonEmptyString(params["serverId"]),
                  let accountKey = nonEmptyString(params["accountKey"])
            else {
                throw SandMcpConfigError("MCP server id and account label are required.")
            }
            let state = try await manager.removeAccount(serverId: serverId, accountKey: accountKey)
            return .handled(["servers": state.servers.map(projectServer)])

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
            "accountKey": server.accountKey,
            "rowServerIdentifier": server.rowServerIdentifier,
            "transport": server.transport.rawValue,
            "toolCount": server.toolCount,
            "disabledToolCount": server.disabledToolCount,
            "isTeamServer": server.isTeamServer,
            "pluginId": server.attribution.pluginId ?? NSNull(),
            "isRequired": server.attribution.isRequired,
            "managedByTeamPluginPolicy": server.attribution.managedByTeamPluginPolicy,
            "status": server.status,
            "statusDetail": server.statusDetail ?? NSNull(),
        ]
    }

    private func projectCursorAuthStatus(_ status: IOSCursorAuthStatus) -> [String: Any] {
        [
            "loggedIn": status.loggedIn,
            "authId": status.authId ?? NSNull(),
            "email": status.email ?? NSNull(),
            "expiresAtMs": status.expiresAtMs.map(NSNumber.init(value:)) ?? NSNull(),
        ]
    }

    private func projectOAuthOutcome(_ outcome: McpOAuthCallbackOutcome) -> String {
        switch outcome {
        case .success: return "success"
        case .refused(let reason): return "refused:\(reason.rawValue)"
        case .failed(let reason, let retryable):
            return "failed:\(reason.rawValue):\(retryable ? "retryable" : "terminal")"
        case .notFound: return "notFound"
        case .unsupportedURL: return "unsupportedURL"
        }
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
