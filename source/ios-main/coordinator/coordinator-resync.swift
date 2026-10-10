import Foundation

typealias DisabledToolsByServer = [String: [String]]

enum CoordinatorResync {
    static func unionDisabledTools(
        _ lhs: DisabledToolsByServer,
        _ rhs: DisabledToolsByServer
    ) -> DisabledToolsByServer {
        var merged = lhs
        for (server, names) in rhs {
            merged[server] = Array(Set((merged[server] ?? []) + names)).sorted()
        }
        return merged
    }
}

@MainActor
final class CoordinatorResyncChain {
    struct Step {
        let name: String
        let run: @MainActor () async throws -> Void
    }

    struct Result: Equatable, Sendable {
        let failedSteps: [String]
        let durationMilliseconds: Int
    }

    private let steps: [Step]
    private let now: () -> Date

    init(steps: [Step], now: @escaping () -> Date = Date.init) {
        self.steps = steps
        self.now = now
    }

    func runOnce() async -> Result {
        let started = now()
        var failures: [String] = []
        for step in steps {
            do {
                try await step.run()
            } catch {
                failures.append(step.name)
            }
        }
        let duration = max(0, Int(now().timeIntervalSince(started) * 1_000))
        return .init(failedSteps: failures, durationMilliseconds: duration)
    }
}
