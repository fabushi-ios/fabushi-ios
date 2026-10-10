import Foundation

protocol McpAccountBackend: AnyObject {
    func logoutAccount(serverUrl: String, accountKey: String) async throws
    func renameAccount(serverId: Int32, accountKey: String, newAccountKey: String) async throws
    func deleteAccount(serverId: Int32, accountKey: String) async throws
}

struct McpListedState: Equatable, Sendable {
    var servers: [McpServerSummary]
}

final class SandMcpAccountSlotLifecycle {
    typealias ReloadServers = () async throws -> McpListedState
    typealias ClearPendingWatch = (String, String) async -> McpAuthWatchReference?
    typealias NotifyWatchCancelled = (McpAuthWatchReference) async -> Void

    private let backend: any McpAccountBackend
    private let resolveDisplayServer: (String) async -> DisplayServer?
    private let reloadServers: ReloadServers
    private let clearPendingAuthWatch: ClearPendingWatch
    private let notifyWatchCancelled: NotifyWatchCancelled
    private let getDisplay: () -> AccountDisplayConfig?
    private let setDisplay: (AccountDisplayConfig) -> Void
    private let getListedState: () -> McpListedState?
    private let setListedState: (McpListedState) -> Void
    private let adoptAccountConfig: (McpRuntimeConfig?) async -> Void
    private let invalidateToolsCache: () async -> Void
    private let resetPushState: () async -> Void

    init(
        backend: any McpAccountBackend,
        resolveDisplayServer: @escaping (String) async -> DisplayServer?,
        reloadServers: @escaping ReloadServers,
        clearPendingAuthWatch: @escaping ClearPendingWatch,
        notifyWatchCancelled: @escaping NotifyWatchCancelled,
        getDisplay: @escaping () -> AccountDisplayConfig?,
        setDisplay: @escaping (AccountDisplayConfig) -> Void,
        getListedState: @escaping () -> McpListedState?,
        setListedState: @escaping (McpListedState) -> Void,
        adoptAccountConfig: @escaping (McpRuntimeConfig?) async -> Void,
        invalidateToolsCache: @escaping () async -> Void,
        resetPushState: @escaping () async -> Void
    ) {
        self.backend = backend
        self.resolveDisplayServer = resolveDisplayServer
        self.reloadServers = reloadServers
        self.clearPendingAuthWatch = clearPendingAuthWatch
        self.notifyWatchCancelled = notifyWatchCancelled
        self.getDisplay = getDisplay
        self.setDisplay = setDisplay
        self.getListedState = getListedState
        self.setListedState = setListedState
        self.adoptAccountConfig = adoptAccountConfig
        self.invalidateToolsCache = invalidateToolsCache
        self.resetPushState = resetPushState
    }

    private func resolve(_ raw: String) async throws -> (serverId: String, serverUrl: String) {
        let serverId = try validateMcpServerId(raw)
        guard let server = await resolveDisplayServer(serverId) else {
            throw SandMcpConfigError("MCP server not found.")
        }
        guard server.config.transport != .stdio, let url = server.config.url else {
            throw SandMcpConfigError("This connector runs on Fabushi's computer and has no OAuth accounts.")
        }
        return (serverId, url)
    }

    func logoutAccount(_ id: String, key: String) async throws -> McpListedState {
        let accountKey = try normalizeAccountKey(key)
        let server = try await resolve(id)
        try await backend.logoutAccount(serverUrl: server.serverUrl, accountKey: accountKey)
        _ = await clearPendingAuthWatch(server.serverId, accountKey)
        return try await reloadServers()
    }

    func renameAccount(_ id: String, key: String, next: String) async throws -> McpListedState {
        let accountKey = try normalizeAccountKey(key)
        let newAccountKey = try normalizeAccountKey(next)
        let resolved = try await resolve(id)
        try await backend.renameAccount(
            serverId: try parseInt32McpServerId(resolved.serverId),
            accountKey: accountKey,
            newAccountKey: newAccountKey
        )
        if let watch = await clearPendingAuthWatch(resolved.serverId, accountKey) {
            await notifyWatchCancelled(watch)
        }
        if let committed = await commit(
            serverId: resolved.serverId,
            patchSlots: { slots in
                slots.map { slot in
                    slot.accountKey == accountKey
                        ? .init(accountKey: newAccountKey, hasToken: slot.hasToken)
                        : slot
                }
            },
            patchSummaries: { summaries, row in
                summaries.map { summary in
                    guard summary.id == resolved.serverId, summary.accountKey == accountKey else {
                        return summary
                    }
                    var patched = summary
                    patched.accountKey = newAccountKey
                    patched.serverIdentifier = provisionalMcpAccountServerIdentifier(row, accountKey: newAccountKey)
                    return patched
                }
            }
        ) {
            return committed
        }
        return try await reloadServers()
    }

    func removeAccount(_ id: String, key: String) async throws -> McpListedState {
        let accountKey = try normalizeAccountKey(key)
        let resolved = try await resolve(id)
        try await backend.deleteAccount(
            serverId: try parseInt32McpServerId(resolved.serverId),
            accountKey: accountKey
        )
        if let watch = await clearPendingAuthWatch(resolved.serverId, accountKey) {
            await notifyWatchCancelled(watch)
        }
        if let committed = await commit(
            serverId: resolved.serverId,
            patchSlots: { $0.filter { $0.accountKey != accountKey } },
            patchSummaries: { summaries, _ in
                summaries.filter { !($0.id == resolved.serverId && $0.accountKey == accountKey) }
            }
        ) {
            return committed
        }
        return try await reloadServers()
    }

    private func commit(
        serverId: String,
        patchSlots: ([McpDisplayAccountSlot]) -> [McpDisplayAccountSlot],
        patchSummaries: ([McpServerSummary], String) -> [McpServerSummary]
    ) async -> McpListedState? {
        guard let display = getDisplay(),
              let state = getListedState(),
              let row = display.servers.first(where: { $0.id == serverId }) else { return nil }
        let rowIdentifier = row.serverIdentifier ?? "mcp-row-\(row.id)"
        let accounts = patchSlots(row.accounts)
        guard !accounts.isEmpty else { return nil }

        let patchedDisplay = AccountDisplayConfig(
            servers: display.servers.map { server in
                guard server.id == serverId else { return server }
                var patched = server
                patched.accounts = accounts
                return patched
            },
            cacheScope: display.cacheScope,
            unavailable: display.unavailable,
            unresolvedServerIds: display.unresolvedServerIds
        )
        setDisplay(patchedDisplay)
        await adoptAccountConfig(runtimeConfigFromDisplay(patchedDisplay))
        await invalidateToolsCache()
        await resetPushState()

        let patchedState = McpListedState(
            servers: patchSummaries(state.servers, rowIdentifier)
        )
        setListedState(patchedState)
        return patchedState
    }
}
