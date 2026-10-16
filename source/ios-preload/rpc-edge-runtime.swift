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

    static func methodName(edge: String, channel: String) -> String? {
        let prefix = "sand-rpc:\(edge):m:"
        guard channel.hasPrefix(prefix) else { return nil }
        let method = String(channel.dropFirst(prefix.count))
        return method.isEmpty ? nil : method
    }

    static func eventName(edge: String, channel: String) -> String? {
        let prefix = "sand-rpc:\(edge):e:"
        guard channel.hasPrefix(prefix) else { return nil }
        let event = String(channel.dropFirst(prefix.count))
        return event.isEmpty ? nil : event
    }

    @MainActor
    static func subscribe(
        edge: String,
        event: String,
        transport: any IOSRPCEdgeTransport,
        listener: @escaping @MainActor (CoordinatorPayload) -> Void
    ) -> () -> Void {
        transport.subscribe(
            channel: eventChannel(edge: edge, event: event),
            listener: listener
        )
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
        } catch is CancellationError {
            throw CancellationError()
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
