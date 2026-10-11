import Foundation

struct GatewayTraceContext: Equatable, Sendable {
    let requestId: String
    let attempt: Int
    let rootTraceparent: String
    let traceparent: String
}

enum GatewayTraceParent {
    static func makeRoot() -> String {
        let traceId = randomHex(count: 32)
        let spanId = randomHex(count: 16)
        return "00-\(traceId)-\(spanId)-01"
    }

    static func makeChild(root: String) -> String {
        let parts = root.split(separator: "-", omittingEmptySubsequences: false)
        guard parts.count == 4,
              parts[0].count == 2,
              parts[1].count == 32,
              parts[2].count == 16,
              parts[3].count == 2
        else {
            return root
        }
        return "\(parts[0])-\(parts[1])-\(randomHex(count: 16))-\(parts[3])"
    }

    private static func randomHex(count: Int) -> String {
        let value = UUID().uuidString
            .replacingOccurrences(of: "-", with: "")
            .lowercased()
        return String(value.prefix(count))
    }
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

    static func shouldRetry(method: String, error: Error) -> Bool {
        guard isRetrySafe(method) else { return false }
        if error is GatewayCommandError { return false }
        if let error = error as? GatewayUnreachableError {
            return GatewayReachability.isRetryableTransportKind(
                error.transportKind
            )
        }
        return GatewayReachability.classifyOutcome(error)?.retryable == true
    }
}

private struct GatewayRequestFingerprint: Equatable {
    let method: String
    let args: CoordinatorPayload
}

private func coordinatorGatewayClientNonce(
    method: String,
    args: CoordinatorPayload
) -> String? {
    guard case .object = args,
          let object = args.foundationValue as? [String: Any]
    else { return nil }

    if method == "feature.execute",
       let command = object["command"] as? [String: Any],
       command["type"] as? String == "chat.send" {
        return command["requestId"] as? String
    }
    if method == "sendPrompt" {
        return object["clientNonce"] as? String
    }
    return nil
}

@MainActor
final class CoordinatorGatewayClient {
    struct Connection: Equatable, Sendable {
        let baseURL: URL
        let headers: [String: String]
    }

    typealias CommandTransport = @MainActor (_ connection: Connection, _ method: String, _ args: CoordinatorPayload) async throws -> CoordinatorPayload
    typealias TracedCommandTransport = @MainActor (_ connection: Connection, _ method: String, _ args: CoordinatorPayload, _ trace: GatewayTraceContext) async throws -> CoordinatorPayload
    typealias TraceRootFactory = @MainActor () -> String

    private struct InFlightRequest {
        let token: String
        let fingerprint: GatewayRequestFingerprint
        let task: Task<CoordinatorPayload, Error>
    }

    private let supervisor: CoordinatorHostSupervisor
    private let transport: TracedCommandTransport
    private let dnsDiagnostics: GatewayDNSDiagnosticTracker
    private let traceRootFactory: TraceRootFactory
    private let transportStageRecorder: TransportStageRecorder?
    private var inFlight: [String: InFlightRequest] = [:]

    init(
        supervisor: CoordinatorHostSupervisor,
        dnsDiagnostics: GatewayDNSDiagnosticTracker = .init(),
        traceRootFactory: @escaping TraceRootFactory = { GatewayTraceParent.makeRoot() },
        transportStageRecorder: TransportStageRecorder? = nil,
        transport: @escaping CommandTransport
    ) {
        self.supervisor = supervisor
        self.dnsDiagnostics = dnsDiagnostics
        self.traceRootFactory = traceRootFactory
        self.transportStageRecorder = transportStageRecorder
        self.transport = { connection, method, args, _ in
            try await transport(connection, method, args)
        }
    }

    init(
        supervisor: CoordinatorHostSupervisor,
        dnsDiagnostics: GatewayDNSDiagnosticTracker = .init(),
        traceRootFactory: @escaping TraceRootFactory = { GatewayTraceParent.makeRoot() },
        transportStageRecorder: TransportStageRecorder? = nil,
        tracedTransport: @escaping TracedCommandTransport
    ) {
        self.supervisor = supervisor
        self.dnsDiagnostics = dnsDiagnostics
        self.traceRootFactory = traceRootFactory
        self.transportStageRecorder = transportStageRecorder
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
        guard !normalizedRequestId.isEmpty, normalizedRequestId.utf8.count <= 256 else {
            throw GatewayCommandError(message: "gateway requestId must be 1...256 bytes")
        }
        let fingerprint = GatewayRequestFingerprint(method: method, args: args)
        if let existing = inFlight[normalizedRequestId] {
            guard existing.fingerprint == fingerprint else {
                throw GatewayCommandError(
                    message: "gateway requestId was reused for a different command"
                )
            }
            return try await existing.task.value
        }

        let token = UUID().uuidString.lowercased()
        let task = Task { @MainActor [weak self] () throws -> CoordinatorPayload in
            guard let self else {
                throw GatewayUnreachableError(
                    transportKind: "coordinator",
                    message: "gateway client released before request completed"
                )
            }

            let rootTraceparent = self.traceRootFactory()
            let sendTrace: CoordinatorSendTrace?
            if let recorder = self.transportStageRecorder {
                sendTrace = await recorder.beginSend(
                    accountSlot: "host",
                    clientNonce: coordinatorGatewayClientNonce(method: method, args: args),
                    traceparent: rootTraceparent,
                    nowMonotonicMilliseconds: coordinatorTransportMonotonicMilliseconds()
                )
            } else {
                sendTrace = nil
            }
            var pinnedBaseURL: URL?
            var lastError: Error?

            for attempt in 1...GatewayRetryPolicy.maxAttempts {
                try Task.checkCancellation()
                let attemptIndex = attempt - 1
                var connectStage = sendTrace?.beginStage(
                    "gateway-connect",
                    attempt: attemptIndex,
                    startEpochMilliseconds: coordinatorTransportEpochMilliseconds(),
                    startMonotonicMilliseconds: coordinatorTransportMonotonicMilliseconds()
                )

                let connection: Connection
                do {
                    connection = try await self.supervisor.ensureConnection()
                    if let pinnedBaseURL, pinnedBaseURL != connection.baseURL {
                        if let report = connectStage?.fail(
                            nowMonotonicMilliseconds: coordinatorTransportMonotonicMilliseconds()
                        ), let recorder = self.transportStageRecorder {
                            await recorder.record(report: report)
                        }
                        throw GatewayUnreachableError(
                            transportKind: "coordinator",
                            message: "gateway endpoint changed while retrying request \(normalizedRequestId)"
                        )
                    }
                    if pinnedBaseURL == nil {
                        pinnedBaseURL = connection.baseURL
                    }
                    if let report = connectStage?.complete(
                        nowMonotonicMilliseconds: coordinatorTransportMonotonicMilliseconds()
                    ), let recorder = self.transportStageRecorder {
                        await recorder.record(report: report)
                    }
                } catch {
                    if let report = connectStage?.fail(
                        nowMonotonicMilliseconds: coordinatorTransportMonotonicMilliseconds()
                    ), let recorder = self.transportStageRecorder {
                        await recorder.record(report: report)
                    }
                    throw error
                }

                let trace = GatewayTraceContext(
                    requestId: normalizedRequestId,
                    attempt: attempt,
                    rootTraceparent: rootTraceparent,
                    traceparent: GatewayTraceParent.makeChild(root: rootTraceparent)
                )
                var headers = connection.headers
                headers["traceparent"] = trace.traceparent
                let tracedConnection = Connection(
                    baseURL: connection.baseURL,
                    headers: headers
                )
                var postStage = sendTrace?.beginStage(
                    "gateway-post",
                    attempt: attemptIndex,
                    startEpochMilliseconds: coordinatorTransportEpochMilliseconds(),
                    startMonotonicMilliseconds: coordinatorTransportMonotonicMilliseconds()
                )

                do {
                    let value = try await self.transport(
                        tracedConnection,
                        method,
                        args,
                        trace
                    )
                    if let report = postStage?.complete(
                        nowMonotonicMilliseconds: coordinatorTransportMonotonicMilliseconds()
                    ), let recorder = self.transportStageRecorder {
                        await recorder.record(report: report)
                    }
                    try Task.checkCancellation()
                    self.dnsDiagnostics.recordSuccess(url: connection.baseURL)
                    return value
                } catch is CancellationError {
                    if let report = postStage?.fail(
                        nowMonotonicMilliseconds: coordinatorTransportMonotonicMilliseconds()
                    ), let recorder = self.transportStageRecorder {
                        await recorder.record(report: report)
                    }
                    throw CancellationError()
                } catch {
                    if let report = postStage?.fail(
                        nowMonotonicMilliseconds: coordinatorTransportMonotonicMilliseconds()
                    ), let recorder = self.transportStageRecorder {
                        await recorder.record(report: report)
                    }
                    lastError = error
                    self.supervisor.invalidateHealthCache()
                    _ = self.dnsDiagnostics.recordFailure(
                        url: connection.baseURL,
                        requestId: normalizedRequestId
                    )
                    if attempt >= GatewayRetryPolicy.maxAttempts
                        || !GatewayRetryPolicy.shouldRetry(method: method, error: error) {
                        throw error
                    }
                }
            }

            throw lastError ?? GatewayUnreachableError(
                transportKind: "coordinator",
                message: "gateway command failed without an error"
            )
        }

        inFlight[normalizedRequestId] = .init(
            token: token,
            fingerprint: fingerprint,
            task: task
        )

        do {
            let value = try await task.value
            settleInFlight(requestId: normalizedRequestId, token: token)
            return value
        } catch {
            settleInFlight(requestId: normalizedRequestId, token: token)
            throw error
        }
    }

    func cancel(requestId: String) {
        let normalized = requestId.trimmingCharacters(in: .whitespacesAndNewlines)
        inFlight.removeValue(forKey: normalized)?.task.cancel()
    }

    private func settleInFlight(requestId: String, token: String) {
        guard inFlight[requestId]?.token == token else { return }
        inFlight.removeValue(forKey: requestId)
    }
}
