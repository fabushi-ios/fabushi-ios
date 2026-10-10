import Foundation

struct McpDefinition: Equatable, Sendable {
    enum Source: String, Equatable, Sendable { case builtin, account }
    let identifier: String
    let serverConfig: McpServerConfig
    let source: Source
}

func backendEntryBelongsToRow(
    rowServerIdentifier: String?,
    rowIdentifier: String
) -> Bool {
    rowServerIdentifier == rowIdentifier
}

func displayRowOwnsIdentifier(
    _ identifier: String,
    rowIdentifier: String,
    slots: [McpDisplayAccountSlot] = []
) -> Bool {
    identifier == rowIdentifier || slots.contains { $0.serverIdentifier == identifier }
}

actor SandMcpDefinitionSource {
    typealias Provider = @Sendable () async throws -> McpRuntimeConfig?

    private let includeBuiltins: Bool
    private let provider: Provider?
    private let builtinProvider: @Sendable () -> [String: McpServerConfig]
    private var lastKnownConfig: McpRuntimeConfig?
    private var currentConfig: McpRuntimeConfig?
    private var epoch = 0

    init(
        includeBuiltins: Bool,
        provider: Provider? = nil,
        builtinProvider: @escaping @Sendable () -> [String: McpServerConfig] = { [:] }
    ) {
        self.includeBuiltins = includeBuiltins
        self.provider = provider
        self.builtinProvider = builtinProvider
    }

    func clearLastKnownAccountConfig() {
        epoch += 1
        lastKnownConfig = nil
        currentConfig = nil
    }

    func adoptAccountConfig(_ config: McpRuntimeConfig?) {
        epoch += 1
        lastKnownConfig = config
        currentConfig = config
    }

    func peekHttpServerNames() -> [String]? {
        peekNames(.http) + peekNames(.sse)
    }

    func peekStdioServerNames() -> [String]? {
        peekNames(.stdio)
    }

    private func peekNames(_ transport: McpTransport) -> [String] {
        guard let lastKnownConfig else { return [] }
        return lastKnownConfig.mcpServers.compactMap { name, config in
            !BUILTIN_MCP_SERVER_NAMES.contains(name) && config.transport == transport ? name : nil
        }.sorted()
    }

    func getStdioServerConfigs() async -> [String: McpServerConfig] {
        let users = await getUserServerConfigs()
        return users.filter { $0.value.transport == .stdio }
    }

    func clearCache() async {
        _ = await loadAccountConfig()
    }

    func ensureConfigLoaded() async {
        if await loadAccountConfig() != nil { return }
        if await loadAccountConfig() == nil, let lastKnownConfig {
            currentConfig = lastKnownConfig
        }
    }

    func refreshInBackground() {
        let expectedEpoch = epoch
        Task {
            guard let provider else { return }
            do {
                if let config = try await provider(), expectedEpoch == self.epoch {
                    self.lastKnownConfig = config
                    self.currentConfig = config
                }
            } catch {
                reportMcpHostEdgeFailure("definition-source-refresh", error: error)
            }
        }
    }

    func getUserServerConfigs() async -> [String: McpServerConfig] {
        if currentConfig == nil { _ = await loadAccountConfig() }
        let account = currentConfig ?? EMPTY_MCP_CONFIG
        return account.mcpServers.filter { !BUILTIN_MCP_SERVER_NAMES.contains($0.key) }
    }

    func getServerUrlForIdentifier(_ identifier: String) async -> String? {
        if currentConfig == nil { _ = await loadAccountConfig() }
        return (currentConfig ?? lastKnownConfig)?.mcpServers[identifier]?.url
    }

    func getDefinitions() async -> [McpDefinition] {
        let builtins = includeBuiltins ? builtinProvider() : [:]
        var result = builtins.map {
            McpDefinition(identifier: $0.key, serverConfig: $0.value, source: .builtin)
        }
        let users = await getUserServerConfigs()
        result.append(contentsOf: users.compactMap { identifier, config in
            guard builtins[identifier] == nil, config.transport != .stdio else { return nil }
            return .init(identifier: identifier, serverConfig: config, source: .account)
        })
        // Grok preserves category order: built-ins are projected first, then
        // account definitions. Do not globally sort the combined list because
        // that can move account rows ahead of built-in product surfaces.
        return result
    }

    @discardableResult
    private func loadAccountConfig() async -> McpRuntimeConfig? {
        guard let provider else {
            currentConfig = currentConfig ?? lastKnownConfig
            return currentConfig
        }
        epoch += 1
        let loadEpoch = epoch
        do {
            let config = try await provider()
            guard loadEpoch == epoch else { return currentConfig }
            currentConfig = config
            if let config { lastKnownConfig = config }
            return config
        } catch {
            reportMcpHostEdgeFailure("definition-source-load", error: error)
            return nil
        }
    }
}
