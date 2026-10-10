import Foundation

enum McpAuthStartStatus: String, Equatable, Sendable {
    case started
    case alreadyAuthenticated = "already-authenticated"
    case notConfigured = "not-configured"
    case notSupported = "not-supported"
    case unreachable
}

struct McpAuthStartResult: Equatable, Sendable {
    let status: McpAuthStartStatus
    let serverName: String
    var authorizationUrl: String? = nil
    var message: String? = nil
}

enum McpAuthCompletionOutcome: String, Equatable, Sendable {
    case completed
    case cancelled
}

struct McpAuthCompletion: Equatable, Sendable {
    let serverId: String
    let accountKey: String
    let serverName: String
    var serverIdentifier: String? = nil
    var requestingAgentId: String? = nil
    let outcome: McpAuthCompletionOutcome
}

struct McpConnectorAuthEvent: Equatable, Sendable {
    let phase: String
    let outcome: String
    let serverId: String
    var serverName: String? = nil
    var reason: String? = nil
    var reauth: Bool? = nil
}

struct SandMcpAuthWatchDependencies: @unchecked Sendable {
    let checkAuthStatus: @Sendable (
        _ serverId: String,
        _ oauthRedirectUri: String,
        _ forceReauth: Bool,
        _ accountKey: String
    ) async throws -> BackendMcpAuthStatus
    let validateTokens: @Sendable (
        _ targets: [BackendMcpTokenTarget]
    ) async -> [BackendMcpTokenValidation]
    let resolveDisplayServer: @Sendable (
        _ serverId: String,
        _ requireFreshRead: Bool
    ) async throws -> DisplayServer?
    let reload: @Sendable () async -> Void
    var registerOAuthCallback: (@Sendable (_ authorizationUrl: String, _ serverName: String) async -> Bool)? = nil
    var onConnectorAuth: (@Sendable (McpConnectorAuthEvent) -> Void)? = nil
    var nowMs: @Sendable () -> Int64 = {
        Int64(Date().timeIntervalSince1970 * 1_000)
    }
    var authWatchPollIntervalMs: Int = AUTH_WATCH_POLL_INTERVAL_MS
    var authWatchTimeoutMs: Int = AUTH_WATCH_TIMEOUT_MS
    var autoPollEnabled: Bool = true
}

actor SandMcpAuthWatchLifecycle {
    private struct Watch {
        let token: UUID
        let serverId: String
        let accountKey: String
        let serverName: String
        var serverIdentifier: String?
        let serverUrl: String
        var requestingAgentId: String?
        var suppressFirstPoll: Bool
        let expiresAtMs: Int64
        var isPolling: Bool
        var pollTask: Task<Void, Never>?
    }

    private let deps: SandMcpAuthWatchDependencies
    private var pending: [String: Watch] = [:]
    private var observer: (@Sendable (McpAuthCompletion) -> Void)?
    private var sceneActive = true

    init(deps: SandMcpAuthWatchDependencies) {
        self.deps = deps
    }

    func setAuthCompletionObserver(
        _ observer: (@Sendable (McpAuthCompletion) -> Void)?
    ) {
        self.observer = observer
    }

    func authenticateServer(
        _ rawId: String,
        accountKey rawKey: String = DEFAULT_MCP_ACCOUNT_KEY,
        requestingAgentId: String? = nil,
        forceReauth: Bool = false,
        trigger: String? = nil
    ) async throws -> McpAuthStartResult {
        let serverId = try validateMcpServerId(rawId)
        let accountKey = try normalizeAccountKey(rawKey)
        let server = try await deps.resolveDisplayServer(serverId, false)

        if trigger == "connector_card" {
            deps.onConnectorAuth?(.init(
                phase: "card_clicked",
                outcome: "ok",
                serverId: serverId,
                serverName: server?.name
            ))
        }

        guard let server else {
            emitRefused(reason: "not_configured", serverId: serverId)
            return .init(status: .notConfigured, serverName: serverId)
        }

        if server.disabledByTeamAdminPolicy {
            var blocked = false
            do {
                let fresh = try await deps.resolveDisplayServer(serverId, true)
                blocked = fresh == nil || fresh?.disabledByTeamAdminPolicy == true
            } catch {
                // Reference behavior only refuses after a fresh policy read
                // confirms the block. A stale row alone is insufficient.
                blocked = false
            }

            if blocked {
                let watches = clearPendingAuthWatchesForServerInternal(serverId)
                await deps.reload()
                for watch in watches {
                    notifyCancelled(watch)
                }
                emitRefused(
                    reason: "admin_blocked",
                    serverId: serverId,
                    serverName: server.name
                )
                return .init(
                    status: .notSupported,
                    serverName: server.name,
                    message: "This connector is disabled by your team admin's MCP policy, so it can't be authenticated."
                )
            }
        }

        if server.config.transport == .stdio {
            emitRefused(
                reason: "stdio_unsupported",
                serverId: serverId,
                serverName: server.name
            )
            return .init(
                status: .notSupported,
                serverName: server.name,
                message: "This connector uses a Runner-managed stdio transport and does not use browser sign-in on iOS."
            )
        }

        let status: BackendMcpAuthStatus
        do {
            status = try await deps.checkAuthStatus(
                serverId,
                MCP_OAUTH_IOS_CALLBACK_URL,
                forceReauth,
                accountKey
            )
        } catch {
            deps.onConnectorAuth?(.init(
                phase: "flow_started",
                outcome: "failed",
                serverId: serverId,
                serverName: server.name,
                reason: isTimeout(error) ? "rpc_timeout" : "probe_failed"
            ))
            throw error
        }

        if !forceReauth &&
            (status.hasValidToken || (status.isAvailable && !status.requiresAuth)) {
            await deps.reload()
            return .init(
                status: .alreadyAuthenticated,
                serverName: server.name
            )
        }

        if status.requiresAuth, !status.authUrl.isEmpty {
            guard let authorizationUrl = validateAuthorizationUrl(
                status.authUrl,
                serverUrl: server.config.url
            ) else {
                if forceReauth {
                    _ = clearPendingAuthWatchInternal(serverId, accountKey, emitCancelled: true)
                    await deps.reload()
                }
                emitRefused(
                    reason: "invalid_auth_url",
                    serverId: serverId,
                    serverName: server.name
                )
                return .init(
                    status: .notSupported,
                    serverName: server.name,
                    message: "Only HTTPS authentication URLs are supported unless both the connector and authentication endpoint are loopback URLs."
                )
            }

            if let registerOAuthCallback = deps.registerOAuthCallback {
                guard await registerOAuthCallback(authorizationUrl, server.name) else {
                    if forceReauth {
                        _ = clearPendingAuthWatchInternal(serverId, accountKey, emitCancelled: true)
                        await deps.reload()
                    }
                    emitRefused(
                        reason: "invalid_auth_url",
                        serverId: serverId,
                        serverName: server.name
                    )
                    return .init(
                        status: .notSupported,
                        serverName: server.name,
                        message: "The connector sign-in URL did not target Fabushi's registered iOS OAuth callback."
                    )
                }
            }

            let slotIdentifier = server.accounts.first {
                $0.accountKey == accountKey
            }?.serverIdentifier ?? server.serverIdentifier.map {
                provisionalMcpAccountServerIdentifier($0, accountKey: accountKey)
            }

            beginPendingAuthWatchInternal(
                serverId: serverId,
                accountKey: accountKey,
                serverName: server.name,
                serverIdentifier: slotIdentifier,
                serverUrl: server.config.url ?? "",
                requestingAgentId: requestingAgentId,
                forceReauth: forceReauth
            )
            deps.onConnectorAuth?(.init(
                phase: "flow_started",
                outcome: "ok",
                serverId: serverId,
                serverName: server.name,
                reauth: forceReauth
            ))
            return .init(
                status: .started,
                serverName: server.name,
                authorizationUrl: authorizationUrl
            )
        }

        if forceReauth {
            _ = clearPendingAuthWatchInternal(serverId, accountKey, emitCancelled: true)
            await deps.reload()
        }

        let detail = stripMarkupAndBoundConnectorError(status.error)
        if !status.isAvailable && !status.requiresAuth {
            emitRefused(
                reason: "unreachable",
                serverId: serverId,
                serverName: server.name
            )
            return .init(
                status: .unreachable,
                serverName: server.name,
                message: detail.isEmpty ? "The connector reported no details." : detail
            )
        }

        emitRefused(
            reason: "no_auth_link",
            serverId: serverId,
            serverName: server.name
        )
        return .init(
            status: .notSupported,
            serverName: server.name,
            message: detail.isEmpty
                ? "This connector did not provide a sign-in link."
                : detail
        )
    }

    func beginPendingAuthWatch(
        serverId: String,
        accountKey: String,
        serverName: String,
        serverIdentifier: String?,
        serverUrl: String,
        requestingAgentId: String?,
        forceReauth: Bool
    ) throws {
        let id = try validateMcpServerId(serverId)
        let key = try normalizeAccountKey(accountKey)
        beginPendingAuthWatchInternal(
            serverId: id,
            accountKey: key,
            serverName: serverName,
            serverIdentifier: serverIdentifier,
            serverUrl: serverUrl,
            requestingAgentId: requestingAgentId,
            forceReauth: forceReauth
        )
    }

    private func beginPendingAuthWatchInternal(
        serverId: String,
        accountKey: String,
        serverName: String,
        serverIdentifier: String?,
        serverUrl: String,
        requestingAgentId: String?,
        forceReauth: Bool
    ) {
        let key = authWatchKey(serverId, accountKey)
        let priorAgent = pending[key]?.requestingAgentId
        _ = clearPendingAuthWatchInternal(serverId, accountKey, emitCancelled: true)

        let token = UUID()
        var watch = Watch(
            token: token,
            serverId: serverId,
            accountKey: accountKey,
            serverName: serverName,
            serverIdentifier: serverIdentifier,
            serverUrl: serverUrl,
            requestingAgentId: requestingAgentId ?? priorAgent,
            suppressFirstPoll: forceReauth,
            expiresAtMs: deps.nowMs() + Int64(deps.authWatchTimeoutMs),
            isPolling: false,
            pollTask: nil
        )

        if deps.autoPollEnabled && sceneActive {
            watch.pollTask = makePollingTask(key: key)
        }
        pending[key] = watch
    }

    func clearPendingAuthWatch(
        serverId: String,
        accountKey: String
    ) -> McpAuthWatchReference? {
        clearPendingAuthWatchInternal(serverId, accountKey, emitCancelled: false)
            .map(reference)
    }

    func clearPendingAuthWatchCancelled(
        serverId: String,
        accountKey: String
    ) -> McpAuthWatchReference? {
        clearPendingAuthWatchInternal(serverId, accountKey, emitCancelled: true)
            .map(reference)
    }

    func noteAuthCompletedElsewhere(
        serverId rawId: String,
        accountKey rawKey: String
    ) -> String? {
        guard let id = try? validateMcpServerId(rawId),
              let key = try? normalizeAccountKey(rawKey) else { return nil }
        let watch = clearPendingAuthWatchInternal(id, key, emitCancelled: false)
        return watch?.requestingAgentId
    }

    func clearPendingAuthWatchesForServer(
        _ serverId: String
    ) -> [McpAuthWatchReference] {
        clearPendingAuthWatchesForServerInternal(serverId).map(reference)
    }

    func notifyWatchCancelled(_ reference: McpAuthWatchReference) {
        observer?(.init(
            serverId: reference.serverId,
            accountKey: reference.accountKey,
            serverName: reference.serverName ?? reference.serverId,
            serverIdentifier: reference.serverIdentifier,
            requestingAgentId: reference.requestingAgentId,
            outcome: .cancelled
        ))
    }

    func clearAllPendingAuthWatches() {
        for watch in pending.values {
            watch.pollTask?.cancel()
        }
        pending.removeAll()
    }

    func clearAllPendingAuthWatchesCancelled() {
        let watches = Array(pending.values)
        clearAllPendingAuthWatches()
        for watch in watches {
            emitCancelled(watch)
        }
    }

    func pendingWatchCount() -> Int {
        pending.count
    }

    func activeAutoPollTaskCount() -> Int {
        pending.values.filter { $0.pollTask != nil }.count
    }

    /// iOS may suspend immediately after entering background. Stop all interval
    /// work here; pending auth state remains in memory and is checked on resume.
    func sceneEnteredBackground() {
        sceneActive = false
        for key in Array(pending.keys) {
            guard var watch = pending[key] else { continue }
            watch.pollTask?.cancel()
            watch.pollTask = nil
            pending[key] = watch
        }
    }

    /// Resume with an immediate validation pass before restarting foreground-only
    /// interval polling. This closes the completion gap after iOS suspension.
    func sceneBecameActive() async {
        sceneActive = true
        await pollAllPendingAuthWatches()
        for key in Array(pending.keys) {
            guard var watch = pending[key],
                  watch.pollTask == nil,
                  deps.autoPollEnabled else { continue }
            watch.pollTask = makePollingTask(key: key)
            pending[key] = watch
        }
    }

    func pollAllPendingAuthWatches() async {
        for key in Array(pending.keys) {
            await pollPendingAuthWatch(key: key)
        }
    }

    func pollPendingAuthWatch(
        serverId: String,
        accountKey: String
    ) async {
        await pollPendingAuthWatch(key: authWatchKey(serverId, accountKey))
    }

    private func pollPendingAuthWatch(key: String) async {
        guard var watch = pending[key], !watch.isPolling else { return }

        if watch.suppressFirstPoll {
            watch.suppressFirstPoll = false
            pending[key] = watch
            return
        }

        if deps.nowMs() >= watch.expiresAtMs {
            _ = clearPendingAuthWatchInternal(
                watch.serverId,
                watch.accountKey,
                emitCancelled: false
            )
            deps.onConnectorAuth?(.init(
                phase: "token_stored",
                outcome: "timeout",
                serverId: watch.serverId,
                serverName: watch.serverName,
                reason: "auth_abandoned"
            ))
            return
        }

        watch.isPolling = true
        pending[key] = watch
        let token = watch.token

        let serverUrl = watch.serverUrl.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !serverUrl.isEmpty else {
            markPollingFinished(key: key, token: token)
            return
        }

        let results = await deps.validateTokens([
            .init(serverUrl: serverUrl, accountKey: watch.accountKey)
        ])
        guard pending[key]?.token == token else { return }

        let landed = results.contains {
            $0.serverUrl == serverUrl
                && normalizeMcpAccountLabel($0.accountKey ?? DEFAULT_MCP_ACCOUNT_KEY)
                    == watch.accountKey
                && $0.hasValidToken
        }
        guard landed else {
            markPollingFinished(key: key, token: token)
            return
        }

        var current: DisplayServer?
        var freshReadFailed = false
        do {
            current = try await deps.resolveDisplayServer(watch.serverId, true)
        } catch {
            freshReadFailed = true
        }
        guard pending[key]?.token == token else { return }

        if !freshReadFailed,
           current == nil || current?.disabledByTeamAdminPolicy == true {
            _ = clearPendingAuthWatchInternal(
                watch.serverId,
                watch.accountKey,
                emitCancelled: true
            )
            await deps.reload()
            notifyCancelled(watch)
            return
        }

        _ = clearPendingAuthWatchInternal(
            watch.serverId,
            watch.accountKey,
            emitCancelled: false
        )
        notifyCompleted(watch)
        await deps.reload()
    }

    private func makePollingTask(key: String) -> Task<Void, Never> {
        let interval = max(1, deps.authWatchPollIntervalMs)
        return Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(interval))
                guard !Task.isCancelled else { return }
                await self?.pollPendingAuthWatch(key: key)
            }
        }
    }

    private func markPollingFinished(key: String, token: UUID) {
        guard var watch = pending[key], watch.token == token else { return }
        watch.isPolling = false
        pending[key] = watch
    }

    private func clearPendingAuthWatchInternal(
        _ serverId: String,
        _ accountKey: String,
        emitCancelled shouldEmitCancelled: Bool
    ) -> Watch? {
        let key = authWatchKey(serverId, accountKey)
        guard let watch = pending.removeValue(forKey: key) else { return nil }
        watch.pollTask?.cancel()
        if shouldEmitCancelled {
            emitCancelled(watch)
        }
        return watch
    }

    private func clearPendingAuthWatchesForServerInternal(
        _ serverId: String
    ) -> [Watch] {
        let watches = pending.values.filter { $0.serverId == serverId }
        for watch in watches {
            _ = clearPendingAuthWatchInternal(
                watch.serverId,
                watch.accountKey,
                emitCancelled: true
            )
        }
        return watches
    }

    private func emitRefused(
        reason: String,
        serverId: String,
        serverName: String? = nil
    ) {
        deps.onConnectorAuth?(.init(
            phase: "flow_started",
            outcome: "failed",
            serverId: serverId,
            serverName: serverName,
            reason: reason
        ))
    }

    private func emitCancelled(_ watch: Watch) {
        deps.onConnectorAuth?(.init(
            phase: "token_stored",
            outcome: "cancelled",
            serverId: watch.serverId,
            serverName: watch.serverName
        ))
    }

    private func notifyCancelled(_ watch: Watch) {
        observer?(.init(
            serverId: watch.serverId,
            accountKey: watch.accountKey,
            serverName: watch.serverName,
            serverIdentifier: watch.serverIdentifier,
            requestingAgentId: watch.requestingAgentId,
            outcome: .cancelled
        ))
    }

    private func notifyCompleted(_ watch: Watch) {
        deps.onConnectorAuth?(.init(
            phase: "token_stored",
            outcome: "ok",
            serverId: watch.serverId,
            serverName: watch.serverName
        ))
        observer?(.init(
            serverId: watch.serverId,
            accountKey: watch.accountKey,
            serverName: watch.serverName,
            serverIdentifier: watch.serverIdentifier,
            requestingAgentId: watch.requestingAgentId,
            outcome: .completed
        ))
    }

    private func reference(_ watch: Watch) -> McpAuthWatchReference {
        .init(
            serverId: watch.serverId,
            accountKey: watch.accountKey,
            serverName: watch.serverName,
            serverIdentifier: watch.serverIdentifier,
            requestingAgentId: watch.requestingAgentId
        )
    }

    private func isTimeout(_ error: Error) -> Bool {
        (error as? URLError)?.code == .timedOut
    }
}
