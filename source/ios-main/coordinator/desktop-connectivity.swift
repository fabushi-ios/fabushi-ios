import Foundation

@MainActor
final class IOSConnectivity {
    static let recentResumeWindow: TimeInterval = 60

    private let isOnlineProvider: @MainActor () -> Bool
    private let now: () -> Date
    private var lastResumeAt: Date?

    init(
        now: @escaping () -> Date = Date.init,
        isOnline: @escaping @MainActor () -> Bool
    ) {
        self.now = now
        isOnlineProvider = isOnline
    }

    func isOnline() -> Bool {
        isOnlineProvider()
    }

    func noteResume() {
        lastResumeAt = now()
    }

    func recentResume() -> Bool {
        guard let lastResumeAt else { return false }
        return now().timeIntervalSince(lastResumeAt) < Self.recentResumeWindow
    }

    func telemetryStamps() -> [String: String] {
        [
            "client_online": String(isOnline()),
            "recent_wake": String(recentResume()),
        ]
    }
}
