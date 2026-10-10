import Foundation

let SAND_OS_NOTIFICATION_THROTTLE_MS: Int64 = 5_000
let MAX_NOTIFICATION_BODY_LENGTH = 140

struct NotificationSnapshot: Equatable, Sendable {
    let id: String
    let name: String
    let isRunning: Bool
    let awaitingReason: String?
    let notifyEnabled: Bool
    let isHiddenFromSidebar: Bool
    var lastMessageId: String? = nil
    let lastMessagePreview: String?
}

struct NotificationAgent: Equatable, Sendable {
    let id: String
    let name: String
    let isRunning: Bool
    var awaitingReason: String? = nil
    let notifyOnUpdatesEnabled: Bool
    var isHiddenFromSidebar = false
    var lastMessageId: String? = nil
    var lastMessagePreview: String? = nil
}

enum NotificationTransitionKind: String, Equatable, Sendable {
    case agentNeedsInput = "agent-needs-input"
    case agentDone = "agent-done"
}

struct NotificationTransition: Equatable, Sendable {
    let agentId: String
    let agentName: String
    let kind: NotificationTransitionKind
    let reason: String?
    let notifyEnabled: Bool
    let isHiddenFromSidebar: Bool
    var lastMessageId: String? = nil
    let lastMessagePreview: String?
}

struct NotificationContent: Equatable, Sendable {
    let title: String
    let body: String
}

func toNotificationSnapshot(_ agent: NotificationAgent) -> NotificationSnapshot {
    .init(
        id: agent.id,
        name: agent.name,
        isRunning: agent.isRunning,
        awaitingReason: agent.awaitingReason,
        notifyEnabled: agent.notifyOnUpdatesEnabled,
        isHiddenFromSidebar: agent.isHiddenFromSidebar,
        lastMessageId: agent.lastMessageId,
        lastMessagePreview: agent.lastMessagePreview
    )
}

func diffAgentNotificationTransitions(
    _ previous: [String: NotificationSnapshot],
    next: [NotificationSnapshot]
) -> [NotificationTransition] {
    var transitions: [NotificationTransition] = []
    for agent in next {
        guard let before = previous[agent.id] else { continue }
        let becameAwaiting = agent.awaitingReason != nil && before.awaitingReason == nil
        let finishedTurn = before.isRunning && !agent.isRunning && agent.awaitingReason == nil
        guard becameAwaiting || finishedTurn else { continue }
        transitions.append(.init(
            agentId: agent.id,
            agentName: agent.name,
            kind: becameAwaiting ? .agentNeedsInput : .agentDone,
            reason: becameAwaiting ? agent.awaitingReason : nil,
            notifyEnabled: agent.notifyEnabled,
            isHiddenFromSidebar: agent.isHiddenFromSidebar,
            lastMessageId: agent.lastMessageId,
            lastMessagePreview: agent.lastMessagePreview
        ))
    }
    return transitions
}

func shouldNotify(
    isHidden: Bool,
    notifyEnabled: Bool,
    isWindowFocused: Bool,
    lastNotifiedAtMs: Int64?,
    nowMs: Int64,
    throttleWindowMs: Int64
) -> Bool {
    !isHidden
        && notifyEnabled
        && !isWindowFocused
        && (lastNotifiedAtMs == nil || nowMs - lastNotifiedAtMs! >= throttleWindowMs)
}

private func truncateNotificationText(_ text: String) -> String {
    let collapsed = text.split(whereSeparator: { $0.isWhitespace }).joined(separator: " ")
    guard collapsed.count > MAX_NOTIFICATION_BODY_LENGTH else { return collapsed }
    return String(collapsed.prefix(MAX_NOTIFICATION_BODY_LENGTH - 1)).trimmingCharacters(in: .whitespaces) + "…"
}

func buildNotificationContent(_ transition: NotificationTransition) -> NotificationContent {
    let trimmedName = transition.agentName.trimmingCharacters(in: .whitespacesAndNewlines)
    let name = trimmedName.isEmpty ? "Your agent" : trimmedName
    if transition.kind == .agentNeedsInput {
        let reason = transition.reason?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return .init(
            title: "\(name) needs you",
            body: reason.isEmpty ? "Waiting for your input." : truncateNotificationText(reason)
        )
    }
    let summary = transition.lastMessagePreview?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    return .init(
        title: name,
        body: summary.isEmpty ? "Open Fabushi to see what it did." : truncateNotificationText(summary)
    )
}

final class SandOsNotificationDecider {
    private var previous: [String: NotificationSnapshot] = [:]
    private var lastNotifiedAtMs: [String: Int64] = [:]
    private var accountedMessageId: [String: String] = [:]
    private var accountedNilMessageId = Set<String>()

    let throttleWindowMs: Int64

    init(throttleWindowMs: Int64 = SAND_OS_NOTIFICATION_THROTTLE_MS) {
        self.throttleWindowMs = throttleWindowMs
    }

    func decide(
        agents: [NotificationSnapshot],
        isWindowFocused: Bool,
        nowMs: Int64
    ) -> [NotificationTransition] {
        let transitions = gateTransitions(
            diffAgentNotificationTransitions(previous, next: agents),
            isWindowFocused: isWindowFocused,
            nowMs: nowMs
        )
        var next: [String: NotificationSnapshot] = [:]
        for agent in agents {
            if !hasAccountedMessage(agent.id) {
                recordMessage(agent.lastMessageId, for: agent.id)
            }
            next[agent.id] = agent
        }
        previous = next
        return transitions
    }

    func seedBaseline(_ agents: [NotificationSnapshot]) {
        for agent in agents {
            if previous[agent.id] == nil {
                previous[agent.id] = agent
            }
            if !hasAccountedMessage(agent.id) {
                recordMessage(agent.lastMessageId, for: agent.id)
            }
        }
    }

    func decideAgent(
        _ agent: NotificationSnapshot,
        isWindowFocused: Bool,
        nowMs: Int64
    ) -> [NotificationTransition] {
        let transitions = gateTransitions(
            diffAgentNotificationTransitions(previous, next: [agent]),
            isWindowFocused: isWindowFocused,
            nowMs: nowMs
        )
        if !hasAccountedMessage(agent.id) {
            recordMessage(agent.lastMessageId, for: agent.id)
        }
        previous[agent.id] = agent
        return transitions
    }

    func observeAgent(_ agent: NotificationSnapshot) {
        if !hasAccountedMessage(agent.id) {
            recordMessage(agent.lastMessageId, for: agent.id)
        }
        previous[agent.id] = agent
    }

    func forget(_ agentId: String) {
        previous.removeValue(forKey: agentId)
        accountedMessageId.removeValue(forKey: agentId)
        accountedNilMessageId.remove(agentId)
        lastNotifiedAtMs.removeValue(forKey: throttleKey(agentId, .agentDone))
        lastNotifiedAtMs.removeValue(forKey: throttleKey(agentId, .agentNeedsInput))
    }

    private func gateTransitions(
        _ transitions: [NotificationTransition],
        isWindowFocused: Bool,
        nowMs: Int64
    ) -> [NotificationTransition] {
        var output: [NotificationTransition] = []
        for transition in transitions {
            if transition.kind == .agentDone {
                guard let messageId = transition.lastMessageId else { continue }
                if hasAccountedMessage(transition.agentId),
                   accountedMessageId[transition.agentId] == messageId {
                    continue
                }
            }
            recordMessage(transition.lastMessageId, for: transition.agentId)
            let key = throttleKey(transition.agentId, transition.kind)
            if shouldNotify(
                isHidden: transition.isHiddenFromSidebar,
                notifyEnabled: transition.notifyEnabled,
                isWindowFocused: isWindowFocused,
                lastNotifiedAtMs: lastNotifiedAtMs[key],
                nowMs: nowMs,
                throttleWindowMs: throttleWindowMs
            ) {
                lastNotifiedAtMs[key] = nowMs
                output.append(transition)
            }
        }
        return output
    }

    private func throttleKey(_ agentId: String, _ kind: NotificationTransitionKind) -> String {
        "\(agentId):\(kind.rawValue)"
    }

    private func hasAccountedMessage(_ agentId: String) -> Bool {
        accountedMessageId[agentId] != nil || accountedNilMessageId.contains(agentId)
    }

    private func recordMessage(_ messageId: String?, for agentId: String) {
        accountedMessageId.removeValue(forKey: agentId)
        accountedNilMessageId.remove(agentId)
        if let messageId {
            accountedMessageId[agentId] = messageId
        } else {
            accountedNilMessageId.insert(agentId)
        }
    }
}
