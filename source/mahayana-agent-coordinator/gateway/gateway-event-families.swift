import Foundation

enum CoordinatorEventFamilies {
    static let channelByFamily: [String: String] = [
        "transcript": "transcript",
        "client-side-tool-v2": "client-side-tool-v2",
        "agents": "agents",
        "agent-upserted": "agent-upserted",
        "tray": "tray",
        "agents-workflow": "workflows",
        "subagents": "subagents",
        "async-tasks": "async-tasks",
        "agents-automation": "automations",
        "mcp-servers-updated": "mcp-servers",
        "forever-box": "forever-box",
        "teach-recording": "teach-recording",
        "box-disk-pressure": "box-disk-pressure",
        "computer-action": "computer-action",
        "outline": "outline",
        "sharing": "sharing",
        "host-settings": "host-settings"
    ]

    private static let familyByChannel = Dictionary(
        uniqueKeysWithValues: channelByFamily.map { ($0.value, $0.key) }
    )

    static func family(forChannel channel: String) -> String? {
        familyByChannel[channel]
    }
}
