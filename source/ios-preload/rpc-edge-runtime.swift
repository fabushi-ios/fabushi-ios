import Foundation

struct IOSEdgeReplyFailure: LocalizedError, Equatable, Sendable {
    let code: String
    let detail: String

    var errorDescription: String? { "\(code): \(detail)" }
}

enum IOSEdgeReplyEnvelope: Equatable, Sendable {
    case ok(CoordinatorPayload)
    case failed(IOSEdgeReplyFailure)
}

@MainActor
protocol IOSRPCEdgeTransport: AnyObject {
    func invoke(channel: String, payload: CoordinatorPayload) async throws -> IOSEdgeReplyEnvelope
    func subscribe(
        channel: String,
        listener: @escaping @MainActor (CoordinatorPayload) -> Void
    ) -> () -> Void
}

enum IOSRPCEdgeRuntime {
    static let unknownMethodCode = "edge/unknown-method"
    static let handlerFailedCode = "edge/handler-failed"

    static func methodChannel(edge: String, method: String) -> String {
        "sand-rpc:\(edge):m:\(method)"
    }

    static func eventChannel(edge: String, event: String) -> String {
        "sand-rpc:\(edge):e:\(event)"
    }

    @MainActor
    static func call(
        edge: String,
        method: String,
        payload: CoordinatorPayload,
        transport: any IOSRPCEdgeTransport
    ) async throws -> CoordinatorPayload {
        let reply: IOSEdgeReplyEnvelope
        do {
            reply = try await transport.invoke(
                channel: methodChannel(edge: edge, method: method),
                payload: payload
            )
        } catch {
            throw IOSEdgeReplyFailure(
                code: unknownMethodCode,
                detail: error.localizedDescription
            )
        }
        switch reply {
        case .ok(let value): return value
        case .failed(let failure): throw failure
        }
    }
}
