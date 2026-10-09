
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

let SKILL_PUBLISH_CONFIRM_MAX_ATTEMPTS = 5

func normalizedSkillPublishVersion(_ raw: String?) -> String? {
    guard let raw else { return nil }
    let value = raw.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    guard value.count == 40, value.utf8.allSatisfy({
        ($0 >= 48 && $0 <= 57) || ($0 >= 97 && $0 <= 102)
    }) else { return nil }
    return value
}

func projectAuthoritativePublishedPlugins(
    _ plugins: [EffectiveUserPlugin],
    currentUserId: UInt64
) -> [[String: Any]] {
    plugins.compactMap { plugin in
        guard let version = normalizedSkillPublishVersion(plugin.versionRef) else { return nil }
        var row: [String: Any] = [
            "pluginId": plugin.pluginId,
            "pluginVersion": version,
            "name": plugin.name,
            "displayName": plugin.displayName,
            "publishedByCurrentUser": plugin.publisherUserId == currentUserId,
            "isEnabledForAgent": plugin.isEnabled,
        ]
        if let teamId = plugin.marketplaceTeamId {
            row["marketplaceTeamId"] = NSNumber(value: teamId)
        }
        return row
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

    func syncWorkflowPluginFacts(
        agentId: String,
        plugins: [[String: Any]]
    ) async throws -> [String: Any] {
        let response = try await hostSupervisor.request(
            method: "feature.workflow.pluginFactsSync",
            params: [
                "agentId": agentId,
                "plugins": plugins,
            ]
        )
        guard let value = response.value as? [String: Any] else {
            throw SandMcpConfigError("Published skill projection response is invalid.")
        }
        return value
    }

    func confirmWorkflowPublish(
        agentId: String,
        workflowId: String,
        pluginId: String,
        commitSha: String
    ) async throws -> [String: Any] {
        let response = try await hostSupervisor.request(
            method: "feature.workflow.publishConfirm",
            params: [
                "agentId": agentId,
                "workflowId": workflowId,
                "pluginId": pluginId,
                "commitSha": commitSha,
            ]
        )
        guard let value = response.value as? [String: Any] else {
            throw SandMcpConfigError("Skill publish confirmation response is invalid.")
        }
        return value
    }

    func confirmPublishedWorkflowResync(
        agentId: String,
        workflowId: String,
        pluginId: String,
        commitSha: String
    ) async throws -> [String: Any] {
        let response = try await hostSupervisor.request(
            method: "feature.workflow.resyncConfirm",
            params: [
                "agentId": agentId,
                "workflowId": workflowId,
                "pluginId": pluginId,
                "commitSha": commitSha,
            ]
        )
        guard let value = response.value as? [String: Any] else {
            throw SandMcpConfigError("Skill resync confirmation response is invalid.")
        }
        return value
    }

    func exportPublishedWorkflowPublishPackage(
        agentId: String,
        workflowId: String
    ) async throws -> [String: Any] {
        let response = try await hostSupervisor.request(
            method: "feature.workflow.resyncPackage",
            params: [
                "agentId": agentId,
                "workflowId": workflowId,
            ]
        )
        guard let value = response.value as? [String: Any] else {
            throw SandMcpConfigError("Published skill package response is invalid.")
        }
        return value
    }

    func prepareWorkflowUnpublish(
        agentId: String,
        workflowId: String
    ) async throws -> [String: Any] {
        let response = try await hostSupervisor.request(
            method: "feature.workflow.unpublishPrepare",
            params: [
                "agentId": agentId,
                "workflowId": workflowId,
            ]
        )
        guard let value = response.value as? [String: Any] else {
            throw SandMcpConfigError("Skill unpublish preparation response is invalid.")
        }
        return value
    }

    func completeWorkflowUnpublish(
        agentId: String,
        pluginId: String
    ) async throws {
        _ = try await hostSupervisor.request(
            method: "feature.workflow.unpublishComplete",
            params: [
                "agentId": agentId,
                "pluginId": pluginId,
            ]
        )
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
    private let avatarImageGenerator: CursorGenerateImageService
    private var computerMigrationWatchTask: Task<Void, Never>?
    private var computerMigrationResumeOffsetKey = ""
    private var computerMigrationStatus: [String: Any]?
    private var computerMigrationLastTerminal: [String: Any]?
    private var computerMigrationOwedOperationId: String?
    private var computerMigrationGeneration: UInt64 = 0
    private var accountScope: String?

    private init(
        manager: SandMcpManager,
        port: CoordinatorMcpHostPort,
        cursorAuth: IOSCursorAuthService,
        dashboard: IOSCursorDashboardClient,
        avatarImageGenerator: CursorGenerateImageService
    ) {
        self.manager = manager
        self.port = port
        self.cursorAuth = cursorAuth
        self.dashboard = dashboard
        self.avatarImageGenerator = avatarImageGenerator
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
        let avatarImageGenerator = createCursorGenerateImageService(
            client: IOSCursorGenerateImageClient(credentials: credentials),
            modelId: SAND_DEFAULT_MODEL_ID
        )
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
        dependencies.effectivePluginsProvider = {
            try await fetchEffectiveUserPlugins(accountDependencies)
        }
        return .init(
            manager: SandMcpManager(deps: dependencies),
            port: port,
            cursorAuth: cursorAuth,
            dashboard: dashboard,
            avatarImageGenerator: avatarImageGenerator
        )
    }

    func updateAccountScope(_ scope: String?) {
        let normalized = scope?.trimmingCharacters(in: .whitespacesAndNewlines)
        let next = normalized?.isEmpty == false ? normalized : nil
        if next != accountScope {
            accountScope = next
            stopComputerMigrationWatch(clearState: true)
        }
        port.updateAccountScope(next)
    }

    private func stopComputerMigrationWatch(clearState: Bool) {
        computerMigrationGeneration &+= 1
        computerMigrationWatchTask?.cancel()
        computerMigrationWatchTask = nil
        if clearState {
            computerMigrationResumeOffsetKey = ""
            computerMigrationStatus = nil
            computerMigrationLastTerminal = nil
            computerMigrationOwedOperationId = nil
        }
    }

    private func ensureComputerMigrationWatch() {
        guard computerMigrationWatchTask == nil else { return }
        computerMigrationGeneration &+= 1
        let generation = computerMigrationGeneration
        computerMigrationWatchTask = Task { @MainActor [weak self] in
            guard let self else { return }
            defer {
                if self.computerMigrationGeneration == generation {
                    self.computerMigrationWatchTask = nil
                }
            }
            while !Task.isCancelled, self.computerMigrationGeneration == generation {
                let fromOffset = self.computerMigrationResumeOffsetKey
                var received = false
                do {
                    for try await event in self.dashboard.watchSandBoxMigration(
                        fromOffsetKey: fromOffset,
                        includeFinished: true
                    ) {
                        guard !Task.isCancelled,
                              self.computerMigrationGeneration == generation
                        else { return }
                        received = true
                        self.ingestComputerMigration(event)
                    }
                } catch {
                    if !received && !fromOffset.isEmpty {
                        self.computerMigrationResumeOffsetKey = ""
                    }
                }
                guard !Task.isCancelled,
                      self.computerMigrationGeneration == generation
                else { return }
                do {
                    try await Task.sleep(for: .seconds(3))
                } catch {
                    return
                }
                self.computerMigrationStatus = nil
            }
        }
    }

    private func ingestComputerMigration(_ event: IOSCursorSandBoxMigrationEvent) {
        guard let phase = event.phaseName else { return }
        if !event.offsetKey.isEmpty {
            computerMigrationResumeOffsetKey = event.offsetKey
        }
        var projected: [String: Any] = [
            "phase": phase,
            "detail": event.detail,
        ]
        if !event.operationId.isEmpty {
            projected["operationId"] = event.operationId
        }
        computerMigrationStatus = projected
        if event.isTerminal {
            if let owed = computerMigrationOwedOperationId,
               !event.operationId.isEmpty,
               event.operationId != owed {
                return
            }
            computerMigrationLastTerminal = projected
            computerMigrationOwedOperationId = nil
        } else if let terminalOperation = computerMigrationLastTerminal?["operationId"] as? String,
                  !event.operationId.isEmpty,
                  terminalOperation != event.operationId {
            computerMigrationLastTerminal = nil
        }
    }

    private func projectRecreate(
        _ result: IOSCursorSandBoxRecreateResult
    ) -> [String: Any] {
        guard result.started else {
            return [
                "status": "rejected",
                "reason": result.reason.isEmpty
                    ? "Computer recreation was rejected by the backend."
                    : result.reason,
            ]
        }
        if !result.operationId.isEmpty {
            computerMigrationOwedOperationId = result.operationId
            return [
                "status": "started",
                "operationId": result.operationId,
            ]
        }
        computerMigrationOwedOperationId = nil
        return ["status": "started-untrackable"]
    }

    private func requireCursorComputerLifecycle() async throws {
        guard (await cursorAuth.status()).loggedIn else {
            throw IOSCursorAuthError.signInRequired
        }
        ensureComputerMigrationWatch()
    }

    func cloudAgentInfo(bcId: String) async throws -> IOSCloudAgentComposerInfo {
        guard (await cursorAuth.status()).loggedIn else {
            throw IOSCursorAuthError.signInRequired
        }
        return try await dashboard.getBackgroundComposerInfo(bcId: bcId)
    }

    private func refreshAuthoritativePluginFacts(
        agentId: String
    ) async throws -> [[String: Any]] {
        guard (await cursorAuth.status()).loggedIn else {
            let empty: [[String: Any]] = []
            _ = try await port.syncWorkflowPluginFacts(agentId: agentId, plugins: empty)
            return empty
        }
        let currentUserId = try await dashboard.getCurrentUserId()
        let effective = try await manager.listEffectivePlugins()
        let projected = projectAuthoritativePublishedPlugins(
            effective,
            currentUserId: currentUserId
        )
        _ = try await port.syncWorkflowPluginFacts(agentId: agentId, plugins: projected)
        return projected
    }

    private func publishTeamId(_ value: Any?) throws -> Int32 {
        guard let number = value as? NSNumber else {
            throw SandMcpConfigError("Skill publishing teamId is required.")
        }
        let raw = number.int64Value
        guard raw > 0, raw <= Int64(Int32.max) else {
            throw SandMcpConfigError("Skill publishing teamId is invalid.")
        }
        return Int32(raw)
    }

    private func uploadPrivateWorkflow(
        agentId: String,
        workflowId: String,
        teamId: Int32
    ) async throws -> IOSCursorPublishedSkill {
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
        return try await dashboard.publishSkillPlugin(
            teamId: teamId,
            name: name,
            displayName: displayName,
            description: description,
            pluginTarGz: archive
        )
    }

    func route(
        method: String,
        params: [String: Any]
    ) async throws -> CoordinatorDevControlRouting {
        switch method {
        case "getBoxMigrationStatus":
            try await requireCursorComputerLifecycle()
            if let status = computerMigrationStatus ?? computerMigrationLastTerminal {
                return .handled(status)
            }
            return .handled(NSNull())

        case "updateComputer":
            try await requireCursorComputerLifecycle()
            guard nonEmptyString(params["id"]) != nil else {
                throw SandMcpConfigError("A computer update requires an agent id.")
            }
            let result = try await dashboard.recreateSandBox(
                preserveData: true,
                force: params["force"] as? Bool == true
            )
            return .handled(projectRecreate(result))

        case "forceRecreateComputer":
            try await requireCursorComputerLifecycle()
            return .handled(projectRecreate(try await dashboard.forceRecreateSandBox()))

        case "generateAgentAvatarImage":
            guard (await cursorAuth.status()).loggedIn else {
                throw SandMcpConfigError("Avatar generation requires Cursor sign-in.")
            }
            guard let description = nonEmptyString(params["description"]) else {
                throw SandMcpConfigError("Describe the avatar to generate first.")
            }
            let generated = try await avatarImageGenerator.generate(description: description)
            guard !generated.imageData.isEmpty, generated.mimeType.hasPrefix("image/") else {
                throw SandMcpConfigError("Avatar generation returned invalid image data.")
            }
            return .handled("data:\(generated.mimeType);base64,\(generated.imageData)")

        case "coordinator.skill.publishTargets":
            guard (await cursorAuth.status()).loggedIn else {
                throw SandMcpConfigError("Skill publishing requires Cursor sign-in.")
            }
            let teams = try await dashboard.getSkillPublishTeams()
            return .handled(["teams": projectSkillPublishTargets(teams)])

        case "coordinator.skill.pluginFactsSync":
            guard let agentId = nonEmptyString(params["agentId"]) else {
                throw SandMcpConfigError("Published skill refresh requires agentId.")
            }
            let plugins = try await refreshAuthoritativePluginFacts(agentId: agentId)
            return .handled(["plugins": plugins])

        case "coordinator.skill.publish":
            guard (await cursorAuth.status()).loggedIn else {
                throw SandMcpConfigError("Skill publishing requires Cursor sign-in.")
            }
            guard let agentId = nonEmptyString(params["agentId"]),
                  let workflowId = nonEmptyString(params["workflowId"])
            else {
                throw SandMcpConfigError(
                    "Skill publishing requires agentId and workflowId."
                )
            }
            let teamId = try publishTeamId(params["teamId"])
            let published = try await uploadPrivateWorkflow(
                agentId: agentId,
                workflowId: workflowId,
                teamId: teamId
            )
            var promotedWorkflowId: String?
            for _ in 0..<SKILL_PUBLISH_CONFIRM_MAX_ATTEMPTS {
                _ = try? await refreshAuthoritativePluginFacts(agentId: agentId)
                let confirmation = try await port.confirmWorkflowPublish(
                    agentId: agentId,
                    workflowId: workflowId,
                    pluginId: published.pluginId,
                    commitSha: published.commitSha
                )
                if confirmation["confirmed"] as? Bool == true {
                    promotedWorkflowId = nonEmptyString(confirmation["promotedWorkflowId"])
                    break
                }
            }
            var result: [String: Any] = [
                "workflowId": workflowId,
                "pluginId": published.pluginId,
                "commitSha": published.commitSha,
                "confirmed": promotedWorkflowId != nil,
            ]
            if let promotedWorkflowId {
                result["promotedWorkflowId"] = promotedWorkflowId
            }
            return .handled(result)

        case "coordinator.skill.publishUpload":
            guard (await cursorAuth.status()).loggedIn else {
                throw SandMcpConfigError("Skill publishing requires Cursor sign-in.")
            }
            guard let agentId = nonEmptyString(params["agentId"]),
                  let workflowId = nonEmptyString(params["workflowId"])
            else {
                throw SandMcpConfigError(
                    "Skill publishing requires agentId and workflowId."
                )
            }
            let published = try await uploadPrivateWorkflow(
                agentId: agentId,
                workflowId: workflowId,
                teamId: try publishTeamId(params["teamId"])
            )
            return .handled([
                "workflowId": workflowId,
                "pluginId": published.pluginId,
                "commitSha": published.commitSha,
                "confirmed": false,
            ])

        case "coordinator.skill.resync":
            guard (await cursorAuth.status()).loggedIn else {
                throw SandMcpConfigError("Skill sync requires Cursor sign-in.")
            }
            guard let agentId = nonEmptyString(params["agentId"]),
                  let workflowId = nonEmptyString(params["workflowId"])
            else {
                throw SandMcpConfigError("Skill sync requires agentId and workflowId.")
            }
            _ = try await refreshAuthoritativePluginFacts(agentId: agentId)
            let package = try await port.exportPublishedWorkflowPublishPackage(
                agentId: agentId,
                workflowId: workflowId
            )
            guard let pluginId = nonEmptyString(package["pluginId"]),
                  let teamIdNumber = package["teamId"] as? NSNumber,
                  let name = nonEmptyString(package["name"]),
                  let displayName = nonEmptyString(package["displayName"]),
                  let description = nonEmptyString(package["description"]),
                  let encoded = nonEmptyString(package["pluginTarGzBase64"]),
                  let archive = Data(base64Encoded: encoded)
            else {
                throw SandMcpConfigError("Published skill package is incomplete.")
            }
            let teamRaw = teamIdNumber.int64Value
            guard teamRaw > 0, teamRaw <= Int64(Int32.max) else {
                throw SandMcpConfigError("Published skill team id is invalid.")
            }
            let published = try await dashboard.publishSkillPlugin(
                teamId: Int32(teamRaw),
                name: name,
                displayName: displayName,
                description: description,
                pluginTarGz: archive
            )
            guard published.pluginId == pluginId else {
                throw SandMcpConfigError(
                    "Skill sync returned a different plugin id; local published state was preserved."
                )
            }
            var confirmed = false
            for _ in 0..<SKILL_PUBLISH_CONFIRM_MAX_ATTEMPTS {
                _ = try? await refreshAuthoritativePluginFacts(agentId: agentId)
                let confirmation = try await port.confirmPublishedWorkflowResync(
                    agentId: agentId,
                    workflowId: workflowId,
                    pluginId: published.pluginId,
                    commitSha: published.commitSha
                )
                if confirmation["confirmed"] as? Bool == true {
                    confirmed = true
                    break
                }
            }
            return .handled([
                "workflowId": workflowId,
                "pluginId": published.pluginId,
                "commitSha": published.commitSha,
                "confirmed": confirmed,
            ])

        case "coordinator.skill.unpublish":
            guard (await cursorAuth.status()).loggedIn else {
                throw SandMcpConfigError("Skill unpublish requires Cursor sign-in.")
            }
            guard let agentId = nonEmptyString(params["agentId"]),
                  let workflowId = nonEmptyString(params["workflowId"])
            else {
                throw SandMcpConfigError("Skill unpublish requires agentId and workflowId.")
            }
            _ = try await refreshAuthoritativePluginFacts(agentId: agentId)
            let prepared = try await port.prepareWorkflowUnpublish(
                agentId: agentId,
                workflowId: workflowId
            )
            guard let pluginId = nonEmptyString(prepared["pluginId"]),
                  let teamNumber = prepared["teamId"] as? NSNumber,
                  let restoredWorkflowId = nonEmptyString(prepared["restoredWorkflowId"])
            else {
                throw SandMcpConfigError("Skill unpublish preparation is incomplete.")
            }
            let teamRaw = teamNumber.int64Value
            guard teamRaw > 0, teamRaw <= Int64(Int32.max) else {
                throw SandMcpConfigError("Published skill team id is invalid.")
            }
            // Host restores the private copy before this remote mutation. If the
            // backend fails, that restored local copy intentionally remains.
            try await dashboard.unpublishSkillPlugin(
                pluginId: pluginId,
                teamId: Int32(teamRaw)
            )
            try await port.completeWorkflowUnpublish(
                agentId: agentId,
                pluginId: pluginId
            )
            _ = try? await refreshAuthoritativePluginFacts(agentId: agentId)
            return .handled([
                "workflowId": workflowId,
                "pluginId": pluginId,
                "restoredWorkflowId": restoredWorkflowId,
                "unpublished": true,
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

        case "coordinator.account.prReviewPreferences":
            guard (await cursorAuth.status()).loggedIn else {
                return .handled([
                    "user": NSNull(),
                    "team": NSNull(),
                ])
            }
            let preferences = try await dashboard.getPrReviewPreferences()
            return .handled([
                "user": preferences.user?.rawValue ?? NSNull(),
                "team": preferences.team?.rawValue ?? NSNull(),
            ])

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
