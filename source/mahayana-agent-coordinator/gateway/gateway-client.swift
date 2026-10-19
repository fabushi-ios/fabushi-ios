import Foundation


struct GatewayTelemetryProjection: Equatable, Sendable {
    let event: String
    let level: String
    let metadata: [String: String]

    static func reachability(
        url: URL,
        outcome: GatewayReachabilityOutcome,
        requestId: String,
        attempt: Int
    ) -> GatewayTelemetryProjection {
        let diagnostic = GatewayDNSDiagnostics.inspect(url)
        var metadata: [String: String] = [
            "event": "gateway_reachability",
            "outcome": outcome.transportKind,
            "request_id": requestId,
            "attempt": String(attempt),
            "base_url_kind": GatewayReachability.classifyBaseURL(url).rawValue,
            "secure_transport": diagnostic.isSecureTransport ? "true" : "false",
        ]
        if let host = diagnostic.host {
            metadata["host_kind"] = diagnostic.kind.rawValue
            metadata["host"] = String(host.prefix(256))
        }
        return .init(
            event: "gateway_reachability",
            level: outcome == .reachable ? "info" : "warn",
            metadata: metadata
        )
    }

    static func dns(_ episode: GatewayDNSFailureEpisode) -> GatewayTelemetryProjection {
        var metadata: [String: String] = [
            "event": "gateway_dns_diagnostic",
            "request_id": episode.requestId,
            "episode": String(episode.episode),
            "host": String(episode.host.prefix(256)),
            "host_kind": episode.diagnostic.kind.rawValue,
            "secure_transport": episode.diagnostic.isSecureTransport ? "true" : "false",
        ]
        metadata["diagnostic_owner"] = "ios_native"
        return .init(event: "gateway_dns_diagnostic", level: "warn", metadata: metadata)
    }

    static func commandSpan(
        trace: GatewayTraceContext,
        method: String,
        startEpochMilliseconds: Int64,
        durationMilliseconds: UInt64,
        isError: Bool
    ) -> GatewayTelemetryProjection? {
        guard let root = parseTraceparent(trace.rootTraceparent),
              let child = parseTraceparent(trace.traceparent),
              child.traceId == root.traceId,
              !method.isEmpty,
              method.utf8.count <= 256
        else { return nil }

        return .init(
            event: "gateway_command_span",
            level: isError ? "warn" : "info",
            metadata: [
                "event": "trace_span",
                "span_name": "sand.gateway_post.\(method)",
                "span_kind": "client",
                "trace_id": root.traceId,
                "parent_span_id": root.spanId,
                "span_id": child.spanId,
                "start_epoch_ms": String(startEpochMilliseconds),
                "duration_ms": String(durationMilliseconds),
                "status": isError ? "error" : "ok",
                "platform_replacement": "ios_structured_log_trace_exporter",
            ]
        )
    }

    static func transport(_ report: CoordinatorTransportStageReport) -> GatewayTelemetryProjection {
        var metadata: [String: String] = [
            "event": "coordinator_transport_stage",
            "account_slot": String(report.accountSlot.prefix(128)),
            "client_nonce": String(report.clientNonce.prefix(256)),
            "stage": String(report.stage.prefix(128)),
            "attempt": String(report.attempt),
            "start_epoch_ms": String(report.startEpochMilliseconds),
            "duration_ms": String(report.durationMilliseconds),
            "is_error": report.isError ? "true" : "false",
        ]
        if let traceparent = report.traceparent {
            metadata["traceparent"] = String(traceparent.prefix(128))
        }
        return .init(
            event: "coordinator_transport_stage",
            level: report.isError ? "warn" : "info",
            metadata: metadata
        )
    }
}

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
        if error is GatewayCommandError { return true }
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
    typealias TelemetrySink = @MainActor (GatewayTelemetryProjection) -> Void

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
    private let telemetrySink: TelemetrySink
    private var inFlight: [String: InFlightRequest] = [:]

    init(
        supervisor: CoordinatorHostSupervisor,
        dnsDiagnostics: GatewayDNSDiagnosticTracker = .init(),
        traceRootFactory: @escaping TraceRootFactory = { GatewayTraceParent.makeRoot() },
        transportStageRecorder: TransportStageRecorder? = nil,
        telemetrySink: @escaping TelemetrySink = { _ in },
        transport: @escaping CommandTransport
    ) {
        self.supervisor = supervisor
        self.dnsDiagnostics = dnsDiagnostics
        self.traceRootFactory = traceRootFactory
        self.transportStageRecorder = transportStageRecorder
        self.telemetrySink = telemetrySink
        self.transport = { connection, method, args, _ in
            try await transport(connection, method, args)
        }
    }

    init(
        supervisor: CoordinatorHostSupervisor,
        dnsDiagnostics: GatewayDNSDiagnosticTracker = .init(),
        traceRootFactory: @escaping TraceRootFactory = { GatewayTraceParent.makeRoot() },
        transportStageRecorder: TransportStageRecorder? = nil,
        telemetrySink: @escaping TelemetrySink = { _ in },
        tracedTransport: @escaping TracedCommandTransport
    ) {
        self.supervisor = supervisor
        self.dnsDiagnostics = dnsDiagnostics
        self.traceRootFactory = traceRootFactory
        self.transportStageRecorder = transportStageRecorder
        self.telemetrySink = telemetrySink
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
                        ) {
                            await self.recordTransportStage(report)
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
                    ) {
                        await self.recordTransportStage(report)
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
                let postStartedEpochMilliseconds = coordinatorTransportEpochMilliseconds()
                let postStartedMonotonicMilliseconds = coordinatorTransportMonotonicMilliseconds()
                var postStage = sendTrace?.beginStage(
                    "gateway-post",
                    attempt: attemptIndex,
                    startEpochMilliseconds: postStartedEpochMilliseconds,
                    startMonotonicMilliseconds: postStartedMonotonicMilliseconds
                )

                do {
                    let value = try await self.transport(
                        tracedConnection,
                        method,
                        args,
                        trace
                    )
                    let postCompletedMonotonicMilliseconds = coordinatorTransportMonotonicMilliseconds()
                    if let report = postStage?.complete(
                        nowMonotonicMilliseconds: postCompletedMonotonicMilliseconds
                    ) {
                        await self.recordTransportStage(report)
                    }
                    self.recordGatewayCommandSpan(
                        trace: trace,
                        method: method,
                        startEpochMilliseconds: postStartedEpochMilliseconds,
                        startMonotonicMilliseconds: postStartedMonotonicMilliseconds,
                        endMonotonicMilliseconds: postCompletedMonotonicMilliseconds,
                        isError: false
                    )
                    try Task.checkCancellation()
                    self.dnsDiagnostics.recordSuccess(url: connection.baseURL)
                    self.telemetrySink(.reachability(
                        url: connection.baseURL,
                        outcome: .reachable,
                        requestId: normalizedRequestId,
                        attempt: attemptIndex
                    ))
                    return value
                } catch is CancellationError {
                    let postFailedMonotonicMilliseconds = coordinatorTransportMonotonicMilliseconds()
                    if let report = postStage?.fail(
                        nowMonotonicMilliseconds: postFailedMonotonicMilliseconds
                    ) {
                        await self.recordTransportStage(report)
                    }
                    self.recordGatewayCommandSpan(
                        trace: trace,
                        method: method,
                        startEpochMilliseconds: postStartedEpochMilliseconds,
                        startMonotonicMilliseconds: postStartedMonotonicMilliseconds,
                        endMonotonicMilliseconds: postFailedMonotonicMilliseconds,
                        isError: true
                    )
                    throw CancellationError()
                } catch {
                    let postFailedMonotonicMilliseconds = coordinatorTransportMonotonicMilliseconds()
                    if let report = postStage?.fail(
                        nowMonotonicMilliseconds: postFailedMonotonicMilliseconds
                    ) {
                        await self.recordTransportStage(report)
                    }
                    self.recordGatewayCommandSpan(
                        trace: trace,
                        method: method,
                        startEpochMilliseconds: postStartedEpochMilliseconds,
                        startMonotonicMilliseconds: postStartedMonotonicMilliseconds,
                        endMonotonicMilliseconds: postFailedMonotonicMilliseconds,
                        isError: true
                    )
                    lastError = error
                    self.supervisor.invalidateHealthCache()
                    let reachability = GatewayReachability.classifyOutcome(error)
                        ?? GatewayReachabilityOutcome.network
                    self.telemetrySink(.reachability(
                        url: connection.baseURL,
                        outcome: reachability,
                        requestId: normalizedRequestId,
                        attempt: attemptIndex
                    ))
                    if let episode = self.dnsDiagnostics.recordFailure(
                        url: connection.baseURL,
                        requestId: normalizedRequestId
                    ) {
                        self.telemetrySink(.dns(episode))
                    }
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

    private func recordGatewayCommandSpan(
        trace: GatewayTraceContext,
        method: String,
        startEpochMilliseconds: Int64,
        startMonotonicMilliseconds: UInt64,
        endMonotonicMilliseconds: UInt64,
        isError: Bool
    ) {
        let duration = endMonotonicMilliseconds >= startMonotonicMilliseconds
            ? endMonotonicMilliseconds - startMonotonicMilliseconds
            : 0
        if let projection = GatewayTelemetryProjection.commandSpan(
            trace: trace,
            method: method,
            startEpochMilliseconds: startEpochMilliseconds,
            durationMilliseconds: duration,
            isError: isError
        ) {
            telemetrySink(projection)
        }
    }

    private func recordTransportStage(_ report: CoordinatorTransportStageReport) async {
        if let recorder = transportStageRecorder {
            await recorder.record(report: report)
        }
        telemetrySink(.transport(report))
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
