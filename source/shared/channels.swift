import Foundation

let DISCORD_PLATFORM = "discord"
let SLACK_PLATFORM = "slack"

struct ConnectorManifest: Codable, Equatable, Sendable {
    enum Availability: String, Codable, Equatable, Sendable {
        case available
        case comingSoon = "coming-soon"
    }

    let platform: String
    let displayName: String
    let blurb: String
    let credentialLabel: String
    let availability: Availability
    let connectGuide: String
}

let CONNECTOR_MANIFESTS: [ConnectorManifest] = [
    .init(
        platform: DISCORD_PLATFORM,
        displayName: "Discord",
        blurb: "Message in Discord servers and DMs (coming soon).",
        credentialLabel: "bot token",
        availability: .comingSoon,
        connectGuide: ""
    ),
    .init(
        platform: SLACK_PLATFORM,
        displayName: "Slack",
        blurb: "Message in Slack channels and DMs (coming soon).",
        credentialLabel: "app token",
        availability: .comingSoon,
        connectGuide: ""
    ),
]

struct ChannelAddress: Codable, Equatable, Sendable {
    let platform: String
    let chat: String
}

func findConnectorManifest(_ platform: String) -> ConnectorManifest? {
    CONNECTOR_MANIFESTS.first { $0.platform == platform }
}

func hasChannelsToShow<Connection>(
    manifests: [ConnectorManifest],
    connections: [Connection]
) -> Bool {
    manifests.contains { $0.availability == .available } || !connections.isEmpty
}

func formatChannelAddress(_ address: ChannelAddress) -> String {
    "\(address.platform):\(address.chat)"
}

func parseChannelAddress(_ raw: String) -> ChannelAddress? {
    let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
    guard let separator = trimmed.firstIndex(of: ":"),
          separator != trimmed.startIndex
    else {
        return nil
    }
    let platform = String(trimmed[..<separator])
        .trimmingCharacters(in: .whitespacesAndNewlines)
    let chat = String(trimmed[trimmed.index(after: separator)...])
        .trimmingCharacters(in: .whitespacesAndNewlines)
    guard !platform.isEmpty, !chat.isEmpty else { return nil }
    return .init(platform: platform, chat: chat)
}
