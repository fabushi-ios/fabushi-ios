import Foundation

@MainActor
final class IOSDevNetworkLatency {
    static let maximumMilliseconds = 10_000
    typealias Sleeper = @MainActor (UInt64) async throws -> Void

    private let sleeper: Sleeper
    private(set) var currentMilliseconds = 0

    init(
        sleeper: @escaping Sleeper = { nanoseconds in
            try await Task<Never, Never>.sleep(nanoseconds: nanoseconds)
        }
    ) {
        self.sleeper = sleeper
    }

    @discardableResult
    func setMilliseconds(_ rawMilliseconds: Int) -> Int {
        if rawMilliseconds <= 0 {
            currentMilliseconds = 0
        } else {
            currentMilliseconds = min(rawMilliseconds, Self.maximumMilliseconds)
        }
        return currentMilliseconds
    }

    func applyBeforeProductionRequest() async throws {
        guard currentMilliseconds > 0 else { return }
        try await sleeper(UInt64(currentMilliseconds) * 1_000_000)
    }
}
