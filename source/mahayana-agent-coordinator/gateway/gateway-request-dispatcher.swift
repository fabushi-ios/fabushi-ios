import Foundation

@MainActor
final class GatewayRequestDispatcher {
    typealias Dispatch = @MainActor (_ method: String, _ args: CoordinatorPayload) async throws -> CoordinatorPayload

    private let dispatchCommand: Dispatch
    private let serves: (String) -> Bool

    init(
        serves: @escaping (String) -> Bool = CoordinatorMethodRegistry.contains,
        dispatchCommand: @escaping Dispatch
    ) {
        self.serves = serves
        self.dispatchCommand = dispatchCommand
    }

    func dispatch(method: String, args: CoordinatorPayload) async -> CoordinatorReplyOutcome {
        guard serves(method) else {
            return .failed(.init(
                code: CoordinatorProtocol.unknownMethod,
                message: "no coordinator method named \(method)"
            ))
        }
        do {
            return .ok(try await dispatchCommand(method, args))
        } catch {
            return .failed(GatewayFailureMapper.failure(for: error))
        }
    }
}
