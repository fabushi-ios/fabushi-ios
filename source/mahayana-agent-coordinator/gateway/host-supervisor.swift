import Foundation

@MainActor
final class IOSGatewayDescriptorKeychainCache {
    static let maxAgeMilliseconds: Int64 = 7 * 24 * 60 * 60 * 1_000

    private struct Envelope: Codable {
        let version: Int
        let accountScope: String
        let savedAtMilliseconds: Int64
        let connection: IOSCursorSandBoxConnection
    }

    typealias FailureReporter = @MainActor (_ operation: String, _ error: Error) -> Void

    private static let secretKey = "gateway-descriptor-v1"
    private let store: any IOSCursorCredentialStoring
    private let nowMilliseconds: () -> Int64
    private let reportFailure: FailureReporter
    private var lastWrittenScope: String?
    private var lastWrittenConnection: IOSCursorSandBoxConnection?

    init(
        store: any IOSCursorCredentialStoring = IOSCursorCredentialStore(
            service: "com.ombhrum.fabushi.gateway-descriptor"
        ),
        nowMilliseconds: @escaping () -> Int64 = {
            Int64((Date().timeIntervalSince1970 * 1_000).rounded())
        },
        reportFailure: @escaping FailureReporter = { _, _ in }
    ) {
        self.store = store
        self.nowMilliseconds = nowMilliseconds
        self.reportFailure = reportFailure
    }

    func read(accountScope: String) async -> IOSCursorSandBoxConnection? {
        do {
            guard let raw = try await store.readSecret(Self.secretKey),
                  let data = raw.data(using: .utf8)
            else { return nil }
            let envelope = try JSONDecoder().decode(Envelope.self, from: data)
            guard envelope.version == 1,
                  envelope.accountScope == accountScope,
                  nowMilliseconds() - envelope.savedAtMilliseconds <= Self.maxAgeMilliseconds
            else { return nil }
            return envelope.connection
        } catch {
            reportFailure("read", error)
            return nil
        }
    }

    func write(
        accountScope: String,
        connection: IOSCursorSandBoxConnection
    ) async {
        if lastWrittenScope == accountScope, lastWrittenConnection == connection {
            return
        }
        do {
            let data = try JSONEncoder().encode(Envelope(
                version: 1,
                accountScope: accountScope,
                savedAtMilliseconds: nowMilliseconds(),
                connection: connection
            ))
            guard let raw = String(data: data, encoding: .utf8) else {
                throw IOSCursorAuthError.invalidLoginResponse
            }
            try await store.writeSecret(Self.secretKey, value: raw)
            lastWrittenScope = accountScope
            lastWrittenConnection = connection
        } catch {
            reportFailure("write", error)
        }
    }

    func clear() async {
        lastWrittenScope = nil
        lastWrittenConnection = nil
        do {
            try await store.deleteSecret(Self.secretKey)
        } catch {
            reportFailure("clear", error)
        }
    }
}

@MainActor
final class IOSGatewayDescriptorFastPath {
    typealias Connection = IOSCursorSandBoxConnection
    typealias ResolveLive = @MainActor () async throws -> Connection
    typealias AccountScopeProvider = @MainActor () -> String?
    typealias FailureReporter = @MainActor (_ operation: String, _ error: Error) -> Void
    typealias BrokerDenialClassifier = @MainActor (_ error: Error) -> Bool

    private struct PendingRefresh {
        let id: UUID
        let accountScope: String?
        let task: Task<Connection, Error>
    }

    private let cache: IOSGatewayDescriptorKeychainCache
    private let getAccountScope: AccountScopeProvider
    private let resolveLive: ResolveLive
    private let reportFailure: FailureReporter
    private let isBrokerDenial: BrokerDenialClassifier
    private var hasLiveConnection = false
    private var liveAccountScope: String?
    private var pendingRefresh: PendingRefresh?
    private var reportedRefreshes = Set<UUID>()
    private var deniedScopes = Set<String>()

    init(
        cache: IOSGatewayDescriptorKeychainCache,
        getAccountScope: @escaping AccountScopeProvider,
        reportFailure: @escaping FailureReporter = { _, _ in },
        isBrokerDenial: @escaping BrokerDenialClassifier = { error in
            let message = (error as NSError).localizedDescription
            return message.contains("HTTP 401")
                || message.contains("HTTP 403")
                || message.contains(GATEWAY_NO_STORAGE_MESSAGE_MARKER)
        },
        resolveLive: @escaping ResolveLive
    ) {
        self.cache = cache
        self.getAccountScope = getAccountScope
        self.reportFailure = reportFailure
        self.isBrokerDenial = isBrokerDenial
        self.resolveLive = resolveLive
    }

    func resolve() async throws -> Connection {
        let accountScope = getAccountScope()
        if hasLiveConnection, liveAccountScope == accountScope {
            return try await resolveLiveConnection(for: accountScope)
        }

        let cached: Connection?
        if let accountScope, !deniedScopes.contains(accountScope) {
            cached = await cache.read(accountScope: accountScope)
        } else {
            cached = nil
        }

        let refresh = startRefresh(for: accountScope)
        guard let cached else {
            return try await refresh.task.value
        }

        if reportedRefreshes.insert(refresh.id).inserted {
            Task { @MainActor [weak self] in
                do {
                    _ = try await refresh.task.value
                } catch {
                    self?.reportFailure("refresh", error)
                }
                self?.reportedRefreshes.remove(refresh.id)
            }
        }
        return cached
    }

    private func startRefresh(for accountScope: String?) -> PendingRefresh {
        if let pendingRefresh, pendingRefresh.accountScope == accountScope {
            return pendingRefresh
        }
        let id = UUID()
        let task = Task { @MainActor [weak self] () throws -> Connection in
            guard let self else { throw CancellationError() }
            defer { self.settlePendingRefresh(id) }
            return try await self.resolveLiveConnection(for: accountScope)
        }
        let refresh = PendingRefresh(id: id, accountScope: accountScope, task: task)
        pendingRefresh = refresh
        return refresh
    }

    private func settlePendingRefresh(_ id: UUID) {
        guard pendingRefresh?.id == id else { return }
        pendingRefresh = nil
    }

    private func resolveLiveConnection(for accountScope: String?) async throws -> Connection {
        do {
            let connection = try await resolveLive()
            if getAccountScope() == accountScope {
                hasLiveConnection = true
                liveAccountScope = accountScope
                if let accountScope {
                    deniedScopes.remove(accountScope)
                    await cache.write(accountScope: accountScope, connection: connection)
                }
            }
            return connection
        } catch {
            if let accountScope,
               getAccountScope() == accountScope,
               isBrokerDenial(error) {
                deniedScopes.insert(accountScope)
                await cache.clear()
            }
            throw error
        }
    }
}

@MainActor
final class CoordinatorHostSupervisor {
    static let healthTimeoutMilliseconds = 1_500
    static let healthProbeTTLMilliseconds = 5_000

    typealias Connection = CoordinatorGatewayClient.Connection
    typealias ResolveConnection = @MainActor () async throws -> Connection
    typealias HealthProbe = @MainActor (Connection) async -> Bool

    private let resolveConnection: ResolveConnection
    private let healthProbe: HealthProbe
    private let now: () -> Date

    private var connection: Connection?
    private var lastHealthyAt: Date?
    private var pendingConnection: Task<Connection, Error>?
    private var pendingConnectionToken: String?
    private var healthEpoch: UInt64 = 0

    init(
        now: @escaping () -> Date = Date.init,
        resolveConnection: @escaping ResolveConnection,
        healthProbe: @escaping HealthProbe
    ) {
        self.now = now
        self.resolveConnection = resolveConnection
        self.healthProbe = healthProbe
    }

    func invalidateHealthCache() {
        healthEpoch &+= 1
        lastHealthyAt = nil
        pendingConnection?.cancel()
        pendingConnection = nil
        pendingConnectionToken = nil
    }

    func ensureConnection() async throws -> Connection {
        if let connection, let lastHealthyAt,
           now().timeIntervalSince(lastHealthyAt) * 1_000 < Double(Self.healthProbeTTLMilliseconds) {
            return connection
        }

        if let connection {
            let probeEpoch = healthEpoch
            let healthy = await healthProbe(connection)
            guard probeEpoch == healthEpoch else {
                return try await ensureConnection()
            }
            if healthy {
                lastHealthyAt = now()
                return connection
            }
        }

        let epoch = healthEpoch
        if let pendingConnection {
            let resolved = try await pendingConnection.value
            guard epoch == healthEpoch else {
                return try await ensureConnection()
            }
            return resolved
        }

        let token = UUID().uuidString.lowercased()
        let task = Task { @MainActor [resolveConnection] in
            try await resolveConnection()
        }
        pendingConnection = task
        pendingConnectionToken = token

        do {
            let resolved = try await task.value
            if pendingConnectionToken == token {
                pendingConnection = nil
                pendingConnectionToken = nil
            }
            guard epoch == healthEpoch else {
                return try await ensureConnection()
            }
            connection = resolved
            lastHealthyAt = nil
            return resolved
        } catch {
            if pendingConnectionToken == token {
                pendingConnection = nil
                pendingConnectionToken = nil
            }
            throw error
        }
    }
}
