import XCTest
@testable import Fabushi

final class SharedRpcTracingParityTests: XCTestCase {
    private let root = RpcSpanContext(
        traceId: "0123456789abcdef0123456789abcdef",
        spanId: "0123456789abcdef",
        traceFlags: 1
    )
    private let child = RpcSpanContext(
        traceId: "fedcba9876543210fedcba9876543210",
        spanId: "fedcba9876543210",
        traceFlags: 0
    )

    func testSpanContextValidationAndTraceparent() {
        XCTAssertTrue(isValidSpanContext(root))
        XCTAssertEqual(
            spanTraceparent(root),
            "00-0123456789abcdef0123456789abcdef-0123456789abcdef-01"
        )
        XCTAssertFalse(isValidSpanContext(.init(
            traceId: String(repeating: "0", count: 32),
            spanId: "0123456789abcdef",
            traceFlags: 1
        )))
        XCTAssertNil(spanTraceparent(.init(traceId: "bad", spanId: "bad", traceFlags: 0)))
    }

    func testRequestIdPinningDoesNotReplaceExistingValue() {
        var headers = ["x-request-id": "pinned"]
        XCTAssertEqual(pinRequestId(&headers, randomUUID: { "minted" }), "pinned")
        XCTAssertEqual(headers["x-request-id"], "pinned")

        headers = [:]
        XCTAssertEqual(pinRequestId(&headers, randomUUID: { "minted" }), "minted")
        XCTAssertEqual(headers["x-request-id"], "minted")
    }

    func testTraceWindowForcesTraceUntilExpiry() {
        var runtime = SandRpcTracingRuntime(sessionId: "session-1")
        XCTAssertTrue(runtime.startTraceWindow(rootContext: root, nowMs: 1_000))

        var headers: [String: String] = [:]
        let prepared = runtime.prepare(
            serviceTypeName: "aiserver.v1.ChatService",
            methodName: "Stream",
            requestStreaming: true,
            headers: &headers,
            childSpanContext: child,
            policy: .init(enabled: false, sampleRatio: 0),
            nowMs: 1_001,
            random: { 1 },
            randomUUID: { "request-1" }
        )
        XCTAssertEqual(prepared?.requestId, "request-1")
        XCTAssertEqual(prepared?.parentTraceparent, spanTraceparent(root))
        XCTAssertEqual(prepared?.attributes["sand.session_id"], "session-1")
        XCTAssertEqual(headers["traceparent"], spanTraceparent(child))

        XCTAssertNil(runtime.getTraceWindowTraceparent(
            nowMs: 1_000 + Int64(SAND_RPC_TRACE_WINDOW_DURATION_MS)
        ))
    }

    func testSamplingAndUntracedAnalyticsBehavior() {
        var runtime = SandRpcTracingRuntime()
        var headers: [String: String] = [:]

        XCTAssertNil(runtime.prepare(
            serviceTypeName: "aiserver.v1.AnalyticsService",
            methodName: "TrackEvents",
            requestStreaming: false,
            headers: &headers,
            childSpanContext: child,
            nowMs: 1_000,
            random: { 0 }
        ))
        XCTAssertTrue(headers.isEmpty)

        XCTAssertNil(runtime.prepare(
            serviceTypeName: "aiserver.v1.ChatService",
            methodName: "Unary",
            requestStreaming: false,
            headers: &headers,
            childSpanContext: child,
            policy: .init(enabled: true, sampleRatio: 0.25),
            nowMs: 1_000,
            random: { 0.5 }
        ))

        let traced = runtime.prepare(
            serviceTypeName: "aiserver.v1.ChatService",
            methodName: "Unary",
            requestStreaming: false,
            headers: &headers,
            childSpanContext: child,
            policy: .init(enabled: true, sampleRatio: 0.25),
            nowMs: 1_000,
            random: { 0.1 },
            randomUUID: { "request-2" }
        )
        XCTAssertEqual(traced?.attributes["rpc.system"], "connectrpc")
        XCTAssertEqual(traced?.attributes["sand.rpc.request_streaming"], "false")
    }

    func testSettlementMatchesResponseAndConnectErrorSemantics() {
        let runtime = SandRpcTracingRuntime()
        XCTAssertEqual(
            runtime.settlement(.success(responseStreaming: true))["sand.rpc.bounded_at"],
            "response-stream-start"
        )
        let failed = runtime.settlement(.failure(label: "Unavailable", connectCode: "UNAVAILABLE"))
        XCTAssertEqual(failed["error"], "Unavailable")
        XCTAssertEqual(failed["rpc.connect_rpc.error_code"], "UNAVAILABLE")
    }
}
