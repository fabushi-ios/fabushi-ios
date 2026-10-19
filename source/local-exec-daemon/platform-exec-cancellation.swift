import Foundation

enum IOSPlatformExecCancellationRace {
    private enum Event<T: Sendable>: Sendable {
        case result(T)
        case cancelled
    }

    static func run<T: Sendable>(
        pollInterval: Duration = .milliseconds(100),
        operation: @escaping @Sendable () async throws -> T,
        cancellationRequested: @escaping @Sendable () async throws -> Bool
    ) async throws -> T {
        try await withThrowingTaskGroup(
            of: Event<T>.self,
            returning: T.self
        ) { group in
            group.addTask {
                .result(try await operation())
            }
            group.addTask {
                while true {
                    try await Task.sleep(for: pollInterval)
                    try Task.checkCancellation()
                    if try await cancellationRequested() {
                        return .cancelled
                    }
                }
            }

            guard let first = try await group.next() else {
                throw CancellationError()
            }
            group.cancelAll()
            switch first {
            case .result(let value):
                return value
            case .cancelled:
                throw CancellationError()
            }
        }
    }
}
