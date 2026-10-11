import XCTest
@testable import Fabushi

@MainActor
private final class GatewayAsyncGate {
    private var continuation: CheckedContinuation<Void, Never>?
    private(set) var started = false

    func wait() async {
        started = true
        await withCheckedContinuation { continuation in
            self.continuation = continuation
        }
    }

    func open() {
        continuation?.resume()
        continuation = nil
    }
}

@MainActor
private final class GatewayFailureHost: MahayanaHostRequesting, @unchecked Sendable {
    let error: Error
    private(set) var requestCount = 0

    init(error: Error) {
        self.error = error
    }

    func request(
        method: String,
        params: [String: Any]
    ) async throws -> MahayanaHostJSONResult {
        requestCount += 1
        throw error
    }
}

@MainActor
private final class GatewayEventHost: MahayanaHostRequesting, @unchecked Sendable {
    var envelope: [String: Any]
    init(envelope: [String: Any]) { self.envelope = envelope }

    func request(
        method: String,
        params: [String: Any]
    ) async throws -> MahayanaHostJSONResult {
        guard method == "feature.receive" else { return .init(value: NSNull()) }
        return .init(value: envelope)
    }
}

final class GatewayParityTests: XCTestCase {
    private func waitForGate(_ gate: GatewayAsyncGate) async {
        for _ in 0..<64 {
            if gate.started { return }
            await Task.yield()
        }
    }

    func testGatewayClientRejectsRequestIdReuseForDifferentCommand() async throws {
        let gate = GatewayAsyncGate()
        let supervisor = CoordinatorHostSupervisor(
            resolveConnection: {
                .init(
                    baseURL: URL(string: "https://gateway.example")!,
                    headers: [:]
                )
            },
            healthProbe: { _ in true }
        )
        let client = CoordinatorGatewayClient(
            supervisor: supervisor,
            tracedTransport: { _, _, _, _ in
                await gate.wait()
                return .object(["ok": .bool(true)])
            }
        )

        let first = Task { @MainActor in
            try await client.dispatchCommand(
                requestId: "shared-request",
                method: "getState",
                args: .object(["scope": .string("a")])
            )
        }
        await waitForGate(gate)

        do {
            _ = try await client.dispatchCommand(
                requestId: "shared-request",
                method: "getState",
                args: .object(["scope": .string("b")])
            )
            XCTFail("requestId reuse with a different payload must fail closed")
        } catch let error as GatewayCommandError {
            XCTAssertTrue(error.localizedDescription.contains("reused"))
        } catch {
            XCTFail("unexpected error: \(error)")
        }

        gate.open()
        XCTAssertEqual(
            try await first.value,
            .object(["ok": .bool(true)])
        )
    }

    func testGatewayClientPropagatesChildTraceparentAcrossBoundedRetry() async throws {
        let root = "00-0123456789abcdef0123456789abcdef-1111111111111111-01"
        var traces: [GatewayTraceContext] = []
        var traceHeaders: [String?] = []
        var attempts = 0
        var resolves = 0

        let supervisor = CoordinatorHostSupervisor(
            resolveConnection: {
                resolves += 1
                return .init(
                    baseURL: URL(string: "https://gateway.example")!,
                    headers: ["authorization": "Bearer test"]
                )
            },
            healthProbe: { _ in false }
        )
        let client = CoordinatorGatewayClient(
            supervisor: supervisor,
            traceRootFactory: { root },
            tracedTransport: { connection, method, _, trace in
                traces.append(trace)
                traceHeaders.append(connection.headers["traceparent"])
                attempts += 1
                XCTAssertEqual(method, "getState")
                if attempts == 1 {
                    throw GatewayUnreachableError(
                        transportKind: "network",
                        message: "transient"
                    )
                }
                return .object(["requestId": .string(trace.requestId)])
            }
        )

        let value = try await client.dispatchCommand(
            requestId: "trace-request",
            method: "getState",
            args: .object([:])
        )

        XCTAssertEqual(value, .object(["requestId": .string("trace-request")]))
        XCTAssertEqual(traces.count, 2)
        XCTAssertEqual(traces.map(\.rootTraceparent), [root, root])
        XCTAssertEqual(traces.map(\.attempt), [1, 2])
        XCTAssertNotEqual(traces[0].traceparent, traces[1].traceparent)
        XCTAssertTrue(
            traces.allSatisfy {
                $0.traceparent.hasPrefix("00-0123456789abcdef0123456789abcdef-")
            }
        )
        XCTAssertEqual(traceHeaders.compactMap { $0 }, traces.map(\.traceparent))
        XCTAssertEqual(resolves, 2)
    }

    func testHostSupervisorRejectsResolveSettledAfterInvalidation() async throws {
        let gate = GatewayAsyncGate()
        var resolves = 0
        let old = CoordinatorGatewayClient.Connection(
            baseURL: URL(string: "https://old.example")!,
            headers: [:]
        )
        let replacement = CoordinatorGatewayClient.Connection(
            baseURL: URL(string: "https://new.example")!,
            headers: [:]
        )
        let supervisor = CoordinatorHostSupervisor(
            resolveConnection: {
                resolves += 1
                if resolves == 1 {
                    await gate.wait()
                    return old
                }
                return replacement
            },
            healthProbe: { _ in false }
        )

        let pending = Task { @MainActor in
            try await supervisor.ensureConnection()
        }
        await waitForGate(gate)
        supervisor.invalidateHealthCache()
        gate.open()

        XCTAssertEqual(try await pending.value, replacement)
        XCTAssertEqual(resolves, 2)
    }

    func testDNSDiagnosticsEmitOneEpisodeUntilSuccessAndThrottleNewEpisode() {
        var now = Date(timeIntervalSince1970: 100)
        let tracker = GatewayDNSDiagnosticTracker(now: { now })
        let url = URL(string: "https://gateway.example/path")!

        XCTAssertEqual(
            tracker.recordFailure(url: url, requestId: "r1")?.episode,
            1
        )
        XCTAssertTrue(tracker.episodeActive(host: "gateway.example"))

        now = Date(timeIntervalSince1970: 200)
        XCTAssertNil(tracker.recordFailure(url: url, requestId: "r2"))

        tracker.recordSuccess(url: url)
        XCTAssertFalse(tracker.episodeActive(host: "gateway.example"))

        now = Date(timeIntervalSince1970: 130)
        XCTAssertNil(tracker.recordFailure(url: url, requestId: "r3"))

        now = Date(timeIntervalSince1970: 161)
        XCTAssertEqual(
            tracker.recordFailure(url: url, requestId: "r4")?.episode,
            2
        )
        XCTAssertEqual(tracker.emitted.map(\.requestId), ["r1", "r4"])
    }

    func testGatewayReachabilityMatchesDesktopRetryClasses() {
        XCTAssertEqual(
            GatewayReachability.classifyBaseURL(
                URL(string: "https://localhost/fabushi")!
            ),
            .loopback
        )
        XCTAssertEqual(
            GatewayReachability.classifyBaseURL(
                URL(string: "https://gateway.example")!
            ),
            .remote
        )
        XCTAssertEqual(
            GatewayReachability.outcome(forHTTPStatus: 503),
            .http(503)
        )
        XCTAssertTrue(
            GatewayReachability.outcome(forHTTPStatus: 503)?.retryable == true
        )
        XCTAssertEqual(
            GatewayReachability.outcome(forHTTPStatus: 403),
            .accessDenied
        )
        XCTAssertFalse(
            GatewayReachability.outcome(forHTTPStatus: 403)?.retryable == true
        )

        let timeout = GatewayReachability.classifyOutcome(
            URLError(.timedOut)
        )
        XCTAssertEqual(timeout, .timeout)
        XCTAssertTrue(timeout?.retryable == true)

        let refused = GatewayReachability.classifyOutcome(
            URLError(.cannotConnectToHost)
        )
        XCTAssertEqual(refused, .refused)
        XCTAssertTrue(refused?.retryable == true)
    }

    func testGatewayShippingDispatchPreservesAccessDeniedAndDoesNotRetry() async {
        let host = GatewayFailureHost(
            error: NSError(
                domain: "GatewayParityTests",
                code: 403,
                userInfo: [
                    NSLocalizedDescriptionKey:
                        GATEWAY_ACCESS_DENIED_MESSAGE_MARKER
                ]
            )
        )
        let supervisor = MahayanaLocalHostSupervisor(
            host: host,
            factory: { host }
        )
        let coordinator = MahayanaCoordinator(hostSupervisor: supervisor)

        let outcome = await coordinator.dispatchTransport(
            requestId: "access-denied",
            method: "feature.receive",
            args: .object([:])
        )
        guard case .failed(let failure) = outcome else {
            return XCTFail("access denied must fail through shipping Gateway dispatch")
        }
        XCTAssertEqual(
            failure.code,
            GatewayFailureCode.unreachable.rawValue
        )
        XCTAssertEqual(failure.transportKind, "access_denied")
        XCTAssertEqual(host.requestCount, 1)
    }

    func testGatewayFailureMapperPreservesReachabilityTransportKind() {
        let timeout = GatewayFailureMapper.failure(
            for: URLError(.timedOut)
        )
        XCTAssertEqual(timeout.code, GatewayFailureCode.unreachable.rawValue)
        XCTAssertEqual(timeout.transportKind, "timeout")

        let denied = GatewayFailureMapper.failure(
            for: GatewayUnreachableError(
                transportKind: "access_denied",
                message: "denied"
            )
        )
        XCTAssertEqual(denied.code, GatewayFailureCode.unreachable.rawValue)
        XCTAssertEqual(denied.transportKind, "access_denied")
    }

    func testGatewayClientDoesNotRetryCommandErrorOnReadSafeMethod() async {
        var attempts = 0
        let supervisor = CoordinatorHostSupervisor(
            resolveConnection: {
                .init(
                    baseURL: URL(string: "https://gateway.example")!,
                    headers: [:]
                )
            },
            healthProbe: { _ in true }
        )
        let client = CoordinatorGatewayClient(
            supervisor: supervisor,
            tracedTransport: { _, _, _, _ in
                attempts += 1
                throw GatewayCommandError(message: "invalid command")
            }
        )

        do {
            _ = try await client.dispatchCommand(
                requestId: "command-error",
                method: "getState",
                args: .object([:])
            )
            XCTFail("command errors must not be retried")
        } catch is GatewayCommandError {
        } catch {
            XCTFail("unexpected error: \(error)")
        }
        XCTAssertEqual(attempts, 1)
    }

    func testGatewayClientRejectsEndpointDriftBeforeSecondTransportAttempt() async {
        var resolves = 0
        var transports = 0
        let supervisor = CoordinatorHostSupervisor(
            resolveConnection: {
                resolves += 1
                let host = resolves == 1 ? "old.example" : "new.example"
                return .init(
                    baseURL: URL(string: "https://\(host)")!,
                    headers: [:]
                )
            },
            healthProbe: { _ in false }
        )
        let client = CoordinatorGatewayClient(
            supervisor: supervisor,
            tracedTransport: { _, _, _, _ in
                transports += 1
                throw GatewayUnreachableError(
                    transportKind: "network",
                    message: "drop"
                )
            }
        )

        do {
            _ = try await client.dispatchCommand(
                requestId: "endpoint-drift",
                method: "getState",
                args: .object([:])
            )
            XCTFail("retry must reject a changed Gateway endpoint")
        } catch let error as GatewayUnreachableError {
            XCTAssertEqual(error.transportKind, "coordinator")
            XCTAssertTrue(error.localizedDescription.contains("endpoint changed"))
        } catch {
            XCTFail("unexpected error: \(error)")
        }
        XCTAssertEqual(resolves, 2)
        XCTAssertEqual(transports, 1)
    }

    func testFeatureReceiveProjectsCanonicalGatewayEventFamilyAndDropsUnknownChannel() async throws {
        let host = GatewayEventHost(envelope: [
            "channel": "workflows",
            "payload": ["workflowId": "wf-1"],
        ])
        let supervisor = MahayanaLocalHostSupervisor(host: host, factory: { host })
        let coordinator = MahayanaCoordinator(hostSupervisor: supervisor)
        var projected: [(String, CoordinatorPayload)] = []
        coordinator.setRendererEventSink { family, payload in projected.append((family, payload)) }

        let outcome = await coordinator.dispatchTransport(
            requestId: "event-request-1",
            method: "feature.receive",
            args: .object([:])
        )
        guard case .ok = outcome else { return XCTFail("feature.receive must settle through shipping transport") }
        XCTAssertEqual(projected.count, 1)
        XCTAssertEqual(projected.first?.0, "agents-workflow")
        XCTAssertEqual(projected.first?.1, .object(["workflowId": .string("wf-1")]))

        host.envelope = ["channel": "unknown-channel", "payload": ["ignored": true]]
        let unknown = await coordinator.dispatchTransport(
            requestId: "event-request-2",
            method: "feature.receive",
            args: .object([:])
        )
        guard case .ok = unknown else { return XCTFail("unknown channel response should settle") }
        XCTAssertEqual(projected.count, 1)
    }
}
