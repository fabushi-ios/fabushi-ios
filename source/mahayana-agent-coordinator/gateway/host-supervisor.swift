import Foundation

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
