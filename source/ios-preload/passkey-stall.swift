import Foundation

enum IOSPasskeyStallError: LocalizedError, Equatable, Sendable {
    case stalled(method: String)

    var errorDescription: String? {
        switch self {
        case .stalled(let method):
            return "Passkey request stalled: \(method)"
        }
    }
}

enum IOSPasskeyStall {
    static let stallNanoseconds: UInt64 = 60_000_000_000

    static func run<T: Sendable>(
        method: String,
        sinceMilliseconds: Int64,
        timeoutNanoseconds: UInt64 = stallNanoseconds,
        operation: @escaping @Sendable () async throws -> T,
        reportStall: @escaping @Sendable (_ method: String, _ sinceMilliseconds: Int64) -> Void
    ) async throws -> T {
        try await withThrowingTaskGroup(of: T.self, returning: T.self) { group in
            group.addTask { try await operation() }
            group.addTask {
                try await Task.sleep(nanoseconds: timeoutNanoseconds)
                reportStall(method, sinceMilliseconds)
                throw IOSPasskeyStallError.stalled(method: method)
            }
            defer { group.cancelAll() }
            guard let result = try await group.next() else {
                throw IOSPasskeyStallError.stalled(method: method)
            }
            return result
        }
    }
}
