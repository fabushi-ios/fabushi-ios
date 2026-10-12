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
private final class GatewayDescriptorTestStore: IOSCursorCredentialStoring {
    var values: [String: String] = [:]
    private(set) var writeCount = 0
    private(set) var deleteCount = 0
    func readSecret(_ key: String) async throws -> String? { values[key] }
    func writeSecret(_ key: String, value: String) async throws {
        writeCount += 1
        values[key] = value
    }
    func deleteSecret(_ key: String) async throws {
        deleteCount += 1
        values.removeValue(forKey: key)
    }
    func waitForEncryptedStorage() async throws {}
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

@MainActor
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
        let firstValue = try await first.value
        XCTAssertEqual(
            firstValue,
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

        let pendingValue = try await pending.value
        XCTAssertEqual(pendingValue, replacement)
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

    func testTransportStageRecorderCorrelatesNonceTraceAndEcho() async {
        let recorder = TransportStageRecorder()
        let trace = await recorder.beginSend(
            accountSlot: "host",
            clientNonce: "nonce-1",
            traceparent: "trace-1",
            nowMonotonicMilliseconds: 10
        )
        XCTAssertEqual(trace?.clientNonce, "nonce-1")
        var token = trace?.beginStage(
            "gateway-post",
            attempt: 0,
            startEpochMilliseconds: 100,
            startMonotonicMilliseconds: 20
        )
        if let report = token?.complete(nowMonotonicMilliseconds: 35) {
            await recorder.record(report: report)
        }
        let echo = await recorder.recordSendEcho(
            accountSlot: "host",
            clientNonce: "nonce-1",
            nowEpochMilliseconds: 150,
            nowMonotonicMilliseconds: 40
        )
        XCTAssertEqual(echo?.stage, COORDINATOR_SSE_ECHO_STAGE)
        XCTAssertEqual(echo?.traceparent, "trace-1")
        let reports = await recorder.reportSnapshot()
        XCTAssertEqual(reports.map(\.stage), ["gateway-post", COORDINATOR_SSE_ECHO_STAGE])
        XCTAssertEqual(reports.first?.durationMilliseconds, 15)
    }

    func testTransportStageRecorderBoundsAndExpiresPendingEchoes() async {
        let recorder = TransportStageRecorder()
        for index in 0..<(COORDINATOR_PENDING_SEND_ECHO_MAX + 1) {
            _ = await recorder.beginSend(
                accountSlot: "host",
                clientNonce: "nonce-\(index)",
                traceparent: "trace-\(index)",
                nowMonotonicMilliseconds: UInt64(index)
            )
        }
        let bounded = await recorder.pendingEchoCount()
        XCTAssertEqual(bounded, COORDINATOR_PENDING_SEND_ECHO_MAX)
        let evicted = await recorder.recordSendEcho(
            accountSlot: "host",
            clientNonce: "nonce-0",
            nowEpochMilliseconds: 1,
            nowMonotonicMilliseconds: 1
        )
        XCTAssertNil(evicted)

        _ = await recorder.beginSend(
            accountSlot: "host",
            clientNonce: "ttl",
            traceparent: "ttl-trace",
            nowMonotonicMilliseconds: 10
        )
        let expired = await recorder.recordSendEcho(
            accountSlot: "host",
            clientNonce: "ttl",
            nowEpochMilliseconds: 2,
            nowMonotonicMilliseconds: 10 + COORDINATOR_PENDING_SEND_ECHO_TTL_MILLISECONDS + 1
        )
        XCTAssertNil(expired)
    }

    func testGatewayClientRecordsNativeChatSendTransportStages() async throws {
        let recorder = TransportStageRecorder()
        let root = "00-0123456789abcdef0123456789abcdef-2222222222222222-01"
        let supervisor = CoordinatorHostSupervisor(
            resolveConnection: {
                .init(baseURL: URL(string: "https://gateway.example")!, headers: [:])
            },
            healthProbe: { _ in true }
        )
        let client = CoordinatorGatewayClient(
            supervisor: supervisor,
            traceRootFactory: { root },
            transportStageRecorder: recorder,
            tracedTransport: { _, _, _, _ in
                .object(["accepted": .bool(true)])
            }
        )
        _ = try await client.dispatchCommand(
            requestId: "outer",
            method: "feature.execute",
            args: .object([
                "command": .object([
                    "type": .string("chat.send"),
                    "requestId": .string("native-nonce"),
                ]),
            ])
        )
        let reports = await recorder.reportSnapshot()
        XCTAssertEqual(reports.map(\.stage), ["gateway-connect", "gateway-post"])
        XCTAssertTrue(reports.allSatisfy {
            $0.clientNonce == "native-nonce"
                && $0.traceparent == root
                && !$0.isError
        })
        let pending = await recorder.pendingEchoCount()
        XCTAssertEqual(pending, 1)
    }

    func testShippingFeatureReceiveSettlesNativeChatSendEchoCorrelation() async {
        let recorder = TransportStageRecorder()
        let host = GatewayEventHost(envelope: [
            "channel": "transcript",
            "payload": [
                "type": "chat.message",
                "role": "user",
                "messageId": "native-echo-nonce",
            ],
        ])
        let coordinator = MahayanaCoordinator(
            hostSupervisor: MahayanaLocalHostSupervisor(host: host, factory: { host }),
            transportStageRecorder: recorder
        )
        let send = await coordinator.dispatchTransport(
            requestId: "send",
            method: "feature.execute",
            args: .object([
                "command": .object([
                    "type": .string("chat.send"),
                    "requestId": .string("native-echo-nonce"),
                ]),
            ])
        )
        guard case .ok = send else { return XCTFail("chat.send failed") }
        let before = await recorder.pendingEchoCount()
        XCTAssertEqual(before, 1)

        let receive = await coordinator.dispatchTransport(
            requestId: "receive",
            method: "feature.receive",
            args: .object([:])
        )
        guard case .ok = receive else { return XCTFail("feature.receive failed") }
        let after = await recorder.pendingEchoCount()
        XCTAssertEqual(after, 0)
        let reports = await recorder.reportSnapshot()
        XCTAssertEqual(reports.last?.stage, COORDINATOR_SSE_ECHO_STAGE)
        XCTAssertEqual(reports.last?.clientNonce, "native-echo-nonce")
    }


    func testGatewayClientDoesNotRetrySideEffectingFeatureExecuteTransportFailure() async {
        var attempts = 0
        let supervisor = CoordinatorHostSupervisor(
            resolveConnection: {
                .init(baseURL: URL(string: "https://gateway.example")!, headers: [:])
            },
            healthProbe: { _ in false }
        )
        let client = CoordinatorGatewayClient(
            supervisor: supervisor,
            tracedTransport: { _, method, _, _ in
                attempts += 1
                XCTAssertEqual(method, "feature.execute")
                throw GatewayUnreachableError(
                    transportKind: "network",
                    message: "ambiguous transport drop"
                )
            }
        )

        do {
            _ = try await client.dispatchCommand(
                requestId: "side-effect-no-retry",
                method: "feature.execute",
                args: .object([
                    "command": .object([
                        "type": .string("chat.send"),
                        "requestId": .string("side-effect-nonce"),
                    ]),
                ])
            )
            XCTFail("side-effecting feature.execute must fail closed without transport retry")
        } catch let error as GatewayUnreachableError {
            XCTAssertEqual(error.transportKind, "network")
        } catch {
            XCTFail("unexpected error: \(error)")
        }
        XCTAssertEqual(attempts, 1)
    }

    func testHostSupervisorUsesHealthyTTLThenReprobesAndResolvesAfterFailure() async throws {
        var now = Date(timeIntervalSince1970: 1_000)
        var resolves = 0
        var probes = 0
        let first = CoordinatorGatewayClient.Connection(
            baseURL: URL(string: "https://first.example")!,
            headers: ["authorization": "Bearer scoped"]
        )
        let second = CoordinatorGatewayClient.Connection(
            baseURL: URL(string: "https://second.example")!,
            headers: ["authorization": "Bearer replacement"]
        )
        var probeResults = [true, false]

        let supervisor = CoordinatorHostSupervisor(
            now: { now },
            resolveConnection: {
                resolves += 1
                return resolves == 1 ? first : second
            },
            healthProbe: { connection in
                probes += 1
                XCTAssertTrue(connection == first || connection == second)
                return probeResults.removeFirst()
            }
        )

        let initialConnection = try await supervisor.ensureConnection()
        XCTAssertEqual(initialConnection, first)
        XCTAssertEqual(resolves, 1)
        XCTAssertEqual(probes, 0)

        let probedConnection = try await supervisor.ensureConnection()
        XCTAssertEqual(probedConnection, first)
        XCTAssertEqual(probes, 1)

        now = now.addingTimeInterval(
            Double(CoordinatorHostSupervisor.healthProbeTTLMilliseconds - 1) / 1_000
        )
        let cachedConnection = try await supervisor.ensureConnection()
        XCTAssertEqual(cachedConnection, first)
        XCTAssertEqual(probes, 1, "fresh health TTL should skip a second probe")

        now = now.addingTimeInterval(0.002)
        let refreshedConnection = try await supervisor.ensureConnection()
        XCTAssertEqual(refreshedConnection, second)
        XCTAssertEqual(probes, 2)
        XCTAssertEqual(resolves, 2)
    }


    func testGatewayDescriptorCacheScopesTTLAndDeduplicatesWrites() async {
        var now: Int64 = 1_000
        let store = GatewayDescriptorTestStore()
        let cache = IOSGatewayDescriptorKeychainCache(store: store, nowMilliseconds: { now })
        let value = IOSCursorSandBoxConnection(
            networkToken: "n", execDaemonAuthToken: "e",
            execDaemonURL: "https://exec.example", vncURL: "",
            gatewayURL: "https://gateway.example", gatewayToken: "g",
            forkVNCBaseURL: ""
        )
        await cache.write(accountScope: "a", connection: value)
        let sameScope = await cache.read(accountScope: "a")
        XCTAssertEqual(sameScope, value)
        let foreignScope = await cache.read(accountScope: "b")
        XCTAssertNil(foreignScope)
        await cache.write(accountScope: "a", connection: value)
        XCTAssertEqual(store.writeCount, 1)
        now += IOSGatewayDescriptorKeychainCache.maxAgeMilliseconds + 1
        let expired = await cache.read(accountScope: "a")
        XCTAssertNil(expired)
    }

    func testGatewayDescriptorFastPathReturnsCacheWhileSingleflightRefreshes() async throws {
        let store = GatewayDescriptorTestStore()
        let cache = IOSGatewayDescriptorKeychainCache(store: store)
        let cached = IOSCursorSandBoxConnection(
            networkToken: "cached", execDaemonAuthToken: "cached",
            execDaemonURL: "https://cached.example", vncURL: "",
            gatewayURL: "", gatewayToken: "", forkVNCBaseURL: ""
        )
        let live = IOSCursorSandBoxConnection(
            networkToken: "live", execDaemonAuthToken: "live",
            execDaemonURL: "https://live.example", vncURL: "",
            gatewayURL: "", gatewayToken: "", forkVNCBaseURL: ""
        )
        await cache.write(accountScope: "a", connection: cached)
        let gate = GatewayAsyncGate()
        var resolves = 0
        let fastPath = IOSGatewayDescriptorFastPath(
            cache: cache,
            getAccountScope: { "a" },
            resolveLive: {
                resolves += 1
                if resolves == 1 { await gate.wait() }
                return live
            }
        )
        XCTAssertEqual(try await fastPath.resolve(), cached)
        await waitForGate(gate)
        XCTAssertEqual(try await fastPath.resolve(), cached)
        XCTAssertEqual(resolves, 1)
        gate.open()
        for _ in 0..<64 {
            if store.writeCount >= 2 { break }
            await Task.yield()
        }
        XCTAssertEqual(store.writeCount, 2)
        XCTAssertEqual(try await fastPath.resolve(), live)
    }

    func testGatewayDescriptorFastPathFencesStaleRefreshAndClearsDenial() async throws {
        let store = GatewayDescriptorTestStore()
        let cache = IOSGatewayDescriptorKeychainCache(store: store)
        let cached = IOSCursorSandBoxConnection(
            networkToken: "cached", execDaemonAuthToken: "cached",
            execDaemonURL: "https://cached.example", vncURL: "",
            gatewayURL: "", gatewayToken: "", forkVNCBaseURL: ""
        )
        let replacement = IOSCursorSandBoxConnection(
            networkToken: "replacement", execDaemonAuthToken: "replacement",
            execDaemonURL: "https://replacement.example", vncURL: "",
            gatewayURL: "", gatewayToken: "", forkVNCBaseURL: ""
        )
        await cache.write(accountScope: "a", connection: cached)
        var scope: String? = "a"
        let gate = GatewayAsyncGate()
        let fastPath = IOSGatewayDescriptorFastPath(
            cache: cache,
            getAccountScope: { scope },
            resolveLive: { await gate.wait(); return replacement }
        )
        XCTAssertEqual(try await fastPath.resolve(), cached)
        await waitForGate(gate)
        scope = "b"
        gate.open()
        for _ in 0..<64 { await Task.yield() }
        let retained = await cache.read(accountScope: "a")
        XCTAssertEqual(retained, cached)
        let foreign = await cache.read(accountScope: "b")
        XCTAssertNil(foreign)

        let denialStore = GatewayDescriptorTestStore()
        let denied = IOSGatewayDescriptorFastPath(
            cache: IOSGatewayDescriptorKeychainCache(store: denialStore),
            getAccountScope: { "a" },
            resolveLive: {
                throw NSError(domain: "gateway", code: 403, userInfo: [
                    NSLocalizedDescriptionKey: "EnsureSandBox failed with HTTP 403"
                ])
            }
        )
        do {
            _ = try await denied.resolve()
            XCTFail("denial must fail")
        } catch {}
        XCTAssertEqual(denialStore.deleteCount, 1)
    }


    func testGatewayDescriptorFastPathHonorsBlockedRetryHold() async throws {
        let store = GatewayDescriptorTestStore()
        let cache = IOSGatewayDescriptorKeychainCache(store: store)
        var now: Int64 = 10_000
        var calls = 0
        let live = IOSCursorSandBoxConnection(
            networkToken: "live", execDaemonAuthToken: "live",
            execDaemonURL: "https://live.example", vncURL: "",
            gatewayURL: "", gatewayToken: "", forkVNCBaseURL: ""
        )
        let fastPath = IOSGatewayDescriptorFastPath(
            cache: cache,
            getAccountScope: { "account-a" },
            nowMilliseconds: { now },
            resolveLive: {
                calls += 1
                if calls == 1 {
                    throw IOSCursorSandBoxBrokerError(
                        kind: .blocked(retryAfterMilliseconds: 7_000),
                        message: "blocked"
                    )
                }
                return live
            }
        )

        do {
            _ = try await fastPath.resolve()
            XCTFail("first broker block must fail")
        } catch let error as IOSCursorSandBoxBrokerError {
            XCTAssertEqual(error.kind, .blocked(retryAfterMilliseconds: 7_000))
        }
        XCTAssertEqual(calls, 1)

        now += 1_000
        do {
            _ = try await fastPath.resolve()
            XCTFail("active hold must fail without another broker request")
        } catch let error as IOSCursorSandBoxBrokerError {
            guard case .blocked(let retry) = error.kind else {
                return XCTFail("expected blocked hold")
            }
            XCTAssertEqual(retry, 6_000)
        }
        XCTAssertEqual(calls, 1)

        now += 6_001
        XCTAssertEqual(try await fastPath.resolve(), live)
        XCTAssertEqual(calls, 2)
    }

}
