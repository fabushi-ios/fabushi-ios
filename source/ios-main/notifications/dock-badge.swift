import Foundation
import UserNotifications

struct IOSDockBadgeAgent: Equatable, Sendable {
    let hasUnread: Bool?
    let isHiddenFromSidebar: Bool?
    let unreadCount: Double?

    init(
        hasUnread: Bool? = nil,
        isHiddenFromSidebar: Bool? = nil,
        unreadCount: Double? = nil
    ) {
        self.hasUnread = hasUnread
        self.isHiddenFromSidebar = isHiddenFromSidebar
        self.unreadCount = unreadCount
    }
}

func computeDockBadgeTotal(_ agents: [IOSDockBadgeAgent]) -> Int {
    var total = 0
    for agent in agents {
        guard agent.hasUnread == true, agent.isHiddenFromSidebar != true else {
            continue
        }
        let raw = agent.unreadCount ?? 1
        let normalized: Int
        if raw.isFinite {
            normalized = max(Int(floor(raw)), 1)
        } else {
            normalized = 1
        }
        total += normalized
    }
    return total
}

protocol IOSAppBadgeApplying: Sendable {
    func setBadgeCount(_ count: Int) async
}

struct IOSUserNotificationBadgeApplier: IOSAppBadgeApplying {
    func setBadgeCount(_ count: Int) async {
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            UNUserNotificationCenter.current().setBadgeCount(max(0, count)) { _ in
                continuation.resume()
            }
        }
    }
}

actor IOSDockBadgeController {
    private let applier: any IOSAppBadgeApplying

    init(applier: any IOSAppBadgeApplying = IOSUserNotificationBadgeApplier()) {
        self.applier = applier
    }

    @discardableResult
    func update(agents: [IOSDockBadgeAgent]) async -> Int {
        let total = computeDockBadgeTotal(agents)
        await applier.setBadgeCount(total)
        return total
    }
}
