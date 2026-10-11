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
        XCTAssertEqual(traceHeaders, traces.map(\.traceparent))
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
}
