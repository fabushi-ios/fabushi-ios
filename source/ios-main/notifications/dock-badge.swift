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
    private var latestGeneration: UInt64 = 0
    private var latestTotal = 0

    init(applier: any IOSAppBadgeApplying = IOSUserNotificationBadgeApplier()) {
        self.applier = applier
    }

    @discardableResult
    func update(
        agents: [IOSDockBadgeAgent],
        generation: UInt64
    ) async -> Int? {
        guard generation >= latestGeneration else { return nil }
        latestGeneration = generation
        let total = computeDockBadgeTotal(agents)
        latestTotal = total
        await applier.setBadgeCount(total)
        guard generation == latestGeneration else { return nil }
        return total
    }

    func reset(generation: UInt64) async {
        guard generation >= latestGeneration else { return }
        latestGeneration = generation
        latestTotal = 0
        await applier.setBadgeCount(0)
    }

    func snapshot() -> (generation: UInt64, total: Int) {
        (latestGeneration, latestTotal)
    }
}


enum IOSAgentNotificationAuthorization: Equatable, Sendable {
    case notDetermined
    case denied
    case authorized
}

protocol IOSAgentNotificationScheduling: Sendable {
    func authorizationState() async -> IOSAgentNotificationAuthorization
    func requestAuthorization() async -> Bool
    func schedule(
        identifier: String,
        title: String,
        body: String,
        silent: Bool,
        userInfo: [String: String]
    ) async throws
    func clearAgentNotifications() async
}

struct IOSUserNotificationScheduler: IOSAgentNotificationScheduling {
    func authorizationState() async -> IOSAgentNotificationAuthorization {
        let settings = await UNUserNotificationCenter.current().notificationSettings()
        switch settings.authorizationStatus {
        case .authorized, .provisional, .ephemeral:
            return .authorized
        case .notDetermined:
            return .notDetermined
        case .denied:
            return .denied
        @unknown default:
            return .denied
        }
    }

    func requestAuthorization() async -> Bool {
        (try? await UNUserNotificationCenter.current().requestAuthorization(
            options: [.alert, .badge, .sound]
        )) == true
    }

    func schedule(
        identifier: String,
        title: String,
        body: String,
        silent: Bool,
        userInfo: [String: String]
    ) async throws {
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        content.userInfo = userInfo
        if !silent {
            content.sound = .default
        }
        try await UNUserNotificationCenter.current().add(
            UNNotificationRequest(identifier: identifier, content: content, trigger: nil)
        )
    }

    func clearAgentNotifications() async {
        let center = UNUserNotificationCenter.current()
        let prefix = IOSAgentNotificationController.identifierPrefix
        let pending = await center.pendingNotificationRequests()
            .map(\.identifier)
            .filter { $0.hasPrefix(prefix) }
        center.removePendingNotificationRequests(withIdentifiers: pending)
        let delivered = await center.deliveredNotifications()
            .map { $0.request.identifier }
            .filter { $0.hasPrefix(prefix) }
        center.removeDeliveredNotifications(withIdentifiers: delivered)
    }
}

actor IOSAgentNotificationController {
    static let identifierPrefix = "fabushi.agent."
    static let agentIdUserInfoKey = "fabushiAgentId"
    static let accountScopeUserInfoKey = "fabushiAccountScope"

    private let scheduler: any IOSAgentNotificationScheduling
    private var decider = SandOsNotificationDecider()
    private var latestGeneration: UInt64 = 0
    private var accountScope: String?
    private var seeded = false

    init(scheduler: any IOSAgentNotificationScheduling = IOSUserNotificationScheduler()) {
        self.scheduler = scheduler
    }

    func update(
        agents: [NotificationSnapshot],
        accountScope: String,
        isAppActive: Bool,
        generation: UInt64,
        nowMs: Int64 = Int64(Date().timeIntervalSince1970 * 1_000)
    ) async {
        guard generation >= latestGeneration else { return }
        if self.accountScope != accountScope {
            decider = SandOsNotificationDecider()
            seeded = false
            self.accountScope = accountScope
        }
        latestGeneration = generation

        if !seeded {
            decider.seedBaseline(agents)
            seeded = true
            return
        }

        let transitions = decider.decide(
            agents: agents,
            isWindowFocused: isAppActive,
            nowMs: nowMs
        )
        guard !transitions.isEmpty else { return }

        var authorization = await scheduler.authorizationState()
        guard generation == latestGeneration, self.accountScope == accountScope else { return }
        if authorization == .notDetermined {
            authorization = await scheduler.requestAuthorization() ? .authorized : .denied
            guard generation == latestGeneration, self.accountScope == accountScope else { return }
        }
        guard authorization == .authorized else { return }

        for transition in transitions {
            guard generation == latestGeneration, self.accountScope == accountScope else { return }
            let content = buildNotificationContent(transition)
            let messageIdentity = transition.lastMessageId ?? "state"
            let identifier = [
                Self.identifierPrefix + accountScope,
                transition.agentId,
                transition.kind.rawValue,
                messageIdentity,
            ].joined(separator: ".")
            try? await scheduler.schedule(
                identifier: identifier,
                title: content.title,
                body: content.body,
                silent: transition.kind == .agentDone,
                userInfo: [
                    Self.agentIdUserInfoKey: transition.agentId,
                    Self.accountScopeUserInfoKey: accountScope,
                ]
            )
        }
    }

    func reset(generation: UInt64) async {
        guard generation >= latestGeneration else { return }
        latestGeneration = generation
        accountScope = nil
        seeded = false
        decider = SandOsNotificationDecider()
        await scheduler.clearAgentNotifications()
    }
}

extension Notification.Name {
    static let fabushiOpenAgentFromNotification =
        Notification.Name("fabushi.open-agent-from-notification")
}
