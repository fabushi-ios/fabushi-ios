import Foundation

let MCP_TOOLS_CACHE_TTL_MS = 24 * 60 * 60 * 1_000
let TOOLS_DISCOVERY_DEADLINE_MS = 120_000

struct McpDiscoveredTool: Equatable, Sendable {
    let providerIdentifier: String
    let name: String
    let toolName: String
    var description: String? = nil
    var inputSchema: McpJSONValue? = nil
}

struct RunnerMcpToolServer: Equatable, Sendable {
    let serverIdentifier: String
    let status: String
    var statusDetail: String? = nil
    let tools: [McpDiscoveredTool]

    var toolCount: Int { tools.count }
}

protocol RunnerMcpExecuting: Sendable {
    func loadServers(configJson: String) async throws
    func listTools(serverIdentifiers: [String]) async throws -> [RunnerMcpToolServer]
    func executeTool(
        providerIdentifier: String,
        toolName: String,
        args: McpJSONValue,
        toolCallId: String,
        agentId: String?
    ) async -> SandMcpResult
}

struct McpToolAuditIdentity: Equatable, Sendable {
    var agentId: String? = nil
}

struct McpToolExecutionRequest: Equatable, Sendable {
    let providerIdentifier: String
    let name: String
    let args: McpJSONValue
    let toolCallId: String
}

struct McpDiscoveryFailureEvent: Equatable, Sendable {
    let errorClass: String
    let elapsedMs: Int64
    let servedStale: Bool
}

struct McpFirstCallEvent: Equatable, Sendable {
    let providerIdentifier: String
    let ok: Bool
    var serverName: String? = nil
    var serverId: String? = nil
}

struct SandMcpToolsDiscoveryDependencies: @unchecked Sendable {
    let definitionSource: SandMcpDefinitionSource
    let settingsStore: SandSettingsStore
    let backendListTools: @Sendable ([String]) async throws -> [BackendMcpToolServer]
    let backendExecuteTool: @Sendable (
        _ providerIdentifier: String,
        _ toolName: String,
        _ args: McpJSONValue,
        _ toolCallId: String,
        _ agentId: String?
    ) async -> SandMcpResult
    var runnerMcpExec: (any RunnerMcpExecuting)? = nil
    var onFirstCall: (@Sendable (McpFirstCallEvent) -> Void)? = nil
    var onDiscoveryFailed: (@Sendable (McpDiscoveryFailureEvent) -> Void)? = nil
    var nowMs: @Sendable () -> Int64 = {
        Int64(Date().timeIntervalSince1970 * 1_000)
    }
}

private func mcpConfigJSONValue(_ config: McpServerConfig) -> [String: Any] {
    switch config.transport {
    case .stdio:
        var value: [String: Any] = [
            "type": "stdio",
            "command": config.command ?? "",
        ]
        if !config.args.isEmpty { value["args"] = config.args }
        if !config.env.isEmpty { value["env"] = config.env }
        return value
    case .sse:
        var value: [String: Any] = [
            "type": "sse",
            "url": config.url ?? "",
        ]
        if !config.headers.isEmpty { value["headers"] = config.headers }
        return value
    case .http:
        var value: [String: Any] = [
            "type": "http",
            "url": config.url ?? "",
        ]
        if !config.headers.isEmpty { value["headers"] = config.headers }
        return value
    }
}

private func mcpConfigJSON(_ configs: [String: McpServerConfig]) throws -> String {
    let object: [String: Any] = [
        "mcpServers": configs.mapValues(mcpConfigJSONValue),
    ]
    let data = try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
    guard let text = String(data: data, encoding: .utf8) else {
        throw SandMcpConfigError("MCP configuration could not be encoded as UTF-8 JSON.")
    }
    return text
}

private func mcpErrorText(_ value: McpJSONValue) -> String? {
    guard case .object(let object) = value else { return nil }
    if case .string(let message) = object["error"] { return message }
    if case .string(let message) = object["message"] { return message }
    return nil
}

private func mcpContentItem(_ value: McpJSONValue) -> SandMcpContentItem {
    guard case .object(let object) = value else {
        return .init(content: .other(caseName: "unknown", value: value))
    }

    let type: String? = {
        if case .string(let type) = object["type"] { return type }
        if case .string(let type) = object["case"] { return type }
        return nil
    }()

    switch type {
    case "text":
        if case .string(let text) = object["text"] {
            return .init(content: .text(.init(text: text)))
        }
    case "image":
        if case .string(let data) = object["data"],
           case .string(let mimeType) = object["mimeType"] {
            return .init(content: .image(.init(data: data, mimeType: mimeType)))
        }
    default:
        break
    }
    return .init(content: .other(caseName: type ?? "unknown", value: value))
}

func sandMcpResultFromBackend(_ result: McpExecResult) -> SandMcpResult {
    if result.caseName == "error" {
        return generatedMcpResultFactory.error(
            mcpErrorText(result.value) ?? "MCP tool execution failed."
        )
    }

    if result.caseName == "success" {
        guard case .object(let object) = result.value else {
            return .init(result: .success(.init(
                content: [],
                isError: false,
                structuredContent: result.value
            )))
        }
        let content: [SandMcpContentItem]
        if case .array(let items) = object["content"] {
            content = items.map(mcpContentItem)
        } else {
            content = []
        }
        let isError: Bool
        if case .bool(let value) = object["isError"] {
            isError = value
        } else {
            isError = false
        }
        return .init(result: .success(.init(
            content: content,
            isError: isError,
            structuredContent: object["structuredContent"]
        )))
    }

    return .init(result: .other(caseName: result.caseName, value: result.value))
}

func applyCustomInstructionsToMcpResult(
    _ result: SandMcpResult,
    serverName: String,
    rawInstructions: String,
    factory: any McpResultFactory = generatedMcpResultFactory
) -> SandMcpResult {
    let instructions = rawInstructions.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !instructions.isEmpty,
          case .success(let success) = result.result else { return result }
    let note = factory.textItem(
        formatMcpCustomInstructionToolNote(
            serverName,
            instructions: instructions
        )
    )
    return factory.success(result, content: [note] + success.content)
}

actor SandMcpToolsDiscovery {
    private struct CacheResolution: Sendable {
        let tools: [McpDiscoveredTool]
        let resolvedKey: String
    }

    private struct FulfilledCache: Sendable {
        let tools: [McpDiscoveredTool]
        let resolvedKey: String
        let atMs: Int64
    }

    private struct CacheEntry {
        let requestedKey: String
        let promise: Task<CacheResolution, Error>
        var fulfilled: FulfilledCache?
        var staleTools: [McpDiscoveredTool]?
    }

    private let deps: SandMcpToolsDiscoveryDependencies
    private var runnerMcpExec: (any RunnerMcpExecuting)?
    private var accountDisplay: AccountDisplayConfig?
    private var lastPushedRunnerConfigJson: String?
    private var hasEverPushedRunnerConfig = false
    private var toolsCacheEntry: CacheEntry?
    private var toolsCacheEpoch = 0
    private var coldWarmScheduled = false
    private var firstCallReported: [String: (ok: Bool, failed: Bool)] = [:]

    init(deps: SandMcpToolsDiscoveryDependencies) {
        self.deps = deps
        runnerMcpExec = deps.runnerMcpExec
    }

    func setAccountDisplay(_ display: AccountDisplayConfig?) {
        accountDisplay = display
    }

    func getTools() async throws -> [McpDiscoveredTool] {
        filterDisabledTools(try await getToolsRaw())
    }

    func getToolsRaw() async throws -> [McpDiscoveredTool] {
        let serverNames = await discoveryServerNames()
        let key = toolServerSetKey(serverNames)
        if key.isEmpty {
            dropSettledCacheForEmptyServerSet()
            return []
        }

        if let entry = toolsCacheEntry {
            if let fulfilled = entry.fulfilled,
               fulfilled.resolvedKey == key {
                if deps.nowMs() - fulfilled.atMs >= Int64(MCP_TOOLS_CACHE_TTL_MS) {
                    _ = startToolsResolution(requestedKey: key, carryStale: true)
                }
                return fulfilled.tools
            }

            if entry.requestedKey == key {
                if let stale = entry.staleTools { return stale }
                return try await finishResolution(entry.promise, expectedKey: key)
            }
        }

        let entry = startToolsResolution(requestedKey: key, carryStale: false)
        return try await finishResolution(entry.promise, expectedKey: key)
    }

    func getToolsForTurnStart() async -> [McpDiscoveredTool] {
        let serverNames = await discoveryServerNames()
        let key = toolServerSetKey(serverNames)
        guard !key.isEmpty else {
            dropSettledCacheForEmptyServerSet()
            return []
        }

        if !toolsEntryUsable(key) {
            _ = startToolsResolution(requestedKey: key, carryStale: true)
        }
        return filterDisabledTools(currentToolsForKey(key) ?? [])
    }

    func scheduleColdStartWarm() {
        guard !coldWarmScheduled else { return }
        coldWarmScheduled = true
        let epoch = toolsCacheEpoch
        Task { [weak self] in
            await self?.warmToolsCache(epoch: epoch)
            await self?.clearColdWarmScheduled()
        }
    }

    private func clearColdWarmScheduled() {
        coldWarmScheduled = false
    }

    private func warmToolsCache(epoch: Int) async {
        let names = await discoveryServerNames()
        guard epoch == toolsCacheEpoch else { return }
        let key = toolServerSetKey(names)
        guard !key.isEmpty else {
            dropSettledCacheForEmptyServerSet()
            return
        }
        if !toolsEntryUsable(key) {
            _ = startToolsResolution(requestedKey: key, carryStale: false)
        }
    }

    func listRunnerServers(
        serverIdentifiers: [String]
    ) async throws -> [RunnerMcpToolServer] {
        guard let runnerMcpExec else {
            throw SandMcpConfigError("This surface has no Runner MCP execution port.")
        }
        await deps.definitionSource.ensureConfigLoaded()
        try await ensureRunnerServersPushed()
        let servers = try await runnerMcpExec.listTools(
            serverIdentifiers: serverIdentifiers
        )
        return servers.map { server in
            guard let row = displayRowForIdentifier(server.serverIdentifier) else {
                return server
            }
            let enabled = countEnabledTools(serverId: row.id, tools: server.tools)
            guard enabled != server.tools.count else { return server }
            let disabled = Set(
                deps.settingsStore.getMcpDisabledToolsByServerId()[row.id] ?? []
            )
            return .init(
                serverIdentifier: server.serverIdentifier,
                status: server.status,
                statusDetail: server.statusDetail,
                tools: server.tools.filter { !disabled.contains($0.toolName) }
            )
        }
    }

    func resolveProviderTransport(
        _ providerIdentifier: String
    ) async -> McpTransport? {
        if await isHTTPProvider(providerIdentifier) {
            return accountDisplay?.servers.first {
                guard let rowIdentifier = $0.serverIdentifier else { return false }
                return displayRowOwnsIdentifier(
                    providerIdentifier,
                    rowIdentifier: rowIdentifier,
                    slots: $0.accounts
                )
            }?.config.transport ?? .http
        }
        if await isRunnerStdioProvider(providerIdentifier) { return .stdio }
        return nil
    }

    func executeTool(
        _ request: McpToolExecutionRequest,
        auditIdentity: McpToolAuditIdentity? = nil
    ) async -> SandMcpResult {
        let displayServer = displayRowForIdentifier(request.providerIdentifier)
        let displayName = displayServer?.name ?? request.providerIdentifier

        if let serverId = displayServer?.id,
           deps.settingsStore.getMcpDisabledToolsByServerId()[serverId]?
            .contains(request.name) == true {
            return generatedMcpResultFactory.error(
                "Tool \"\(request.name)\" is disabled for \"\(displayName)\"."
            )
        }

        let raw = await executeToolRaw(request, auditIdentity: auditIdentity)
        let stored = displayServer.flatMap {
            deps.settingsStore.getRawMcpCustomInstructionByServerId($0.id)
                ?? deps.settingsStore.getRawMcpCustomInstruction($0.name)
        }
        let instructions = resolveMcpCustomInstruction(
            displayName,
            storedInstruction: stored
        )
        return applyCustomInstructionsToMcpResult(
            raw,
            serverName: displayName,
            rawInstructions: instructions
        )
    }

    private func executeToolRaw(
        _ request: McpToolExecutionRequest,
        auditIdentity: McpToolAuditIdentity?
    ) async -> SandMcpResult {
        if await isHTTPProvider(request.providerIdentifier) {
            let result = await deps.backendExecuteTool(
                request.providerIdentifier,
                request.name,
                request.args,
                request.toolCallId,
                auditIdentity?.agentId
            )
            reportFirstCall(
                providerIdentifier: request.providerIdentifier,
                ok: {
                    if case .error = result.result { return false }
                    return true
                }()
            )
            return result
        }

        if await isRunnerStdioProvider(request.providerIdentifier) {
            guard let runnerMcpExec else {
                return generatedMcpResultFactory.error(
                    "MCP server \"\(request.providerIdentifier)\" requires a Remote Runner; iOS does not execute stdio MCP processes locally."
                )
            }
            do {
                try await ensureRunnerServersPushed()
            } catch {
                return generatedMcpResultFactory.error(
                    "Could not load MCP servers on the Remote Runner: \(error.localizedDescription)"
                )
            }
            return await runnerMcpExec.executeTool(
                providerIdentifier: request.providerIdentifier,
                toolName: request.name,
                args: request.args,
                toolCallId: request.toolCallId,
                agentId: auditIdentity?.agentId
            )
        }

        return generatedMcpResultFactory.error(
            "MCP server \"\(request.providerIdentifier)\" is not available here. HTTP/SSE servers execute through the backend and stdio servers require a Remote Runner."
        )
    }

    func setRunnerMcpExec(_ next: (any RunnerMcpExecuting)?) {
        runnerMcpExec = next
        lastPushedRunnerConfigJson = nil
        toolsCacheEntry = nil
        firstCallReported.removeAll()
        toolsCacheEpoch += 1
        Task { [weak self] in
            guard let self else { return }
            await self.warmToolsCache(epoch: await self.cacheEpoch())
        }
    }

    private func cacheEpoch() -> Int { toolsCacheEpoch }

    func invalidateToolsCache() {
        toolsCacheEpoch += 1
        toolsCacheEntry = nil
        firstCallReported.removeAll()
        let epoch = toolsCacheEpoch
        Task { [weak self] in await self?.warmToolsCache(epoch: epoch) }
    }

    func resetPushState() {
        lastPushedRunnerConfigJson = nil
    }

    func isRunnerExecWired() -> Bool {
        runnerMcpExec != nil
    }

    private func discoveryServerNames() async -> [String] {
        let configs = await deps.definitionSource.getUserServerConfigs()
        return configs.compactMap { name, config in
            switch config.transport {
            case .http, .sse:
                return name
            case .stdio:
                return runnerMcpExec == nil ? nil : name
            }
        }.sorted()
    }

    private func toolServerSetKey(_ names: [String]) -> String {
        names.sorted().joined(separator: "\0")
    }

    private func displayRowForIdentifier(_ identifier: String) -> DisplayServer? {
        accountDisplay?.servers.first { server in
            guard let row = server.serverIdentifier else { return false }
            return displayRowOwnsIdentifier(
                identifier,
                rowIdentifier: row,
                slots: server.accounts
            )
        }
    }

    private func filterDisabledTools(
        _ tools: [McpDiscoveredTool]
    ) -> [McpDiscoveredTool] {
        let disabled = deps.settingsStore.getMcpDisabledToolsByServerId()
        guard !disabled.isEmpty else { return tools }
        return tools.filter { tool in
            guard let row = displayRowForIdentifier(tool.providerIdentifier) else {
                return true
            }
            return !(disabled[row.id]?.contains(tool.toolName) ?? false)
        }
    }

    private func countEnabledTools(
        serverId: String,
        tools: [McpDiscoveredTool]
    ) -> Int {
        let disabled = Set(
            deps.settingsStore.getMcpDisabledToolsByServerId()[serverId] ?? []
        )
        guard !disabled.isEmpty else { return tools.count }
        return tools.filter { !disabled.contains($0.toolName) }.count
    }

    private func isHTTPProvider(_ identifier: String) async -> Bool {
        if let row = displayRowForIdentifier(identifier) {
            return row.config.transport == .http || row.config.transport == .sse
        }
        return await deps.definitionSource.getServerUrlForIdentifier(identifier) != nil
    }

    private func isRunnerStdioProvider(_ identifier: String) async -> Bool {
        let configs = await deps.definitionSource.getStdioServerConfigs()
        return configs[identifier] != nil
    }

    private func ensureRunnerServersPushed() async throws {
        guard let runnerMcpExec else { return }
        let configs = await deps.definitionSource.getStdioServerConfigs()
        let configJson = try mcpConfigJSON(configs)
        if configJson == lastPushedRunnerConfigJson { return }
        if configs.isEmpty && !hasEverPushedRunnerConfig { return }

        try await runnerMcpExec.loadServers(configJson: configJson)
        lastPushedRunnerConfigJson = configJson
        hasEverPushedRunnerConfig = true
    }

    private func fetchToolsViaPorts() async throws -> CacheResolution {
        let userServers = await deps.definitionSource.getUserServerConfigs()
        let currentRunnerMcpExec = runnerMcpExec
        let httpNames = userServers.compactMap { name, config in
            config.transport == .http || config.transport == .sse ? name : nil
        }.sorted()
        let stdioNames = currentRunnerMcpExec == nil ? [] : userServers.compactMap { name, config in
            config.transport == .stdio ? name : nil
        }.sorted()
        let resolvedKey = toolServerSetKey(httpNames + stdioNames)

        async let backendResult: Result<[McpDiscoveredTool], Error> = {
            guard !httpNames.isEmpty else { return .success([]) }
            do {
                let servers = try await deps.backendListTools(httpNames)
                return .success(servers.flatMap { server in
                    server.tools.map {
                        .init(
                            providerIdentifier: $0.providerIdentifier,
                            name: $0.name,
                            toolName: $0.toolName,
                            description: $0.description,
                            inputSchema: $0.inputSchema
                        )
                    }
                })
            } catch {
                return .failure(error)
            }
        }()

        async let runnerResult: Result<[McpDiscoveredTool], Error> = {
            guard !stdioNames.isEmpty, let runnerMcpExec = currentRunnerMcpExec else {
                return .success([])
            }
            do {
                try await ensureRunnerServersPushed()
                let servers = try await runnerMcpExec.listTools(
                    serverIdentifiers: stdioNames
                )
                return .success(servers.flatMap(\.tools))
            } catch {
                return .failure(error)
            }
        }()

        let backend = await backendResult
        let runner = await runnerResult

        switch (backend, runner) {
        case (.failure(let error), _):
            throw error
        case (.success(let http), .failure(let error)):
            guard !httpNames.isEmpty else { throw error }
            reportMcpHostEdgeFailure("runner-list-tools", error: error)
            return .init(tools: http, resolvedKey: resolvedKey)
        case (.success(let http), .success(let local)):
            return .init(tools: http + local, resolvedKey: resolvedKey)
        }
    }

    private func startToolsResolution(
        requestedKey: String,
        carryStale: Bool
    ) -> CacheEntry {
        let stale = carryStale ? currentToolsForKey(requestedKey) : nil
        let startedAt = deps.nowMs()
        let epoch = toolsCacheEpoch
        let task = Task<CacheResolution, Error> { [weak self] in
            guard let self else {
                throw CancellationError()
            }
            return try await self.fetchToolsViaPorts()
        }
        let entry = CacheEntry(
            requestedKey: requestedKey,
            promise: task,
            fulfilled: nil,
            staleTools: stale
        )
        toolsCacheEntry = entry

        Task { [weak self] in
            do {
                let resolution = try await task.value
                await self?.commitResolution(
                    resolution,
                    requestedKey: requestedKey,
                    epoch: epoch,
                    startedAt: startedAt,
                    stale: stale
                )
            } catch {
                await self?.commitResolutionFailure(
                    requestedKey: requestedKey,
                    epoch: epoch,
                    startedAt: startedAt,
                    stale: stale,
                    error: error
                )
            }
        }
        return entry
    }

    private func finishResolution(
        _ task: Task<CacheResolution, Error>,
        expectedKey: String
    ) async throws -> [McpDiscoveredTool] {
        let resolution = try await task.value
        guard resolution.resolvedKey == expectedKey else {
            return resolution.tools
        }
        return resolution.tools
    }

    private func commitResolution(
        _ resolution: CacheResolution,
        requestedKey: String,
        epoch: Int,
        startedAt: Int64,
        stale: [McpDiscoveredTool]?
    ) {
        guard epoch == toolsCacheEpoch,
              toolsCacheEntry?.requestedKey == requestedKey else { return }
        toolsCacheEntry?.fulfilled = .init(
            tools: resolution.tools,
            resolvedKey: resolution.resolvedKey,
            atMs: deps.nowMs()
        )
    }

    private func commitResolutionFailure(
        requestedKey: String,
        epoch: Int,
        startedAt: Int64,
        stale: [McpDiscoveredTool]?,
        error: Error
    ) {
        guard epoch == toolsCacheEpoch,
              toolsCacheEntry?.requestedKey == requestedKey else { return }

        if let stale {
            toolsCacheEntry?.fulfilled = .init(
                tools: stale,
                resolvedKey: requestedKey,
                atMs: 0
            )
        } else {
            toolsCacheEntry = nil
        }
        deps.onDiscoveryFailed?(.init(
            errorClass: String(describing: type(of: error)),
            elapsedMs: max(0, deps.nowMs() - startedAt),
            servedStale: stale != nil
        ))
    }

    private func currentToolsForKey(_ key: String) -> [McpDiscoveredTool]? {
        guard let entry = toolsCacheEntry else { return nil }
        if entry.fulfilled?.resolvedKey == key {
            return entry.fulfilled?.tools
        }
        if entry.requestedKey == key { return entry.staleTools }
        return nil
    }

    private func toolsEntryUsable(_ key: String) -> Bool {
        guard let entry = toolsCacheEntry else { return false }
        if let fulfilled = entry.fulfilled {
            return fulfilled.resolvedKey == key
                && deps.nowMs() - fulfilled.atMs < Int64(MCP_TOOLS_CACHE_TTL_MS)
        }
        return entry.requestedKey == key
    }

    private func dropSettledCacheForEmptyServerSet() {
        if toolsCacheEntry?.fulfilled != nil {
            toolsCacheEntry = nil
        }
    }

    private func reportFirstCall(
        providerIdentifier: String,
        ok: Bool
    ) {
        guard deps.onFirstCall != nil else { return }
        var state = firstCallReported[providerIdentifier] ?? (false, false)
        if ok ? state.ok : state.failed { return }
        if ok { state.ok = true } else { state.failed = true }
        firstCallReported[providerIdentifier] = state
        let row = displayRowForIdentifier(providerIdentifier)
        deps.onFirstCall?(.init(
            providerIdentifier: providerIdentifier,
            ok: ok,
            serverName: row?.name,
            serverId: row?.id
        ))
    }
}

struct SandMcpExecutor: Sendable {
    let discovery: SandMcpToolsDiscovery
    var persistImage: (@Sendable (String, String) async throws -> SavedMcpImage?)? = nil
    var spillLargeText: (@Sendable (SandMcpResult) async -> SandMcpResult)? = nil
    var auditIdentity: McpToolAuditIdentity? = nil

    func execute(_ request: McpToolExecutionRequest) async -> SandMcpResult {
        let result = await discovery.executeTool(
            request,
            auditIdentity: auditIdentity
        )
        let spilled = if let spillLargeText {
            await spillLargeText(result)
        } else {
            result
        }
        guard let persistImage else { return spilled }
        return await augmentMcpResultWithSavedImages(
            spilled,
            persistImage: persistImage
        )
    }
}
