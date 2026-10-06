import Foundation

struct SandMcpManagerDependencies: @unchecked Sendable {
    let settingsStore: SandSettingsStore
    let backendMcpExec: DashboardSandBackendMcpExec
    let definitionSource: SandMcpDefinitionSource
    let toolsDiscovery: SandMcpToolsDiscovery
    let accountDisplayConfigProvider: @Sendable (_ requireFreshRead: Bool) async throws -> AccountDisplayConfig?
    var accountMcpWriter: AccountMcpWriter? = nil
    var catalog: SandMcpCatalogFlow? = nil
    var effectivePluginsProvider: (@Sendable () async throws -> [EffectiveUserPlugin])? = nil
    var parseServerConfig: (@Sendable (Any) throws -> McpServerConfig)? = nil
    var onConnectorAuth: (@Sendable (McpConnectorAuthEvent) -> Void)? = nil
    var onAccountScopeApplied: (@Sendable () -> Void)? = nil
    var setToolDisabled: (@Sendable (_ serverIdentifier: String, _ toolName: String, _ disabled: Bool) async throws -> Void)? = nil
    var authWatchPollIntervalMs: Int = AUTH_WATCH_POLL_INTERVAL_MS
    var authWatchTimeoutMs: Int = AUTH_WATCH_TIMEOUT_MS
    var autoPollEnabled: Bool = true
}

struct SandMcpRemoveResult: Equatable, Sendable {
    let state: McpListedState
    let removed: Bool
    var reason: String? = nil
}

private func accountMcpServerConfig(from config: McpServerConfig) -> AccountMcpServerConfig {
    switch config.transport {
    case .stdio:
        return .stdio(.init(
            type: "stdio",
            command: config.command ?? "",
            args: config.args,
            env: config.env.isEmpty ? nil : config.env
        ))
    case .sse:
        return .remote(.init(
            type: "sse",
            url: config.url ?? "",
            headers: config.headers.isEmpty ? nil : config.headers
        ))
    case .http:
        return .remote(.init(
            type: "http",
            url: config.url ?? "",
            headers: config.headers.isEmpty ? nil : config.headers
        ))
    }
}

private func mergeAccountDisplay(
    _ fresh: AccountDisplayConfig,
    cached: AccountDisplayConfig?
) -> AccountDisplayConfig {
    guard let cached,
          !fresh.unresolvedServerIds.isEmpty else { return fresh }
    let unresolved = Set(fresh.unresolvedServerIds)
    let freshById = Dictionary(uniqueKeysWithValues: fresh.servers.map { ($0.id, $0) })
    let cachedIds = Set(cached.servers.map(\.id))
    var servers: [DisplayServer] = []

    for old in cached.servers {
        if let replacement = freshById[old.id] {
            servers.append(replacement)
        } else if unresolved.contains(old.id) {
            servers.append(old)
        }
    }
    servers.append(contentsOf: fresh.servers.filter { !cachedIds.contains($0.id) })
    return .init(
        servers: servers,
        cacheScope: fresh.cacheScope,
        unavailable: fresh.unavailable,
        unresolvedServerIds: fresh.unresolvedServerIds
    )
}

/// iOS owner for Grok's shared MCP manager boundary.
///
/// This actor deliberately keeps process effects outside the shared layer:
/// HTTP/SSE uses the backend port and stdio uses `SandMcpToolsDiscovery`'s
/// `RunnerMcpExecuting` port. It never spawns a local stdio process on iOS.
actor SandMcpManager {
    private let deps: SandMcpManagerDependencies
    private let oauthCallbacks: SandMcpOAuthCallbackLifecycle
    private var authWatches: SandMcpAuthWatchLifecycle?
    private var lastDisplay: AccountDisplayConfig?
    private var lastState: McpListedState?
    private var lastBackendTools: [NamedBackendTool] = []
    private var lastScope: String?

    init(deps: SandMcpManagerDependencies) {
        self.deps = deps
        let backend = deps.backendMcpExec
        self.oauthCallbacks = SandMcpOAuthCallbackLifecycle(
            completeOAuth: { stateId, code in
                try await backend.completeOAuth(stateId: stateId, code: code)
            }
        )
    }

    private func authLifecycle() -> SandMcpAuthWatchLifecycle {
        if let authWatches { return authWatches }
        let backend = deps.backendMcpExec
        let oauthCallbacks = self.oauthCallbacks
        let lifecycle = SandMcpAuthWatchLifecycle(deps: .init(
            checkAuthStatus: { serverId, redirect, forceReauth, accountKey in
                try await backend.checkAuthStatus(
                    serverId: serverId,
                    oauthRedirectUri: redirect,
                    forceReauth: forceReauth,
                    accountKey: accountKey
                )
            },
            validateTokens: { targets in
                await backend.validateTokens(targets)
            },
            resolveDisplayServer: { [weak self] id, fresh in
                guard let self else { return nil }
                return try await self.resolveDisplayServer(
                    id,
                    requireFreshRead: fresh
                )
            },
            reload: { [weak self] in
                await self?.reload()
            },
            registerOAuthCallback: { authorizationUrl, serverName in
                await oauthCallbacks.registerPendingAuthFromUrl(
                    authorizationUrl: authorizationUrl,
                    serverName: serverName
                )
            },
            onConnectorAuth: deps.onConnectorAuth,
            authWatchPollIntervalMs: deps.authWatchPollIntervalMs,
            authWatchTimeoutMs: deps.authWatchTimeoutMs,
            autoPollEnabled: deps.autoPollEnabled
        ))
        authWatches = lifecycle
        return lifecycle
    }

    func setAuthCompletionObserver(
        _ observer: (@Sendable (McpAuthCompletion) -> Void)?
    ) async {
        await authLifecycle().setAuthCompletionObserver(observer)
    }

    func setRunnerMcpExec(_ runner: (any RunnerMcpExecuting)?) async {
        await deps.toolsDiscovery.setRunnerMcpExec(runner)
    }

    func definitionSourceView() -> SandMcpDefinitionSource {
        deps.definitionSource
    }

    func backendMcpExecView() -> DashboardSandBackendMcpExec {
        deps.backendMcpExec
    }

    func lastAccountDisplayConfigView() -> AccountDisplayConfig? {
        lastDisplay
    }

    func lastListedStateView() -> McpListedState? {
        lastState
    }

    private func loadDisplay(requireFreshRead: Bool) async throws -> AccountDisplayConfig? {
        let fetched: AccountDisplayConfig?
        do {
            fetched = try await deps.accountDisplayConfigProvider(requireFreshRead)
        } catch {
            if requireFreshRead { throw error }
            return lastDisplay
        }

        guard var display = fetched else {
            if requireFreshRead {
                throw SandMcpConfigError("account display config unavailable")
            }
            return lastDisplay
        }

        if display.unavailable {
            if requireFreshRead {
                throw SandMcpConfigError("account display config unavailable")
            }
            return lastDisplay
        }

        let sameScope = display.cacheScope != nil && display.cacheScope == lastScope
        display = mergeAccountDisplay(display, cached: sameScope ? lastDisplay : nil)

        if display.cacheScope != lastScope {
            if lastScope != nil || lastDisplay != nil {
                lastDisplay = nil
                lastState = nil
                lastBackendTools = []
                if let authWatches {
                    await authWatches.clearAllPendingAuthWatchesCancelled()
                }
                await deps.toolsDiscovery.invalidateToolsCache()
                await deps.toolsDiscovery.resetPushState()
                await deps.definitionSource.clearLastKnownAccountConfig()
            }
            if let scope = display.cacheScope {
                deps.settingsStore.scopeToAccount(scope)
            } else {
                deps.settingsStore.clearAccountScope()
            }
            lastScope = display.cacheScope
            deps.onAccountScopeApplied?()
        }

        lastDisplay = display
        await deps.definitionSource.adoptAccountConfig(runtimeConfigFromDisplay(display))
        await deps.toolsDiscovery.setAccountDisplay(display)
        for server in display.servers where server.id != "0" {
            deps.settingsStore.migrateMcpCustomInstructionToServerId(
                serverId: server.id,
                displayName: server.name
            )
        }
        return display
    }

    func resolveDisplayServer(
        _ rawId: String,
        requireFreshRead: Bool = false
    ) async throws -> DisplayServer? {
        let id = try validateMcpServerId(rawId)
        let display = try await loadDisplay(requireFreshRead: requireFreshRead)
        return display?.servers.first { $0.id == id }
    }

    func listServers() async throws -> McpListedState {
        let display = try await loadDisplay(requireFreshRead: false)
        let runtime = await deps.definitionSource.getUserServerConfigs()

        var rows = display?.servers ?? []
        if rows.isEmpty {
            rows = runtime.sorted { $0.key < $1.key }.map { identifier, config in
                .init(
                    id: "0",
                    name: identifier,
                    serverIdentifier: identifier,
                    config: config,
                    isTeamServer: false
                )
            }
        }
        let visible = rows.filter {
            guard let identifier = $0.serverIdentifier else { return true }
            return !BUILTIN_MCP_SERVER_NAMES.contains(identifier)
        }

        let httpRows = visible.filter {
            !$0.disabledByTeamAdminPolicy
                && $0.serverIdentifier != nil
                && $0.config.transport != .stdio
        }
        let stdioRows = visible.filter {
            !$0.disabledByTeamAdminPolicy
                && $0.serverIdentifier != nil
                && $0.config.transport == .stdio
        }

        let backend = httpRows.isEmpty
            ? []
            : try await deps.backendMcpExec.listTools(
                serverIdentifiers: httpRows.compactMap(\.serverIdentifier)
            )

        let runnerWired = await deps.toolsDiscovery.isRunnerExecWired()
        var runnerByIdentifier: [String: RunnerMcpToolServer] = [:]
        var runnerUnavailable = false
        if runnerWired, !stdioRows.isEmpty {
            do {
                for server in try await deps.toolsDiscovery.listRunnerServers(
                    serverIdentifiers: stdioRows.compactMap(\.serverIdentifier)
                ) {
                    runnerByIdentifier[server.serverIdentifier] = server
                }
                runnerUnavailable = runnerByIdentifier.isEmpty
            } catch {
                reportMcpHostEdgeFailure("runner-settings-list", error: error)
                runnerUnavailable = true
            }
        }

        let summaries = SandMcpListingSummaries(
            settingsStore: { self.deps.settingsStore },
            isRemoteRunnerExecWired: { runnerWired }
        )
        let disabled = deps.settingsStore.getMcpDisabledToolsByServerId()
        lastBackendTools = backend.flatMap { entry -> [NamedBackendTool] in
            guard let row = httpRows.first(where: {
                guard let identifier = $0.serverIdentifier else { return false }
                return backendEntryBelongsToRow(
                    rowServerIdentifier: entry.rowServerIdentifier,
                    rowIdentifier: identifier
                )
            }) else { return [] }
            let blocked = Set(disabled[row.id] ?? [])
            return entry.tools.filter { !blocked.contains($0.toolName) }
        }

        var result: [McpServerSummary] = []
        for server in visible {
            if server.disabledByTeamAdminPolicy {
                result.append(summaries.createAdminDisabledServerSummary(server))
                continue
            }

            if server.config.transport == .stdio {
                let box = server.serverIdentifier.flatMap { runnerByIdentifier[$0] }.map {
                    McpBoxEntry(
                        status: $0.status,
                        statusDetail: $0.statusDetail,
                        toolCount: $0.toolCount,
                        tools: $0.tools.map { .init(toolName: $0.toolName) }
                    )
                }
                result.append(summaries.createBoxServerSummary(
                    server: server,
                    box: box,
                    unavailable: runnerUnavailable
                ))
                continue
            }

            let identifier = server.serverIdentifier
            let entries = backend.filter { entry in
                guard let identifier else { return false }
                return backendEntryBelongsToRow(
                    rowServerIdentifier: entry.rowServerIdentifier,
                    rowIdentifier: identifier
                )
            }.map {
                McpBackendEntry(
                    accountLabel: $0.accountLabel,
                    serverIdentifier: $0.serverIdentifier,
                    status: $0.status,
                    tools: $0.tools.map { .init(toolName: $0.toolName) }
                )
            }
            result.append(contentsOf: summaries.createBackendServerSummaries(
                server: server,
                entries: entries
            ))
        }

        let state = McpListedState(servers: result)
        lastState = state
        return state
    }

    func listConnectedBackendTools() async throws -> [NamedBackendTool] {
        _ = try await listServers()
        return lastBackendTools
    }

    func getTools() async throws -> [McpDiscoveredTool] {
        try await deps.toolsDiscovery.getTools()
    }

    func getToolsForTurnStart() async -> [McpDiscoveredTool] {
        await deps.toolsDiscovery.getToolsForTurnStart()
    }

    func executeTool(
        _ request: McpToolExecutionRequest,
        auditIdentity: McpToolAuditIdentity? = nil
    ) async -> SandMcpResult {
        await deps.toolsDiscovery.executeTool(request, auditIdentity: auditIdentity)
    }

    func authenticateServer(
        _ serverId: String,
        accountKey: String = DEFAULT_MCP_ACCOUNT_KEY,
        requestingAgentId: String? = nil,
        forceReauth: Bool = false,
        trigger: String? = nil
    ) async throws -> McpAuthStartResult {
        try await authLifecycle().authenticateServer(
            serverId,
            accountKey: accountKey,
            requestingAgentId: requestingAgentId,
            forceReauth: forceReauth,
            trigger: trigger
        )
    }

    func noteAuthCompletedElsewhere(
        serverId: String,
        accountKey: String
    ) async -> String? {
        await authLifecycle().noteAuthCompletedElsewhere(
            serverId: serverId,
            accountKey: accountKey
        )
    }

    func handleOAuthCallback(_ url: URL) async -> McpOAuthCallbackOutcome {
        let outcome = await oauthCallbacks.handleCallback(url)
        if outcome == .success, let authWatches {
            await authWatches.pollAllPendingAuthWatches()
        }
        return outcome
    }

    func sceneEnteredBackground() async {
        if let authWatches {
            await authWatches.sceneEnteredBackground()
        }
    }

    func sceneBecameActive() async {
        await oauthCallbacks.expirePending()
        if let authWatches {
            await authWatches.sceneBecameActive()
        }
    }

    func refreshAccountConfigInBackground() async {
        await deps.definitionSource.refreshInBackground()
    }

    func reload() async {
        await deps.definitionSource.clearCache()
        await deps.toolsDiscovery.invalidateToolsCache()
        await deps.toolsDiscovery.resetPushState()
    }

    func reloadServers(removedServerId: String? = nil) async throws -> McpListedState {
        await reload()
        if let removedServerId, let display = lastDisplay {
            lastDisplay = .init(
                servers: display.servers.filter { $0.id != removedServerId },
                cacheScope: display.cacheScope,
                unavailable: display.unavailable,
                unresolvedServerIds: display.unresolvedServerIds.filter {
                    $0 != removedServerId
                }
            )
            if let lastDisplay {
                await deps.definitionSource.adoptAccountConfig(
                    runtimeConfigFromDisplay(lastDisplay)
                )
                await deps.toolsDiscovery.setAccountDisplay(lastDisplay)
            }
        }
        return try await listServers()
    }

    func setServerCustomInstructions(
        serverId rawId: String,
        instructions: String
    ) async throws -> McpListedState {
        let serverId = try validateMcpServerId(rawId)
        guard let server = try await resolveDisplayServer(serverId) else {
            throw SandMcpConfigError("MCP server not found.")
        }
        let sameNameCount = lastDisplay?.servers.filter {
            $0.name == server.name
        }.count ?? 0
        deps.settingsStore.setMcpCustomInstructionByServerId(
            serverId: serverId,
            displayName: server.name,
            value: instructions,
            mirrorLegacyName: sameNameCount <= 1
        )
        return try await listServers()
    }

    func listServerTools(_ rawId: String) async throws -> [McpToolListing] {
        let serverId = try validateMcpServerId(rawId)
        guard let server = try await resolveDisplayServer(serverId) else {
            throw SandMcpConfigError("MCP server not found.")
        }
        let disabled = Set(
            deps.settingsStore.getMcpDisabledToolsByServerId()[serverId] ?? []
        )
        let tools = try await deps.toolsDiscovery.getToolsRaw()
        var seen = Set<String>()
        return tools.compactMap { tool in
            guard let row = server.serverIdentifier,
                  displayRowOwnsIdentifier(
                    tool.providerIdentifier,
                    rowIdentifier: row,
                    slots: server.accounts
                  ),
                  seen.insert(tool.toolName).inserted else {
                return nil
            }
            return .init(
                name: tool.toolName,
                description: tool.description,
                isDisabled: disabled.contains(tool.toolName)
            )
        }
    }

    func setMcpToolDisabled(
        serverId rawId: String,
        toolName: String,
        disabled: Bool
    ) async throws -> [McpToolListing] {
        let serverId = try validateMcpServerId(rawId)
        let toolName = toolName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !toolName.isEmpty else {
            throw SandMcpConfigError("MCP tool name is required.")
        }
        guard let server = try await resolveDisplayServer(
            serverId,
            requireFreshRead: deps.setToolDisabled != nil
        ) else {
            throw SandMcpConfigError("MCP server not found.")
        }

        if let setToolDisabled = deps.setToolDisabled,
           let serverIdentifier = server.serverIdentifier {
            let expectedScope = lastScope
            try await setToolDisabled(serverIdentifier, toolName, disabled)
            _ = try await loadDisplay(requireFreshRead: true)
            guard lastScope == expectedScope else {
                throw SandMcpConfigError(
                    "MCP tool update became stale because the account changed."
                )
            }
            await deps.toolsDiscovery.invalidateToolsCache()
            return try await listServerTools(serverId)
        }

        var all = deps.settingsStore.getMcpDisabledToolsByServerId()
        var current = all[serverId] ?? []
        if disabled {
            if !current.contains(toolName) { current.append(toolName) }
        } else {
            current.removeAll { $0 == toolName }
        }
        current.sort()
        all[serverId] = current
        deps.settingsStore.setMcpDisabledToolsByServerId(all)
        return try await listServerTools(serverId)
    }

    func toggleMcpToolDisabled(
        serverId rawId: String,
        toolName: String
    ) async throws -> [McpToolListing] {
        let current = try await listServerTools(rawId)
        guard let tool = current.first(where: { $0.name == toolName }) else {
            throw SandMcpConfigError("MCP tool not found.")
        }
        return try await setMcpToolDisabled(
            serverId: rawId,
            toolName: toolName,
            disabled: !tool.isDisabled
        )
    }

    func getMcpCustomInstructions() -> [String: String] {
        var result = deps.settingsStore.getMcpCustomInstructions()
        let byId = deps.settingsStore.getMcpCustomInstructionsByServerId()
        for server in lastDisplay?.servers ?? [] {
            guard let identifier = server.serverIdentifier else { continue }
            let instruction = resolveMcpCustomInstruction(
                server.name,
                storedInstruction: byId[server.id]
                    ?? deps.settingsStore.getRawMcpCustomInstruction(server.name)
            )
            result[identifier] = instruction
            for slot in server.accounts {
                if let slotIdentifier = slot.serverIdentifier {
                    result[slotIdentifier] = instruction
                }
            }
        }
        return result
    }

    func addServer(name rawName: String, configJson: String) async throws -> McpListedState {
        guard let writer = deps.accountMcpWriter else {
            throw SandMcpConfigError(
                "Managing MCP servers requires a signed-in account."
            )
        }
        guard let parse = deps.parseServerConfig else {
            throw SandMcpConfigError("MCP server configuration parser is unavailable.")
        }
        let name = try validateServerName(rawName)
        guard !BUILTIN_MCP_SERVER_NAMES.contains(name) else {
            throw SandMcpConfigError(
                "MCP server name \"\(name)\" is reserved for a built-in server."
            )
        }
        let config = try parseServerConfig(configJson, parse: parse)
        var current = try await writer.getConfigForEdit()
        current.config.mcpServers[name] = accountMcpServerConfig(from: config)
        try await writer.setConfig(
            current.config,
            serverIdsByName: current.serverIdsByName
        )
        return try await reloadServers()
    }

    func removeServer(_ rawId: String) async throws -> SandMcpRemoveResult {
        let id = try validateMcpServerId(rawId)
        guard let row = try await resolveDisplayServer(id) else {
            return .init(
                state: try await listServers(),
                removed: true
            )
        }
        guard let writer = deps.accountMcpWriter else {
            throw SandMcpConfigError(
                "Managing MCP servers requires a signed-in account."
            )
        }

        if let pluginId = row.pluginId,
           !row.isTeamServer,
           !row.managedByTeamPluginPolicy,
           !row.isRequired,
           let numeric = UInt64(pluginId) {
            try await writer.uninstallPlugin(pluginId: numeric)
        } else {
            var current = try await writer.getConfigForEdit()
            guard let numericId = UInt64(id),
                  let name = current.serverIdsByName.first(where: {
                    $0.value == numericId
                  })?.key else {
                let state = try await listServers()
                return .init(
                    state: state,
                    removed: !state.servers.contains { $0.id == id },
                    reason: state.servers.contains { $0.id == id }
                        ? "still-present"
                        : nil
                )
            }
            current.config.mcpServers.removeValue(forKey: name)
            current.serverIdsByName.removeValue(forKey: name)
            try await writer.setConfig(
                current.config,
                serverIdsByName: current.serverIdsByName
            )
        }

        if let authWatches {
            _ = await authWatches.clearPendingAuthWatchesForServer(id)
        }
        let sameNameCount = lastDisplay?.servers.filter {
            $0.name == row.name
        }.count ?? 1
        deps.settingsStore.deleteMcpCustomInstructionByServerId(
            serverId: id,
            displayName: row.name,
            deleteLegacyName: sameNameCount <= 1
        )
        var disabled = deps.settingsStore.getMcpDisabledToolsByServerId()
        disabled.removeValue(forKey: id)
        deps.settingsStore.setMcpDisabledToolsByServerId(disabled)

        let state = try await reloadServers(removedServerId: id)
        let remaining = state.servers.first { $0.id == id }
        return .init(
            state: state,
            removed: remaining == nil,
            reason: remaining == nil
                ? nil
                : (remaining?.isTeamServer == true ? "team-server" : "still-present")
        )
    }

    func logoutAccount(
        serverId: String,
        accountKey: String
    ) async throws -> McpListedState {
        let key = try normalizeAccountKey(accountKey)
        guard let server = try await resolveDisplayServer(serverId),
              server.config.transport != .stdio,
              let url = server.config.url else {
            throw SandMcpConfigError(
                "This connector uses a Remote Runner and has no local OAuth account."
            )
        }
        try await deps.backendMcpExec.logoutAccount(
            serverUrl: url,
            accountKey: key
        )
        if let authWatches {
            _ = await authWatches.clearPendingAuthWatch(
                serverId: serverId,
                accountKey: key
            )
        }
        return try await reloadServers()
    }

    func renameAccount(
        serverId: String,
        accountKey: String,
        newAccountKey: String
    ) async throws -> McpListedState {
        let id = try validateMcpServerId(serverId)
        let key = try normalizeAccountKey(accountKey)
        let next = try normalizeAccountKey(newAccountKey)
        guard let server = try await resolveDisplayServer(id),
              server.config.transport != .stdio else {
            throw SandMcpConfigError(
                "This connector uses a Remote Runner and has no local OAuth account."
            )
        }
        try await deps.backendMcpExec.renameAccount(
            serverId: id,
            accountKey: key,
            newAccountKey: next
        )
        if let authWatches {
            _ = await authWatches.clearPendingAuthWatchCancelled(
                serverId: id,
                accountKey: key
            )
        }
        return try await reloadServers()
    }

    func removeAccount(
        serverId: String,
        accountKey: String
    ) async throws -> McpListedState {
        let id = try validateMcpServerId(serverId)
        let key = try normalizeAccountKey(accountKey)
        guard let server = try await resolveDisplayServer(id),
              server.config.transport != .stdio else {
            throw SandMcpConfigError(
                "This connector uses a Remote Runner and has no local OAuth account."
            )
        }
        try await deps.backendMcpExec.deleteAccount(
            serverId: id,
            accountKey: key
        )
        if let authWatches {
            _ = await authWatches.clearPendingAuthWatchCancelled(
                serverId: id,
                accountKey: key
            )
        }
        return try await reloadServers()
    }

    func getCatalog(forceRefresh: Bool = false) async throws -> [SandMarketplacePluginView] {
        guard let catalog = deps.catalog else {
            throw SandMcpConfigError("MCP marketplace is unavailable.")
        }
        return try await catalog.getCatalog(forceRefresh: forceRefresh)
    }

    func resolvePluginLogo(_ url: String) async -> String? {
        guard let catalog = deps.catalog else { return nil }
        return await catalog.resolvePluginLogo(url)
    }

    func installEntry(
        entryId: String,
        values: [String: String] = [:],
        hasTeamConfiguredVariables: Bool = false
    ) async throws {
        guard let catalog = deps.catalog else {
            throw SandMcpConfigError("MCP marketplace is unavailable.")
        }
        try await catalog.installEntry(
            entryId: entryId,
            values: values,
            hasTeamConfiguredVariables: hasTeamConfiguredVariables
        )
        await reload()
    }

    func updatePluginInstall(
        pluginId: String,
        values: [String: String]
    ) async throws {
        guard let catalog = deps.catalog else {
            throw SandMcpConfigError("MCP marketplace is unavailable.")
        }
        try await catalog.updatePluginInstall(
            pluginId: pluginId,
            values: values
        )
        await reload()
    }

    func listEffectivePlugins() async throws -> [EffectiveUserPlugin] {
        try await deps.effectivePluginsProvider?() ?? []
    }

    func uninstallPlugin(_ rawPluginId: String) async throws -> McpListedState {
        let pluginId = try validateMarketplacePluginId(rawPluginId)
        guard let numeric = UInt64(pluginId),
              let writer = deps.accountMcpWriter else {
            throw SandMcpConfigError(
                "Managing MCP plugins requires a signed-in account."
            )
        }
        let rows = lastDisplay?.servers.filter { $0.pluginId == pluginId } ?? []
        if rows.contains(where: { $0.isRequired }) {
            throw SandMcpConfigError(
                "This plugin is required by the user's team and can't be uninstalled."
            )
        }
        let effective = (try? await listEffectivePlugins()) ?? []
        if effective.contains(where: {
            $0.pluginId == pluginId && $0.installMode == .teamRequired
        }) {
            throw SandMcpConfigError(
                "This plugin is required by the user's team and can't be uninstalled."
            )
        }

        try await writer.uninstallPlugin(pluginId: numeric)
        for row in rows where !row.isTeamServer {
            if let authWatches {
                _ = await authWatches.clearPendingAuthWatchesForServer(row.id)
            }
            var disabled = deps.settingsStore.getMcpDisabledToolsByServerId()
            disabled.removeValue(forKey: row.id)
            deps.settingsStore.setMcpDisabledToolsByServerId(disabled)
        }
        return try await reloadServers()
    }

    func dispose() async {
        if let authWatches {
            await authWatches.clearAllPendingAuthWatches()
        }
        await oauthCallbacks.dispose()
    }
}
