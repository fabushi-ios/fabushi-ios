import Foundation

let UNTRACED_SERVICE_TYPE_NAMES: Set<String> = ["aiserver.v1.AnalyticsService"]
let SAND_RPC_TRACE_WINDOW_DURATION_MS = 2 * 60 * 1_000

struct RpcSpanContext: Equatable, Sendable {
    let traceId: String
    let spanId: String
    let traceFlags: UInt8
}

struct RpcTracingPolicy: Equatable, Sendable {
    let enabled: Bool
    let sampleRatio: Double
}

struct RpcTracePreparation: Equatable, Sendable {
    let requestId: String
    let traceparent: String?
    let attributes: [String: String]
    let parentTraceparent: String?
}

enum RpcTraceSettlement: Equatable, Sendable {
    case success(responseStreaming: Bool)
    case failure(label: String, connectCode: String?)
}

func isValidSpanContext(_ context: RpcSpanContext) -> Bool {
    let trace = context.traceId
    let span = context.spanId
    guard trace.count == 32, span.count == 16 else { return false }
    guard trace.range(of: #"^[0-9a-fA-F]{32}$"#, options: .regularExpression) != nil,
          span.range(of: #"^[0-9a-fA-F]{16}$"#, options: .regularExpression) != nil else {
        return false
    }
    return trace.contains { $0 != "0" } && span.contains { $0 != "0" }
}

func spanTraceparent(_ context: RpcSpanContext) -> String? {
    guard isValidSpanContext(context) else { return nil }
    let flags = (context.traceFlags & 1) == 1 ? "01" : "00"
    return "00-\(context.traceId.lowercased())-\(context.spanId.lowercased())-\(flags)"
}

func pinRequestId(
    _ headers: inout [String: String],
    randomUUID: () -> String = { UUID().uuidString.lowercased() }
) -> String {
    if let pinned = headers["x-request-id"], !pinned.isEmpty {
        return pinned
    }
    let minted = randomUUID()
    headers["x-request-id"] = minted
    return minted
}

func injectTraceparent(_ headers: inout [String: String], context: RpcSpanContext) {
    if let traceparent = spanTraceparent(context) {
        headers["traceparent"] = traceparent
    }
}

struct SandRpcTracingRuntime: Sendable {
    private(set) var activeWindowContext: RpcSpanContext?
    private(set) var activeWindowExpiresAtMs: Int64?
    var sessionId: String?

    mutating func startTraceWindow(rootContext: RpcSpanContext, nowMs: Int64) -> Bool {
        guard isValidSpanContext(rootContext) else { return false }
        activeWindowContext = rootContext
        activeWindowExpiresAtMs = nowMs + Int64(SAND_RPC_TRACE_WINDOW_DURATION_MS)
        return true
    }

    mutating func endTraceWindow() {
        activeWindowContext = nil
        activeWindowExpiresAtMs = nil
    }

    mutating func getTraceWindowTraceparent(nowMs: Int64) -> String? {
        expireTraceWindowIfNeeded(nowMs: nowMs)
        return activeWindowContext.flatMap(spanTraceparent)
    }

    mutating func prepare(
        serviceTypeName: String,
        methodName: String,
        requestStreaming: Bool,
        headers: inout [String: String],
        childSpanContext: RpcSpanContext,
        policy: RpcTracingPolicy = .init(enabled: true, sampleRatio: 1),
        nowMs: Int64,
        random: () -> Double = { Double.random(in: 0..<1) },
        randomUUID: () -> String = { UUID().uuidString.lowercased() }
    ) -> RpcTracePreparation? {
        guard !UNTRACED_SERVICE_TYPE_NAMES.contains(serviceTypeName) else { return nil }
        expireTraceWindowIfNeeded(nowMs: nowMs)

        let parentTraceparent = activeWindowContext.flatMap(spanTraceparent)
        if activeWindowContext == nil {
            guard policy.enabled else { return nil }
            let ratio = min(1, max(0, policy.sampleRatio))
            guard random() < ratio else { return nil }
        }

        let requestId = pinRequestId(&headers, randomUUID: randomUUID)
        injectTraceparent(&headers, context: childSpanContext)

        var attributes = [
            "rpc.system": "connectrpc",
            "rpc.service": serviceTypeName,
            "rpc.method": methodName,
            "sand.rpc.request_streaming": requestStreaming ? "true" : "false",
            "sand.request_id": requestId,
        ]
        if let sessionId, !sessionId.isEmpty {
            attributes["sand.session_id"] = sessionId
        }

        return .init(
            requestId: requestId,
            traceparent: headers["traceparent"],
            attributes: attributes,
            parentTraceparent: parentTraceparent
        )
    }

    func settlement(_ result: RpcTraceSettlement) -> [String: String] {
        switch result {
        case .success(let responseStreaming):
            return [
                "sand.rpc.bounded_at": responseStreaming ? "response-stream-start" : "response-complete"
            ]
        case .failure(let label, let connectCode):
            var values = ["error": label]
            if let connectCode, !connectCode.isEmpty {
                values["rpc.connect_rpc.error_code"] = connectCode
            }
            return values
        }
    }

    private mutating func expireTraceWindowIfNeeded(nowMs: Int64) {
        if let expiry = activeWindowExpiresAtMs, nowMs >= expiry {
            endTraceWindow()
        }
    }
}
