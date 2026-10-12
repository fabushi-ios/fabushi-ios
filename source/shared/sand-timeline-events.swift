import Foundation

let AUTOMATION_ACTION_VERB = [
    "created": "Created",
    "updated": "Updated",
    "enabled": "Enabled",
    "disabled": "Disabled",
    "deleted": "Deleted",
]
let TIMELINE_EVENT_WAKE_CUE = "[event]"

enum SandTimelineEvent: Equatable, Sendable {
    case nameChanged(to: String)
    case channelConnected(label: String)
    case channelDisconnected(label: String)
    case automationChanged(action: String, automationName: String)
    case unknown(type: String)
}

func fallbackForUnknownTimelineEvent(_ fallback: String) -> String {
    fallback
}

func describeTimelineEvent(_ event: SandTimelineEvent) -> String {
    switch event {
    case .nameChanged(let to):
        return "Renamed to \(to)"
    case .channelConnected(let label):
        return "Connected to \(label)"
    case .channelDisconnected(let label):
        return "Disconnected from \(label)"
    case .automationChanged(let action, let automationName):
        return "\(AUTOMATION_ACTION_VERB[action] ?? "Changed") automation \"\(automationName)\""
    case .unknown:
        return fallbackForUnknownTimelineEvent("Updated this conversation")
    }
}

func buildTimelineEventWakePrompt(_ events: [SandTimelineEvent]) -> String {
    let lines = events.map { "- \(describeTimelineEvent($0))" }
    return ([
        "\(TIMELINE_EVENT_WAKE_CUE) Something about this conversation just changed.",
        "This is a system event recorded in your timeline, not the user typing in this app, and possibly something you did yourself.",
    ] + lines + [
        "If it is worth acknowledging to the user, reply with SendMessage; otherwise it is fine to stay silent.",
    ]).joined(separator: "\n")
}
