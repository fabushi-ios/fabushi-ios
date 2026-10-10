import Foundation

@MainActor
final class CoordinatorControlExecutors {
    typealias Executor = @MainActor (CoordinatorPayload) async throws -> CoordinatorPayload

    private var executors: [String: Executor] = [:]

    func register(_ method: String, executor: @escaping Executor) {
        guard !method.isEmpty else { return }
        executors[method] = executor
    }

    func unregister(_ method: String) {
        executors.removeValue(forKey: method)
    }

    func execute(method: String, args: CoordinatorPayload) async -> CoordinatorReplyOutcome {
        guard let executor = executors[method] else {
            return .failed(.init(code: "main-unknown-command", message: "no control command named \(method)"))
        }
        do {
            return .ok(try await executor(args))
        } catch {
            return .failed(.init(code: "main-execution-failure", message: error.localizedDescription))
        }
    }
}
