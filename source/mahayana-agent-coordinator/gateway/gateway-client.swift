import Foundation

struct GatewayTraceContext: Equatable, Sendable {
    let requestId: String
    let attempt: Int
}

enum GatewayRetryPolicy {
    static let maxAttempts = 2

    static func isRetrySafe(_ method: String) -> Bool {
        method.hasPrefix("get")
            || method.hasPrefix("list")
            || method == "feature.receive"
            || method == "coordinator.mcp.servers"
            || method == "coordinator.mcp.tools"
    }
}

@MainActor
final class CoordinatorGatewayClient {
    struct Connection: Equatable, Sendable {
        let baseURL: URL
        let headers: [String: String]
    }

    typealias CommandTransport = @MainActor (_ connection: Connection, _ method: String, _ args: CoordinatorPayload) async throws -> CoordinatorPayload
    typealias TracedCommandTransport = @MainActor (_ connection: Connection, _ method: String, _ args: CoordinatorPayload, _ trace: GatewayTraceContext) async throws -> CoordinatorPayload

    private let supervisor: CoordinatorHostSupervisor
    private let transport: TracedCommandTransport
    private let dnsDiagnostics: GatewayDNSDiagnosticTracker
    private var inFlight: [String: Task<CoordinatorPayload, Error>] = [:]

    init(
        supervisor: CoordinatorHostSupervisor,
        dnsDiagnostics: GatewayDNSDiagnosticTracker = .init(),
        transport: @escaping CommandTransport
    ) {
        self.supervisor = supervisor
        self.dnsDiagnostics = dnsDiagnostics
        self.transport = { connection, method, args, _ in
            try await transport(connection, method, args)
        }
    }

    init(
        supervisor: CoordinatorHostSupervisor,
        dnsDiagnostics: GatewayDNSDiagnosticTracker = .init(),
        tracedTransport: @escaping TracedCommandTransport
    ) {
        self.supervisor = supervisor
        self.dnsDiagnostics = dnsDiagnostics
        self.transport = tracedTransport
    }

    func dispatchCommand(method: String, args: CoordinatorPayload) async throws -> CoordinatorPayload {
        try await dispatchCommand(
            requestId: UUID().uuidString.lowercased(),
            method: method,
            args: args
        )
    }

    func dispatchCommand(
        requestId: String,
        method: String,
        args: CoordinatorPayload
    ) async throws -> CoordinatorPayload {
        let normalizedRequestId = requestId.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !normalizedRequestId.isEmpty else {
            throw GatewayCommandError(message: "gateway requestId must not be empty")
        }
        if let existing = inFlight[normalizedRequestId] {
            return try await existing.value
        }

        let task = Task { @MainActor [weak self] () throws -> CoordinatorPayload in
            guard let self else {
                throw GatewayUnreachableError(
                    transportKind: "coordinator",
                    message: "gateway client released before request completed"
                )
            }
            defer { self.inFlight.removeValue(forKey: normalizedRequestId) }
            var lastError: Error?
            for attempt in 1...GatewayRetryPolicy.maxAttempts {
                try Task.checkCancellation()
                let connection = try await self.supervisor.ensureConnection()
                do {
                    let value = try await self.transport(
                        connection,
                        method,
                        args,
                        .init(requestId: normalizedRequestId, attempt: attempt)
                    )
                    try Task.checkCancellation()
                    return value
                } catch is CancellationError {
                    throw CancellationError()
                } catch {
                    lastError = error
                    self.supervisor.invalidateHealthCache()
                    _ = self.dnsDiagnostics.recordFailure(
                        url: connection.baseURL,
                        requestId: normalizedRequestId
                    )
                    if attempt >= GatewayRetryPolicy.maxAttempts
                        || !GatewayRetryPolicy.isRetrySafe(method) {
                        throw error
                    }
                }
            }
            throw lastError ?? GatewayUnreachableError(
                transportKind: "coordinator",
                message: "gateway command failed without an error"
            )
        }
        inFlight[normalizedRequestId] = task
        return try await task.value
    }

    func cancel(requestId: String) {
        inFlight.removeValue(forKey: requestId)?.cancel()
    }
}
