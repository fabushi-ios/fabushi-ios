import Foundation

@MainActor
final class GatewayRequestDispatcher {
    typealias Dispatch = @MainActor (_ requestId: String, _ method: String, _ args: CoordinatorPayload) async throws -> CoordinatorPayload
    typealias LegacyDispatch = @MainActor (_ method: String, _ args: CoordinatorPayload) async throws -> CoordinatorPayload

    private let dispatchCommand: Dispatch
    private let serves: (String) -> Bool

    init(
        serves: @escaping (String) -> Bool = CoordinatorMethodRegistry.contains,
        dispatchCommand: @escaping Dispatch
    ) {
        self.serves = serves
        self.dispatchCommand = dispatchCommand
    }

    convenience init(
        serves: @escaping (String) -> Bool = CoordinatorMethodRegistry.contains,
        dispatchCommand: @escaping LegacyDispatch
    ) {
        self.init(serves: serves) { _, method, args in
            try await dispatchCommand(method, args)
        }
    }

    func dispatch(method: String, args: CoordinatorPayload) async -> CoordinatorReplyOutcome {
        await dispatch(
            requestId: UUID().uuidString.lowercased(),
            method: method,
            args: args
        )
    }

    func dispatch(
        requestId: String,
        method: String,
        args: CoordinatorPayload
    ) async -> CoordinatorReplyOutcome {
        guard serves(method) else {
            return .failed(.init(
                code: CoordinatorProtocol.unknownMethod,
                message: "no coordinator method named \(method)"
            ))
        }
        do {
            return .ok(try await dispatchCommand(requestId, method, args))
        } catch is CancellationError {
            return .failed(.init(
                code: CoordinatorProtocol.cancelled,
                message: "request cancelled"
            ))
        } catch {
            return .failed(GatewayFailureMapper.failure(for: error))
        }
    }
}
