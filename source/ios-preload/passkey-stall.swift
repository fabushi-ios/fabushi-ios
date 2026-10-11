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


/// Shipping deadline owner for native passkey ceremonies.
///
/// Desktop fences navigator.credentials at the trusted preload boundary. iOS
/// performs the same ceremony through AuthenticationServices, so the deadline
/// lives beside the native provider instead of a WebView shim. Re-arming
/// advances a generation and cancels the prior timer; stale timers therefore
/// cannot settle a replacement ceremony.
@MainActor
final class IOSPasskeyCeremonyDeadline {
    typealias StallReporter = @MainActor (_ method: String, _ sinceMilliseconds: Int64) -> Void
    typealias TimeoutHandler = @MainActor (_ generation: UInt64) -> Void

    private let timeoutNanoseconds: UInt64
    private let reportStall: StallReporter
    private var timeoutTask: Task<Void, Never>?
    private(set) var generation: UInt64 = 0

    init(
        timeoutNanoseconds: UInt64 = IOSPasskeyStall.stallNanoseconds,
        reportStall: @escaping StallReporter = { _, _ in }
    ) {
        self.timeoutNanoseconds = timeoutNanoseconds
        self.reportStall = reportStall
    }

    @discardableResult
    func arm(
        method: String,
        sinceMilliseconds: Int64,
        onTimeout: @escaping TimeoutHandler
    ) -> UInt64 {
        timeoutTask?.cancel()
        generation &+= 1
        if generation == 0 { generation = 1 }
        let currentGeneration = generation
        let delay = timeoutNanoseconds

        timeoutTask = Task { @MainActor [weak self] in
            do {
                try await Task<Never, Never>.sleep(nanoseconds: delay)
            } catch {
                return
            }
            guard let self,
                  !Task.isCancelled,
                  self.generation == currentGeneration
            else { return }

            self.timeoutTask = nil
            self.reportStall(method, sinceMilliseconds)
            onTimeout(currentGeneration)
        }
        return currentGeneration
    }

    func disarm(generation expectedGeneration: UInt64) {
        guard expectedGeneration == generation else { return }
        timeoutTask?.cancel()
        timeoutTask = nil
    }

    func isCurrent(_ expectedGeneration: UInt64) -> Bool {
        expectedGeneration == generation
    }
}
